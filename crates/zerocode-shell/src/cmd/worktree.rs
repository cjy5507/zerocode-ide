//! Worktree commands.

use crate::*;

/// Orca's title-derived git name: four ASCII words, with a ledger task id kept
/// in front of (and outside) that word budget.
///
/// `t-2089` is identity, not prose. Feeding it through the ordinary four-word
/// sanitizer would count `t` and `2089` as two words and leave room for only
/// half a title. Keeping it first also preserves the orchestrator's
/// `t-<id>/<slug>` branch grouping.
pub(crate) fn workspace_title_slug(title: &str) -> String {
    use zerocode_core::source_control_ai as ai;

    let title = title.trim();
    let (task_id, prose) =
        leading_task_id(title).map_or((None, title), |(id, rest)| (Some(id), rest));
    let slug = ai::sanitize_branch_slug(prose, ai::MAX_BRANCH_NAME_WORDS);
    match (task_id, slug.is_empty()) {
        (Some(id), true) => format!("t-{id}"),
        (Some(id), false) => format!("t-{id}-{slug}"),
        (None, true) => zerocode_orchestrator::naming::FALLBACK_SLUG.to_string(),
        (None, false) => slug,
    }
}

/// A leading `t-` followed by digits, only when the next character is a
/// separator or the title ends. `t-12factor` is prose, not task 12.
fn leading_task_id(title: &str) -> Option<(&str, &str)> {
    // `work_item_seed` title-cases free text for display, so accept its `T-`
    // too and normalize the emitted identity back to canonical lowercase.
    let suffix = title
        .strip_prefix("t-")
        .or_else(|| title.strip_prefix("T-"))?;
    let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &suffix[digits..];
    if rest
        .chars()
        .next()
        .is_some_and(|one| one.is_ascii_alphanumeric())
    {
        return None;
    }
    Some((&suffix[..digits], rest))
}

/// Preserve the human title for the window while giving the orchestrator the
/// bounded title it uses for the checkout path and branch.
fn workspace_task_from_spec(spec: &str) -> WorktreeTask {
    let mut task = WorktreeTask::from_spec(spec, None, None);
    task.task_title = workspace_title_slug(&task.task_title);
    task
}

/// Every workspace of the active project, the repository's own checkout first.
/// A non-git project contributes its one folder workspace instead of an empty
/// project header.
#[tauri::command]
pub(crate) async fn list_worktrees(
    state: State<'_, AppState>,
) -> Result<Vec<WorktreeEntry>, String> {
    let here = state.active();
    let Some(orchestrator) = here.orchestrator else {
        return Ok(vec![WorktreeEntry::folder(&here.root, &here.root)]);
    };
    let worktrees = orchestrator.list().map_err(|error| error.to_string())?;
    let links = catalog_work_item_links(state.settings());
    let mut entries: Vec<WorktreeEntry> = worktrees
        .into_iter()
        .map(|worktree| WorktreeEntry::new(worktree, &here.root))
        .collect();
    for entry in &mut entries {
        decorate_worktree_link(entry, &links);
    }
    attach_creation_bases_from(&mut entries, &orchestrator.creation_bases());
    Ok(entries)
}

/// Which agent the orchestration ledger last seated in a checkout, if any.
///
/// `None` for a checkout no worker was ever cut for, and when the runtime is
/// down — the window then opens what it always opened. Async: the ledger image
/// is rebuilt to answer, and a restore is not a keystroke's thread to hold.
#[tauri::command(async)]
pub(crate) fn worktree_last_agent(worktree: String) -> Option<orchestration::LastAgentInCheckout> {
    orchestration::last_agent_in_checkout(&worktree)
}

/// What the set of checkouts looks like right now, cheaply enough to ask often.
///
/// Every linked worktree owns one entry under the repository's shared
/// `worktrees` directory, so the names in there change exactly when a checkout
/// is created or removed — by this window, by a person at a shell, or by an
/// agent that ran `git worktree add` in a pane. Reading them costs one
/// directory listing per project; asking git the same question costs a
/// subprocess per project, which is why the list is not simply re-fetched on a
/// beat.
///
/// The original watches this same place natively and keeps a two-second poller
/// behind the watcher for the cases a watcher cannot be trusted
/// (`worktree-base-directory-poller.ts`, `WORKTREE_BASE_POLL_INTERVAL_MS`).
/// This is that poller's half, and it is the half that answers "a checkout
/// somebody else made shows up without being asked for".
///
/// A root that cannot be read contributes nothing rather than an error: an
/// unreadable project is a fact the catalog already reports, and a stamp that
/// failed would only stop the window noticing the projects that CAN be read.
#[tauri::command(async)]
pub(crate) fn worktree_stamp(
    app: AppHandle,
    state: State<'_, AppState>,
    roots: Vec<String>,
) -> String {
    file_tree_hooks::reconcile(&app, &state);
    worktree_stamp_of(roots)
}

/// Create a worktree for a task, named from its spec.
///
/// Async because `git worktree add` writes a whole checkout: on a large
/// repository that is seconds, and they must not be seconds the window cannot
/// paint in.
///
/// Nine arguments, and they stay nine for the reason [`launch_agent_tab`]'s
/// eight do: a `#[tauri::command]`'s parameters ARE its wire shape, so folding
/// them into a struct would rename every field the window sends — a payload
/// change to quiet a lint about a payload.
///
/// Five of the arguments are `Option` and every one of them is absent on the
/// ordinary call. That is the compatibility rule this command is written to:
/// an automation, a test, and the dialog's plain path all send `spec` and a
/// project, and everything the create dialog learned to ask for arrives beside
/// them rather than instead of them.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn create_worktree(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: String,
    project: Option<String>,
    // `run_setup` is `None` for "whatever the policy says", which is what every
    // ordinary call passes; the window spells it only when the policy was `ask`
    // and somebody answered.
    run_setup: Option<bool>,
    // `base` walks the ladder in `resolve_create_base` when absent — this
    // repository's remembered base, then its default branch.
    base: Option<String>,
    // `branch` is the name somebody typed in Advanced, instead of prefix+slug,
    // and `reuse_branch` checks that branch out rather than cutting it
    // (Orca's "Reuse branch").
    branch: Option<String>,
    reuse_branch: Option<bool>,
    // What this checkout should be COMPARED with, when that is not the same as
    // what it was cut from. A pull request is the case: it is cut from its own
    // head commit and compared against the branch it is opened against
    // (Orca's `compareBaseRef`, 스펙 §2).
    compare_base: Option<String>,
    // Where a fix made in this checkout goes back to, when the resolve found
    // that out — a pull request's own branch, on its own repository when the
    // pull request came from a fork.
    push_target: Option<PushTargetArg>,
    // The window's name for this creation, so the pending row it drew can be
    // told which phase this is in. Absent means nobody is watching.
    creation_id: Option<String>,
    // A destination for this request only; never persisted as a global preference.
    directory: Option<String>,
) -> Result<WorktreeEntry, String> {
    let orchestrator = match &project {
        Some(path) => known_project_orchestrator(state.config_root(), path)?,
        None => state.orchestrator().ok_or(NOT_A_REPOSITORY)?,
    };
    let active_root = state.active_root();
    let repo_root = orchestrator.repo_root().to_path_buf();
    let settings_document = load_settings_for_boot(state.settings()).document;
    let setup_launch_mode = settings_document.setup_script_launch_mode;
    let refresh_local_base_ref = settings_document.refresh_local_base_ref_on_worktree_create;
    let prefs = settings_document.worktree_prefs;
    let mut workspace_creation_prefs = settings_document.workspace_creation_prefs;
    if let Some(directory) = directory {
        workspace_creation_prefs.directory = directory;
    }
    let settings_key = project_settings_key(&repo_root);
    let pinned = stored_project_settings_at(state.settings(), &settings_key)?.worktree_base_ref;
    let settings_repository = Arc::clone(state.settings());
    let config_root = state.config_root().to_path_buf();
    let (mut entry, setup, repo_root, worktree_path) =
        tauri::async_runtime::spawn_blocking(move || {
            // The host this repository lives on, decided once and carried. Every
            // git call below that is not `git worktree` goes through it — the
            // boundary is what keeps "and now do it over SSH" from being 737
            // inline branches (`zerocode_core::host`).
            let host = Host::for_workspace(&repo_root);
            let say = |phase: &'static str| {
                if let Some(id) = &creation_id {
                    let _ = app.emit(
                        "worktree:creating",
                        CreationPhase {
                            id: id.clone(),
                            phase,
                        },
                    );
                }
            };
            say("preparing");

            let prefix = resolve_branch_prefix(&prefs, git_username(&host, &repo_root).as_deref())?;
            let orchestrator = match &prefix {
                Some(prefix) => orchestrator.with_branch_prefix(prefix),
                None => orchestrator.without_branch_prefix(),
            };
            let orchestrator =
                apply_workspace_creation_prefs(orchestrator, &workspace_creation_prefs)?;

            // Validate the exact first branch candidate, prefix and task-id
            // grouping included, before a fetch or filesystem write. An
            // explicit Advanced/PR branch wins; otherwise the same task slug
            // and the same orchestrator method used by `create_with` spell it.
            let task = workspace_task_from_spec(&spec);
            let generated_branch = orchestrator.prefixed_branch(&task.task_title);
            let branch_to_create = branch.as_deref().unwrap_or(&generated_branch);
            check_branch_name(&host, &repo_root, branch_to_create)?;

            let reuse = reuse_branch.unwrap_or(false);
            let typed_base = base
                .as_deref()
                .map(str::trim)
                .filter(|one| !one.is_empty())
                .map(str::to_string);
            // A reused branch is checked out where it stands, so it has no base to
            // resolve and nothing to fetch — asking would be a round trip for an
            // answer git is about to ignore.
            let base = if reuse {
                None
            } else {
                resolve_create_base(&host, &repo_root, base.as_deref(), pinned.as_deref())
            };
            let mut local_base_ref_refresh = None;
            if let Some(base) = &base {
                say("fetching");
                refresh_base(&host, &repo_root, base);
                if refresh_local_base_ref {
                    local_base_ref_refresh = refresh_local_base_ref_for_worktree_create(
                        &host,
                        &orchestrator,
                        &repo_root,
                        base,
                    );
                }
            }

            say("creating");
            let worktree = orchestrator
                .create_with(
                    &task,
                    &zerocode_orchestrator::NewWorktree {
                        start_point: base.as_deref(),
                        branch: branch.as_deref(),
                        reuse_branch: reuse,
                    },
                )
                .map_err(|error| error.to_string())?;

            // What this branch is compared with, written where it survives this
            // window: git's own config for the branch. Orca keeps the fact twice
            // (`persistWorktreeCreationBase` writes the effective base into the
            // same config key, and the worktree's metadata takes
            // `compareBaseRef ?? remoteTracking ?? baseBranch`, @4440967) — we keep
            // it once, and the one worth keeping is the one a diff can use. For an
            // ordinary create the two are the same thing; for a pull request the
            // start point is a commit of its own head, and comparing a review
            // against the commit it started at would show only what the reviewer
            // has since typed.
            let recorded = compare_base
                .as_deref()
                .map(str::trim)
                .filter(|one| !one.is_empty())
                .or(base.as_deref());
            if !reuse && let (Some(branch), Some(base)) = (worktree.branch.as_deref(), recorded) {
                record_creation_base(&host, &worktree.path, branch, base);
            }
            // 업스트림을 먼저 적고, **적힌 뒤에만** 편의를 준다. 포크의 대상을
            // 적지 못했는데 `push.autoSetupRemote`가 켜지면 첫 push가 조용히
            // 엉뚱한 저장소로 간다 — 그 조합만 만들지 않는다.
            let wired = match (push_target.as_ref(), worktree.branch.as_deref()) {
                (Some(target), Some(branch)) => {
                    set_push_upstream(&host, &worktree.path, branch, target) || !target.fork
                }
                (Some(target), None) => !target.fork,
                (None, _) => true,
            };
            if wired {
                ensure_push_auto_setup_remote(&host, &worktree.path);
            }
            // A base somebody typed becomes this repository's default, once it has
            // actually produced a checkout. After the create rather than before,
            // so a name git refused is not remembered as a preference.
            if let Some(typed) = typed_base {
                let _ = update_project_settings(&settings_repository, &settings_key, |entry| {
                    entry.worktree_base_ref = Some(typed);
                });
            }

            // Share what the repository asked to share, before the setup
            // script runs — `npm ci` in a workspace whose `node_modules` is
            // already the primary's is the whole point, and doing it after
            // would install into a directory about to be replaced by a link.
            share_project_directories(&repo_root, &worktree.path);

            // Decide the exact trusted command on the blocking side, while the
            // repository and checkout facts are still together. Execution moves
            // to a visible PTY below; no script text ever crosses the webview.
            let setup = prepare_new_worktree_setup(
                &settings_repository,
                &config_root,
                &repo_root,
                &worktree.path,
                run_setup,
            )?;
            let worktree_path = worktree.path.clone();
            let mut entry = WorktreeEntry::new(worktree, &active_root);
            entry.local_base_ref_refresh = local_base_ref_refresh;
            // The row the dialog gets back says what it was cut from at once,
            // rather than after the next catalogue walk reads it back.
            if !reuse {
                entry.base = recorded.map(str::to_string);
            }
            Ok::<_, String>((entry, setup, repo_root, worktree_path))
        })
        .await
        .map_err(|join| join.to_string())??;

    if let Some(command) = setup {
        match spawn_setup_terminal(&state, &repo_root, &worktree_path, &command) {
            Ok(term) => {
                entry.setup_terminal = Some(SetupTerminal {
                    term,
                    launch_mode: setup_launch_mode,
                });
            }
            Err(error) => entry.setup_error = Some(error),
        }
    }
    Ok(entry)
}

#[tauri::command(async)]
pub(crate) fn worktree_prefs(state: State<'_, AppState>) -> WorktreePrefs {
    load_settings_for_boot(state.settings())
        .document
        .worktree_prefs
}

/// 접두사 설정을 바꾼다. 쓰기 전에 판정한다 — 저장된 뒤에 거절당하는 설정은
/// 사람이 고칠 수 없는 자리에서 실패하는 설정이다.
#[tauri::command(async)]
pub(crate) fn save_worktree_prefs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: String,
    custom: Option<String>,
) -> Result<SettingsSnapshot, String> {
    let branch_prefix = match mode.as_str() {
        "git-username" => BranchPrefixMode::GitUsername,
        "custom" => BranchPrefixMode::Custom,
        "none" => BranchPrefixMode::None,
        _ => return Err(format!("{mode}은(는) 아는 접두사 모드가 아닙니다")),
    };
    let custom = custom
        .map(|one| one.trim().to_string())
        .filter(|one| !one.is_empty());
    let prefs = WorktreePrefs {
        branch_prefix,
        custom_prefix: custom,
    };
    // 저장하기 전에 같은 판정기를 통과시킨다. 사용자 이름은 여기서 모르므로
    // `None`으로 물어도 되는 이유는 그 값이 `custom` 판정에 쓰이지 않기
    // 때문이다.
    if prefs.branch_prefix == BranchPrefixMode::Custom {
        resolve_branch_prefix(&prefs, None)?;
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WORKTREE_PREFS],
        move |settings| {
            settings.worktree_prefs = prefs;
            Ok(())
        },
    )
}

/// 창이 고급 칸에 적은 이름을 물어보는 문. 만들기와 **같은 함수**를 부른다.
#[tauri::command]
pub(crate) async fn validate_branch_name(
    state: State<'_, AppState>,
    project: Option<String>,
    name: String,
) -> Result<(), String> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        check_branch_name(&Host::for_workspace(&repo_root), &repo_root, &name)
    })
    .await
    .map_err(|join| join.to_string())?
}

/// 이 저장소의 로컬 브랜치들, 최근에 손댄 것부터.
///
/// 브랜치 탭이 읽는 목록이다. 상한이 있는 이유는 화면이 아니라 왕복이다 —
/// 브랜치 이천 개짜리 저장소에서 전부 실어 보내면 다이얼로그가 그 목록을
/// 기다린다.
#[tauri::command]
pub(crate) async fn list_branches(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<Vec<String>, String> {
    let orchestrator = match project {
        Some(path) => known_project_orchestrator(state.config_root(), &path)?,
        None => return Err(NOT_A_REPOSITORY.to_string()),
    };
    let repo_root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let host = Host::for_workspace(&repo_root);
        let said = host
            .vcs()
            .text(
                &repo_root,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)",
                    "--sort=-committerdate",
                    "--count=200",
                    "refs/heads",
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(said
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    })
    .await
    .map_err(|join| join.to_string())?
}

#[tauri::command]
pub(crate) fn work_item_seed(
    text: String,
    current: Option<String>,
    last_auto: Option<String>,
) -> WorkItemReport {
    let _crumb = crate::crumbs::Command::enter("work_item_seed");
    let mut seed = zerocode_core::seed_from_text(&text);
    if !seed.display_name.is_empty() {
        // The badge previews the same checkout/branch material create uses,
        // not the longer workspace-display slug from the classifier.
        seed.seed_name = workspace_title_slug(&seed.display_name);
    }
    WorkItemReport {
        apply_auto_name: zerocode_core::workitem::should_apply_auto_name(
            current.as_deref().unwrap_or_default(),
            last_auto.as_deref(),
        ),
        seed,
        // 시계에서 뽑는다. 목록 안의 어느 낱말이든 좋고, 두 번 연속 같은
        // 낱말이 나오지 않는 것으로 충분하다.
        fallback: zerocode_core::workitem::fallback_name(now_epoch_ms().unsigned_abs()),
    }
}

/// 이 저장소의 PR과 이슈 — GitHub 탭이 그리는 목록.
///
/// `filters`는 창이 고른 **값**이다(상태·초안). 한정자로 옮기는 손은 `gh.rs`
/// 하나뿐이고, 없는 것은 아무것도 좁히지 않는 것이므로 기본값이 곧 예전의 그
/// 질문이다 — 이 인자를 모르는 호출부는 그대로 답을 받는다.
#[tauri::command]
pub(crate) async fn github_work_items(
    state: State<'_, AppState>,
    project: Option<String>,
    projects: Option<Vec<String>>,
    preset: Option<String>,
    query: Option<String>,
    filters: Option<gh::WorkItemFilters>,
) -> Result<Vec<gh::WorkItem>, GhFailure> {
    // 콤보가 고른 집합이 오면 그 전부를, 아니면 지금까지의 프로젝트 하나를 —
    // 계약은 넓어지고 문은 하나다. 모르는 이름은 여기서 즉시 거절: 집합 속에
    // 섞인 낯선 경로가 조용히 빠지면, 사람은 고른 것을 봤다고 믿는다.
    let named = match projects {
        Some(listed) if !listed.is_empty() => listed,
        _ => vec![project.unwrap_or_default()],
    };
    let config_root = state.config_root().to_path_buf();
    let homes = tauri::async_runtime::spawn_blocking(move || {
        known_project_repositories(&config_root, &named)
    })
    .await
    .map_err(|e| GhFailure::ours(e.to_string()))?
    .map_err(GhFailure::ours)?;
    let filters = filters.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let preset = preset.as_deref().unwrap_or_default();
        let query = query.as_deref().unwrap_or_default();
        // 하나면 지금까지의 그 읽기(행이 프로젝트를 입에 올리지 않는다),
        // 여럿이면 나란한 읽기와 신선도 병합.
        if let [(_, root)] = homes.as_slice() {
            gh::fetch_work_items(root, preset, query, &filters).map_err(GhFailure::from)
        } else {
            gh::fetch_work_items_across(&homes, preset, query, &filters).map_err(GhFailure::from)
        }
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// 작성자·담당자 필터의 후보 벤치 — 이 저장소가 맡길 수 있는 사람들.
#[tauri::command]
pub(crate) async fn github_assignable_users(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<Vec<String>, GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        gh::fetch_assignable_users(&root).map_err(GhFailure::from)
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// 프리셋 칩이 검색칸에 써 넣을 문장.
///
/// 문법의 사본은 `gh.rs` 하나라는 규칙(`every_github_read_goes_through_one_door`)
/// 위에서 신판 실측의 UX — 칩 클릭이 검색 문자열을 갈아 끼운다 — 를 지키는
/// 유일한 길이다: 창은 문장을 **짓지** 않고 이 문으로 **받아** 보여 준다.
/// 프로세스를 띄우지 않으므로 blocking 풀에 갈 일도 없다.
#[tauri::command]
pub(crate) fn github_preset_query(
    kind: Option<String>,
    preset: Option<String>,
) -> Result<String, GhFailure> {
    let _crumb = crate::crumbs::Command::enter("github_preset_query");
    let kind = work_item_kind(kind.as_deref())?;
    let chip = preset
        .as_deref()
        .and_then(gh::GhPreset::from_word)
        .ok_or_else(|| GhFailure::ours("모르는 프리셋입니다".to_string()))?;
    gh::preset_query(kind, chip)
        .map(str::to_string)
        .ok_or_else(|| GhFailure::ours("이 종류에는 없는 프리셋입니다".to_string()))
}

/// 이 저장소의 웹 주소들 — 「새 이슈」·「GitHub에서 열기」가 나가는 문.
#[tauri::command]
pub(crate) async fn github_web_urls(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<gh::GithubWebUrls, GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || gh::web_urls(&root).map_err(GhFailure::from))
        .await
        .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// 열린 항목 하나 — 본문과 그 아래 목소리들. 목록과 같은 저장소 문맥에서 읽는다.
#[tauri::command]
pub(crate) async fn github_work_item_detail(
    state: State<'_, AppState>,
    project: Option<String>,
    kind: Option<String>,
    number: Option<u64>,
) -> Result<gh::WorkItemDetail, GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let kind = work_item_kind(kind.as_deref())?;
    let item = number.ok_or_else(|| GhFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let limits = checks_runtime::limits(&state);
    tauri::async_runtime::spawn_blocking(move || {
        gh::fetch_work_item_detail(&root, kind, item, &limits).map_err(GhFailure::from)
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// 항목에 한마디 — 본문은 stdin으로 gh에 건너고, argv에는 절대 실리지 않는다.
#[tauri::command]
pub(crate) async fn github_comment_work_item(
    state: State<'_, AppState>,
    project: Option<String>,
    kind: Option<String>,
    number: Option<u64>,
    body: Option<String>,
) -> Result<(), GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let kind = work_item_kind(kind.as_deref())?;
    let item = number.ok_or_else(|| GhFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let said = body.unwrap_or_default();
    if said.trim().is_empty() {
        return Err(GhFailure::ours("빈 코멘트는 보내지 않습니다".to_string()));
    }
    tauri::async_runtime::spawn_blocking(move || {
        gh::comment_work_item(&root, kind, item, &said).map_err(GhFailure::from)
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// 병합 — 방식 낱말은 enum 문을 지나야 argv가 된다. 게이트 판정은 창의
/// 사다리 몫이고, 최종 거절은 GitHub 자신의 문장이 그대로 온다.
#[tauri::command]
pub(crate) async fn github_merge_pr(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
    method: Option<String>,
) -> Result<(), GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GhFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let method =
        gh::PrMergeMethod::parse(method.as_deref().unwrap_or_default()).map_err(GhFailure::ours)?;
    tauri::async_runtime::spawn_blocking(move || {
        gh::merge_pr(&root, item, method).map_err(GhFailure::from)
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// 닫기/다시 열기 — 권한과 상태 검증은 GitHub 쪽 국경(gh)의 몫.
#[tauri::command]
pub(crate) async fn github_set_work_item_open(
    state: State<'_, AppState>,
    project: Option<String>,
    kind: Option<String>,
    number: Option<u64>,
    open: Option<bool>,
) -> Result<(), GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let kind = work_item_kind(kind.as_deref())?;
    let item = number.ok_or_else(|| GhFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let wanted = open.ok_or_else(|| GhFailure::ours("열림 여부가 없습니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        gh::set_work_item_open(&root, kind, item, wanted).map_err(GhFailure::from)
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// 이 저장소의 이슈와 MR — GitLab 탭이 그리는 목록.
///
/// 창이 보내는 것은 **고른 것**(보기와 칩)이고, 그것을 질문으로 옮기는 손은
/// `glab.rs` 하나다. 모르는 낱말은 추측 없이 거절한다 — 짐작한 목록은 칩이
/// 말하는 그 목록이 아니다.
#[tauri::command]
/// `query` is what somebody typed, for the surface that has a field. The tasks
/// page has none and sends nothing; the composer's picker sends what is in it.
/// It rides as GitLab's own `search`, encoded, rather than being filtered on
/// this side — a page of fifty filtered locally disagrees with what the chip
/// above it claims to be counting, which is the failure this file's own
/// comments already warn about.
///
/// `picker` says which surface is asking, and therefore how many rows. Two
/// surfaces, two answers, and that is the ORIGINAL's arrangement rather than
/// ours: the tasks page reads a page of fifty, the composer's picker reads
/// twelve (`RESULT_LIMIT`, `new-workspace/SmartWorkspaceNameField.tsx:159`).
pub(crate) async fn gitlab_work_items(
    state: State<'_, AppState>,
    project: Option<String>,
    view: Option<String>,
    filter: Option<String>,
    query: Option<String>,
    picker: Option<bool>,
) -> Result<Vec<glab::GlabItem>, GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let filter = filter.unwrap_or_default();
    let read: Box<dyn FnOnce() -> Result<Vec<glab::GlabItem>, GlabFailure> + Send> = match view
        .as_deref()
    {
        Some("issues") => {
            // 이슈 칩이 옮기는 것은 담당자다(실측) — 상태는 언제나 열림이고,
            // 칩이 그것까지 건드리면 한 이름표 아래 두 필터가 선다.
            let mine = match filter.as_str() {
                "opened" => false,
                "assigned-to-me" => true,
                _ => {
                    return Err(GlabFailure::ours(
                        "이슈 필터는 opened 또는 assigned-to-me입니다".to_string(),
                    ));
                }
            };
            Box::new(move || glab::fetch_issues(&root, mine).map_err(GlabFailure::from))
        }
        Some("mrs") => {
            let wanted = glab::MrState::from_word(&filter).ok_or_else(|| {
                GlabFailure::ours("MR 필터는 opened·merged·closed·all 중 하나입니다".to_string())
            })?;
            let rows = if picker.unwrap_or(false) {
                glab::PICKER_ROWS
            } else {
                glab::PAGE_ROWS
            };
            let typed = query.clone();
            Box::new(move || {
                glab::fetch_mrs(&root, wanted, rows, typed.as_deref()).map_err(GlabFailure::from)
            })
        }
        _ => {
            return Err(GlabFailure::ours(
                "작업 항목 보기는 issues 또는 mrs입니다".to_string(),
            ));
        }
    };
    tauri::async_runtime::spawn_blocking(read)
        .await
        .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 이 사람의 대기 중인 할 일. 저장소로 좁히지 않는다 — GitLab의 할 일은 볼 수
/// 있는 모든 프로젝트를 가로지르는 받은 편지함이고, 행이 프로젝트 이름을 함께
/// 그리는 것도 그래서다. 그래도 저장소 안에서 물어야 한다: 어느 인스턴스에
/// 물을지를 `glab`이 그 자리에서 고르기 때문이다.
#[tauri::command]
pub(crate) async fn gitlab_todos(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<Vec<glab::GlabTodo>, GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        glab::fetch_todos(&root).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 열린 항목 하나 — 머리와 본문, 그리고 그 아래 목소리들.
#[tauri::command]
pub(crate) async fn gitlab_item_detail(
    state: State<'_, AppState>,
    project: Option<String>,
    kind: Option<String>,
    number: Option<u64>,
) -> Result<glab::GlabItemDetail, GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let kind = gitlab_item_kind(kind.as_deref())?;
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        glab::fetch_item_detail(&root, kind, item).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 항목에 한마디 — 본문은 stdin으로 `glab`에 건너고, argv에는 절대 실리지 않는다
/// (GitHub 쪽의 그 규칙과 같은 이유: 프로세스 목록은 비밀 통로가 아니다).
#[tauri::command]
pub(crate) async fn gitlab_comment_item(
    state: State<'_, AppState>,
    project: Option<String>,
    kind: Option<String>,
    number: Option<u64>,
    body: Option<String>,
) -> Result<(), GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let kind = gitlab_item_kind(kind.as_deref())?;
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let said = body.unwrap_or_default();
    if said.trim().is_empty() {
        return Err(GlabFailure::ours("빈 코멘트는 보내지 않습니다".to_string()));
    }
    tauri::async_runtime::spawn_blocking(move || {
        glab::comment_on_item(&root, kind, item, &said).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// MR을 병합한다 — 방식은 창의 낱말이 아니라 열거로 건넌다.
#[tauri::command]
pub(crate) async fn gitlab_merge_mr(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
    method: Option<String>,
) -> Result<(), GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let picked = glab::MergeMethod::from_word(&method.unwrap_or_default())
        .ok_or_else(|| GlabFailure::ours("병합 방식은 merge, squash, rebase입니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        glab::merge_merge_request(&root, item, picked).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// MR의 제목·설명·라벨을 저장한다 — 온 필드만, 값은 stdin으로.
/// 실측(updateMR)은 한 호출에 argv `-f`로 전부 싣지만, 사람의 말이 argv에
/// 서지 않는 이 집의 규칙이 우선한다(필드마다 한 PUT — 기록된 이탈).
#[tauri::command]
pub(crate) async fn gitlab_update_mr(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
    title: Option<String>,
    body: Option<String>,
    add_labels: Option<String>,
    remove_labels: Option<String>,
) -> Result<(), GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    // 실측의 그 거절: 비운 제목은 저장이 아니라 실수다.
    if title.as_deref().is_some_and(|said| said.trim().is_empty()) {
        return Err(GlabFailure::ours("제목은 비울 수 없습니다".to_string()));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let fields = [
            (glab::MrField::Title, title),
            (glab::MrField::Description, body),
            (glab::MrField::AddLabels, add_labels),
            (glab::MrField::RemoveLabels, remove_labels),
        ];
        for (field, value) in fields {
            let Some(value) = value else { continue };
            glab::update_merge_request_field(&root, item, field, &value)
                .map_err(GlabFailure::from)?;
        }
        Ok(())
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 리뷰어 카드와 파일 탭의 늦은 한 번 읽기 — 승인과 바뀐 파일들.
/// 부분 거절은 빈 자리로 선다(실측의 그 관대함) — 카드 전체를 죽이지 않는다.
#[tauri::command]
pub(crate) async fn gitlab_mr_review(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
) -> Result<glab::GlabMrReview, GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || Ok(glab::fetch_mr_review(&root, item)))
        .await
        .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 리뷰어를 통째로 바꾼다 — id들만 넘고, 판정은 GitLab의 몫이다.
#[tauri::command]
pub(crate) async fn gitlab_set_mr_reviewers(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
    reviewers: Option<Vec<u64>>,
) -> Result<(), GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let ids = reviewers.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        glab::update_merge_request_reviewers(&root, item, &ids).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 리뷰어로 앉힐 수 있는 사람들 — 프로젝트 구성원 한 페이지.
#[tauri::command]
pub(crate) async fn gitlab_project_members(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<Vec<glab::GlabUser>, GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        glab::fetch_project_members(&root).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 바뀐 파일의 한 줄에 다는 댓글 — 자리는 argv(diff의 데이터), 말은 stdin.
#[tauri::command]
pub(crate) async fn gitlab_inline_comment(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
    place: Option<glab::MrInlinePlace>,
    body: Option<String>,
) -> Result<(), GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let place = place.ok_or_else(|| GlabFailure::ours("댓글을 달 자리가 없습니다".to_string()))?;
    let said = body.unwrap_or_default();
    if said.trim().is_empty() {
        return Err(GlabFailure::ours("댓글 내용이 없습니다".to_string()));
    }
    tauri::async_runtime::spawn_blocking(move || {
        glab::post_inline_comment(&root, item, &place, &said).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// MR 머리 파이프라인의 잡들 — 파이프라인 탭이 서는 이유다.
#[tauri::command]
pub(crate) async fn gitlab_pipeline_jobs(
    state: State<'_, AppState>,
    project: Option<String>,
    pipeline: Option<u64>,
) -> Result<Vec<glab::GlabJob>, GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let ridden =
        pipeline.ok_or_else(|| GlabFailure::ours("파이프라인 번호가 없습니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        glab::fetch_pipeline_jobs(&root, ridden).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 잡 하나를 다시 돌린다 — 어느 파이프라인의 것인지는 GitLab이 안다.
#[tauri::command]
pub(crate) async fn gitlab_retry_job(
    state: State<'_, AppState>,
    project: Option<String>,
    job: Option<u64>,
) -> Result<(), GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let one = job.ok_or_else(|| GlabFailure::ours("잡 번호가 없습니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        glab::retry_pipeline_job(&root, one).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 잡의 로그 전체 — 쓴 적 없는 로그는 빈 문자열이다(거절이 아니다).
#[tauri::command]
pub(crate) async fn gitlab_job_trace(
    state: State<'_, AppState>,
    project: Option<String>,
    job: Option<u64>,
) -> Result<String, GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let one = job.ok_or_else(|| GlabFailure::ours("잡 번호가 없습니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        glab::fetch_job_trace(&root, one).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

/// 닫기/다시 열기 — 권한과 상태 검증은 GitLab 쪽 국경(`glab`)의 몫이다.
#[tauri::command]
pub(crate) async fn gitlab_set_item_open(
    state: State<'_, AppState>,
    project: Option<String>,
    kind: Option<String>,
    number: Option<u64>,
    open: Option<bool>,
) -> Result<(), GlabFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GlabFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let kind = gitlab_item_kind(kind.as_deref())?;
    let item = number.ok_or_else(|| GlabFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let wanted = open.ok_or_else(|| GlabFailure::ours("열림 여부가 없습니다".to_string()))?;
    tauri::async_runtime::spawn_blocking(move || {
        glab::open_or_close_item(&root, kind, item, wanted).map_err(GlabFailure::from)
    })
    .await
    .map_err(|join| GlabFailure::ours(join.to_string()))?
}

#[tauri::command]
pub(crate) async fn resolve_pr_base(
    state: State<'_, AppState>,
    project: Option<String>,
    number: u64,
    head_ref: String,
    base_ref: Option<String>,
    cross_repo: Option<bool>,
) -> Result<PrStartPoint, String> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        // Orca의 `isValidReviewHeadNumber`와 같은 자리. 0번 PR은 없고, 없는
        // 번호로 fetch를 짓는 것은 원격에 아무 뜻도 없는 refspec을 보내는 것이다.
        if number == 0 {
            return Err("PR 번호가 없습니다".to_string());
        }
        let head = head_ref.trim();
        if head.is_empty() {
            return Err(format!("PR #{number}에 head 브랜치가 없습니다"));
        }
        let host = Host::for_workspace(&repo_root);
        let listed = host
            .vcs()
            .text(&repo_root, &["remote"])
            .map_err(|error| error.to_string())?;
        let base_remote = gh::pick_remote(&listed).map_err(|remotes| {
            if remotes.is_empty() {
                "이 저장소에는 원격이 없습니다".to_string()
            } else {
                format!(
                    "원격이 여럿이고({}) 기본이 정해져 있지 않습니다",
                    remotes.join(", ")
                )
            }
        })?;

        // 포크에서 온 PR은 head가 **다른 저장소**에 있다. 그것을 원격으로 더하는
        // 것이 가져오기의 전제이자, 무엇보다 사람이 고친 것을 되돌려 밀 곳이다.
        // 같은 저장소의 PR은 여기를 지나지 않는다 — 물어볼 것이 없다.
        let fork = if cross_repo.unwrap_or(false) {
            Some(resolve_fork_push_target(
                &host,
                &repo_root,
                &base_remote,
                number,
            )?)
        } else {
            None
        };
        // 가져올 원격과 브랜치는 포크가 있으면 포크의 것이다. **비교 기준은
        // 아니다** — PR의 base는 언제나 base 저장소의 브랜치다.
        let (remote, head) = match &fork {
            Some((remote, pushed)) => (remote.as_str(), pushed.branch.as_str()),
            None => (base_remote.as_str(), head),
        };

        // 가져오지 못한 것을 가져온 척하지 않는다.
        //
        // 이 자리는 오래 `let _ =`이었고, 그래서 실패한 fetch 뒤에 남아 있던
        // 로컬 tip을 **조용히** 내줬다 — 만료된 토큰이나 지워진 브랜치에서
        // 어제의 커밋을 잘라 주는 길이다. Orca는 실패를 셋으로 갈라 읽는다
        // (`pr-start-point.ts:199-226`, `fetch-error-classification.ts`):
        //
        //   1. **원격에 그 ref가 없다** → 이 PR의 head는 그 저장소에 없다. 포크는
        //      위에서 이미 자기 원격을 얻었으므로 여기까지 오는 것은 지워진
        //      브랜치이거나 지워진 포크다 — 그리고 Orca가 폴백하는
        //      `refs/pull/<n>/head`는 커밋을 주지만 **밀 곳을 주지 않는다**.
        //      사실을 말하고 멈춘다([`PR_HEAD_NOT_ON_REMOTE`]).
        //   2. **전송이 죽었다** → 이미 가진 tip이 이 원격이 마지막으로 말한
        //      자리다. 그대로 쓴다.
        //   3. **그 밖 전부** → 답이 아니다. 목록은 허용 목록이라 처음 보는
        //      실패는 여기로 떨어진다.
        let fetched = host.vcs().try_within(
            &repo_root,
            &["fetch", remote, &gh::head_refspec(remote, head)],
            gh::HEAD_FETCH_BUDGET,
        );
        if let Within::Refused(said) = &fetched {
            match fetch_refusal::classify(said) {
                fetch_refusal::FetchRefusal::MissingRef => {
                    return Err(format!("{PR_HEAD_NOT_ON_REMOTE} ({remote}/{head})"));
                }
                fetch_refusal::FetchRefusal::Fatal => {
                    return Err(format!(
                        "{remote}/{head}을(를) 가져오지 못했습니다: {}",
                        fetch_refusal::first_line(said)
                    ));
                }
                fetch_refusal::FetchRefusal::Transient => {}
            }
        }
        let tracking = gh::tracking_ref(remote, head);
        let start_point = host
            .vcs()
            .text(
                &repo_root,
                &["rev-parse", "--verify", &format!("{tracking}^{{commit}}")],
            )
            .map(|said| said.trim().to_string())
            .ok()
            .filter(|sha| !sha.is_empty())
            .ok_or_else(|| {
                // 가져온 뒤에도 없는 것과, 가져오지도 못한 데다 남은 것도 없는
                // 것은 사람이 할 일이 다르다.
                if matches!(fetched, Within::Said(_)) {
                    format!("가져온 뒤에도 {remote}/{head}이(가) 없습니다")
                } else {
                    format!(
                        "{remote}/{head}을(를) 가져오지 못했고, 이 저장소에 남아 있는 것도 없습니다"
                    )
                }
            })?;

        // 비교 기준은 **가져와졌을 때만** 말한다. Orca의
        // `fetchCompareBaseRefWithLocalFallback`이 같은 규칙이다 — 없는 ref를
        // 비교 기준으로 적어 두면 나중에 diff가 거절당한다.
        //
        // 여기는 실패의 이유를 묻지 않는다(그래서 `text_within`이다). 이유가
        // 무엇이든 다음 수가 하나이기 때문이다 — 로컬에 그 ref가 있으면 쓰고,
        // 없으면 비교 기준 없이 간다. head 쪽과 갈리는 지점이 정확히 이것이다.
        let compare_base = base_ref
            .as_deref()
            .map(str::trim)
            .filter(|one| !one.is_empty())
            .and_then(|base| {
                let _ = host.vcs().text_within(
                    &repo_root,
                    &["fetch", &base_remote, &gh::head_refspec(&base_remote, base)],
                    gh::HEAD_FETCH_BUDGET,
                );
                let tracking = gh::tracking_ref(&base_remote, base);
                ref_exists(&host, &repo_root, &tracking).then_some(tracking)
            });
        Ok(PrStartPoint {
            start_point,
            branch: head.to_string(),
            compare_base,
            // 가져온 자리가 곧 밀 자리다. 같은 저장소의 PR에도 싣는 것은 Orca와
            // 같다(`pr-start-point.ts:246`) — 업스트림이 적혀 있어야 첫 화면의
            // ahead/behind가 맞고, 사람이 아무것도 고르지 않아도 push가 PR의
            // 브랜치로 간다.
            push_remote: remote.to_string(),
            push_branch: head.to_string(),
            push_fork: fork.is_some(),
            maintainer_can_modify: fork
                .as_ref()
                .and_then(|(_, pushed)| pushed.maintainer_can_modify),
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// 고른 merge request가 어디서 잘리는가.
///
/// [`resolve_pr_base`]의 GitLab 짝이고, 같은 저장소에서 온 MR만 답한다. 두
/// 도로가 나뉘어 있는 이유는 프로바이더가 둘이어서가 아니라 **포크의 head를
/// 푸는 방법이 서로 다르기** 때문이다(GitHub은 포크를 원격으로 더하고, GitLab은
/// 특수 ref에서 가져온다). 나뉜 자리가 정확히 그 하나이므로, 같은 저장소의
/// 갈래는 [`PrStartPoint`]를 **그대로** 돌려준다 — 필드 여섯이 전부 진짜 값이고
/// 하나도 지어내지 않는다:
///
///   * `push_remote`/`push_branch` — base 원격과 MR의 source 브랜치. 같은
///     저장소이므로 가져온 자리가 곧 밀 자리다.
///   * `push_fork` — `false`. 위에서 포크를 거절했다.
///   * `maintainer_can_modify` — `None`. GitHub의 "Allow edits from
///     maintainers"에 대응하는 것이 GitLab MR에 없다.
///
/// 그 구조체가 `Pr`이라는 이름을 달고 있는 것은 남은 흠이고, 고치는 방법은
/// 이름 하나를 바꾸는 기계적인 조각이지 이 조각이 아니다.
///
/// **원본과 갈리는 자리 하나, 의도한 것**: 원본의 같은-저장소 MR은
/// `baseBranch`로 `<remote>/<source_branch>`라는 **이름**을 돌려준다
/// (`orca-runtime.ts:26510-26521`). 우리는 [`resolve_pr_base`]와 같이 sha로
/// 내린다 — 이름으로 넘기면 만들기와 fetch 사이에 누가 force-push한 순간 다른
/// 커밋이 잘린다. 그 이유는 프로바이더와 무관하다.
#[tauri::command]
pub(crate) async fn resolve_mr_base(
    state: State<'_, AppState>,
    project: Option<String>,
    number: u64,
    source_branch: String,
    target_branch: Option<String>,
    cross_repo: Option<bool>,
) -> Result<PrStartPoint, String> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        // `iid`가 0인 MR은 없다. GitHub 쪽과 같은 자리, 같은 이유.
        if number == 0 {
            return Err("MR 번호가 없습니다".to_string());
        }
        let head = source_branch.trim();
        if head.is_empty() {
            return Err(format!("MR !{number}에 source 브랜치가 없습니다"));
        }
        // 포크는 먼저 거절한다 — 원격을 고르기도 전에. 아래의 어떤 단계도
        // 포크의 head에는 닿지 못하고, 닿지 못한 채로 끝까지 가면 **base
        // 브랜치의 tip**을 시작점으로 내주게 된다.
        if cross_repo.unwrap_or(false) {
            return Err(MR_FROM_A_FORK.to_string());
        }
        let host = Host::for_workspace(&repo_root);
        let listed = host
            .vcs()
            .text(&repo_root, &["remote"])
            .map_err(|error| error.to_string())?;
        let remote = gh::pick_remote(&listed).map_err(|remotes| {
            if remotes.is_empty() {
                "이 저장소에는 원격이 없습니다".to_string()
            } else {
                format!(
                    "원격이 여럿이고({}) 기본이 정해져 있지 않습니다",
                    remotes.join(", ")
                )
            }
        })?;

        // 실패한 fetch 뒤에 남아 있던 로컬 tip을 조용히 내주지 않는다 — 셋으로
        // 갈라 읽는 그 판정은 프로바이더의 것이 아니라 **git의 것**이라
        // [`fetch_refusal`]을 그대로 쓴다.
        let fetched = host.vcs().try_within(
            &repo_root,
            &["fetch", &remote, &gh::head_refspec(&remote, head)],
            gh::HEAD_FETCH_BUDGET,
        );
        if let Within::Refused(said) = &fetched {
            match fetch_refusal::classify(said) {
                fetch_refusal::FetchRefusal::MissingRef => {
                    return Err(format!(
                        "이 원격에 그 MR의 source 브랜치가 없습니다 ({remote}/{head})"
                    ));
                }
                fetch_refusal::FetchRefusal::Fatal => {
                    return Err(format!(
                        "{remote}/{head}을(를) 가져오지 못했습니다: {}",
                        fetch_refusal::first_line(said)
                    ));
                }
                fetch_refusal::FetchRefusal::Transient => {}
            }
        }
        let tracking = gh::tracking_ref(&remote, head);
        let start_point = host
            .vcs()
            .text(
                &repo_root,
                &["rev-parse", "--verify", &format!("{tracking}^{{commit}}")],
            )
            .map(|said| said.trim().to_string())
            .ok()
            .filter(|sha| !sha.is_empty())
            .ok_or_else(|| {
                if matches!(fetched, Within::Said(_)) {
                    format!("가져온 뒤에도 {remote}/{head}이(가) 없습니다")
                } else {
                    format!(
                        "{remote}/{head}을(를) 가져오지 못했고, 이 저장소에 남아 있는 것도 없습니다"
                    )
                }
            })?;

        // 비교 기준은 가져와졌을 때만. 실패의 이유를 묻지 않는 것도 GitHub
        // 쪽과 같다 — 이유가 무엇이든 다음 수가 하나다.
        let compare_base = target_branch
            .as_deref()
            .map(str::trim)
            .filter(|one| !one.is_empty())
            .and_then(|base| {
                let _ = host.vcs().text_within(
                    &repo_root,
                    &["fetch", &remote, &gh::head_refspec(&remote, base)],
                    gh::HEAD_FETCH_BUDGET,
                );
                let tracking = gh::tracking_ref(&remote, base);
                ref_exists(&host, &repo_root, &tracking).then_some(tracking)
            });
        Ok(PrStartPoint {
            start_point,
            branch: head.to_string(),
            compare_base,
            push_remote: remote.clone(),
            push_branch: head.to_string(),
            push_fork: false,
            maintainer_can_modify: None,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

#[tauri::command]
pub(crate) async fn worktree_loss(
    state: State<'_, AppState>,
    path: String,
) -> Result<PendingLossReport, String> {
    let (orchestrator, chosen) = known_worktree_context(state.config_root(), &path)?;
    let loss = orchestrator
        .pending_loss(&chosen.path)
        .map_err(|error| error.to_string())?;
    Ok(PendingLossReport {
        uncommitted: loss.uncommitted_display(),
        ignored: loss.ignored_display(),
        fingerprint: loss.fingerprint(),
    })
}

/// Remove a worktree the user has confirmed.
///
/// `discard_changes` is the caller's promise, spelled on the wire: it is the
/// difference between "they were shown what would be lost and chose it" and
/// "refuse if anything would be". There is no third value, because there is
/// no path here that removes a worktree nobody agreed to (F7).
#[tauri::command]
pub(crate) async fn remove_worktree(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    path: String,
    discard_changes: bool,
) -> Result<(), String> {
    let (orchestrator, chosen) = known_worktree_context(state.config_root(), &path)?;
    if chosen.locked {
        let reason = format!(
            "worktree cleanup skipped for {}: git reports that the worktree is locked",
            chosen.path.display()
        );
        note_window_event(state.local_data_root(), &reason);
        return Err(reason);
    }
    // And the terminals this window is holding INSIDE it.
    //
    // This is the door. The webview has four `invoke("remove_worktree")` and
    // three user roads through them — a row's quick delete, its dialog, the
    // inactive-cleanup batch and the space batch — and all of them arrive
    // here. Guarding the scan rows alone would leave a stale row, a batch
    // confirmed a minute ago, and a direct invoke each able to walk past it;
    // guarding here covers all four at once, and the rows stay a display of
    // the same answer rather than the thing that enforces it.
    //
    // Why a terminal is a refusal and not a warning: the agent inside it goes
    // on writing to a directory that is not there, its report lands nowhere,
    // and nothing in the window ever says which of the removal roads did it.
    // That is the shape of the incident this gate was written for.
    //
    // And why this cannot become "never deletes": the answer is this
    // window's own live state, not a record on disk. Closing the pane clears
    // it this second, and the message says so. Checkouts nobody can remove
    // fill the disk, and a full disk takes the ledger runtime down with it —
    // a refusal with no way out would be trading today's bug for that one.
    let held = checkout_occupancy(&state, &chosen.path);
    if held > 0 {
        let reason = format!(
            "worktree removal refused for {}: this window is still running {held} \
             terminal(s) in it — close them and try again",
            chosen.path.display()
        );
        note_window_event(state.local_data_root(), &reason);
        return Err(reason);
    }
    let repo_root = orchestrator.repo_root().to_path_buf();
    let fallback = orchestrator.clone();
    let archive_root = repo_root.clone();
    let settings_repository = Arc::clone(state.settings());
    let trust_root = state.config_root().to_path_buf();
    let history_root = state.local_data_root().to_path_buf();
    let removed = tauri::async_runtime::spawn_blocking(move || {
        // BEFORE the removal, not after: the archive script exists to take
        // down what this checkout brought up — a compose stack, a tunnel, a
        // database — and it needs the directory it is talking about to still
        // be there. Running it afterwards would hand it a path that is gone.
        //
        // Its outcome does not gate the removal. A person who confirmed a
        // removal is owed the removal; a failing teardown must not leave them
        // with a workspace they asked to be rid of and no way to say so twice.
        let _ = archive_worktree(
            &settings_repository,
            &trust_root,
            &archive_root,
            &chosen.path,
        );
        // And the shared links come away before git looks, for a reason that
        // only shows up once somebody uses the feature: a symlink into the
        // primary's `node_modules` is untracked as far as git is concerned, so
        // leaving it makes EVERY ordinary delete fail with "use force". After
        // the archive script, which may still want what the links reach.
        unshare_project_directories(&archive_root, &chosen.path);
        let removal = if discard_changes {
            Removal::ConfirmedDiscardingChanges
        } else {
            Removal::ConfirmedIfClean
        };
        orchestrator
            .remove(&chosen.path, removal)
            .map_err(|error| error.to_string())?;
        // After the removal and not before: history is the one thing a person
        // who lands back on a refused removal still wants.
        forget_worktree_history(&history_root, &chosen.path.to_string_lossy());
        note_worktree_removal(
            &history_root,
            "window-remove",
            &chosen.path,
            if discard_changes {
                "somebody confirmed it, discarding what would be lost"
            } else {
                "somebody confirmed it and git found nothing to lose"
            },
        );
        Ok::<PathBuf, String>(chosen.path)
    })
    .await
    .map_err(|join| join.to_string())??;

    // The window cannot go on looking at a directory that is gone. Falling
    // back here rather than in the webview keeps the invariant with the value
    // it protects: every file command reads this, and one of them running
    // between the removal and a repaint would be resolving against nothing.
    if state.active_root() == removed {
        state.set_active_context(repo_root, Some(fallback));
    }
    // The path can never return as the same workspace after an explicit
    // removal, so its board-only metadata has no owner. Best effort because
    // the destructive operation already succeeded: a settings write failure
    // must not report that the checkout itself is still present.
    let removed_key = removed.to_string_lossy().into_owned();
    if let Ok(snapshot) = mutate_settings(state.settings(), move |settings| {
        settings.workspace_board.cards.remove(&removed_key);
        Ok(())
    }) {
        emit_settings_changed(
            &app,
            webview.label(),
            snapshot.revision,
            &[setting_key::WORKSPACE_BOARD],
        );
    }
    Ok(())
}

/// Point the file surfaces at one workspace already present in the catalog.
///
/// A git workspace is resolved from git's worktree list. A folder workspace is
/// resolved by exact canonical match against the persisted project catalog.
/// Both paths therefore keep the webview from selecting an arbitrary directory.
#[tauri::command(async)]
pub(crate) fn set_active_worktree(
    state: State<'_, AppState>,
    path: String,
) -> Result<Option<String>, String> {
    // The catalog that drew the row already knows which repository owns it.
    // Asking git again here — open, then list, for every stored project until
    // one matched — was two subprocess starts per project (~25ms each on this
    // machine) on the MAIN thread, on every workspace click: the stage could
    // not swap and the webview could not paint until they returned, which is
    // what a click that "lags" is made of. The last catalog is a backend-made
    // allowlist, so a path it names is one this window may make a cwd; the
    // directory is looked at because that listing can be a click old, and a
    // path it does not name still walks the git road below.
    let remembered = state.catalog_owners().get(&path).cloned();
    if let Some((orchestrator, chosen)) = remembered
        && !chosen.prunable
        && chosen.path.is_dir()
    {
        let branch = chosen.branch.clone();
        note_project(
            state.settings(),
            state.config_root(),
            orchestrator.repo_root(),
        );
        state.set_active_context(chosen.path, Some(orchestrator));
        return Ok(branch);
    }
    match known_workspace_context(state.config_root(), &path)? {
        KnownWorkspace::Git(orchestrator, chosen) => {
            // git still lists a worktree whose directory somebody deleted by
            // hand. Pointing the file surfaces at one would answer every
            // command with the same io error instead of showing anything.
            if chosen.prunable {
                return Err("이 워크트리의 디렉터리가 사라졌습니다".to_string());
            }
            let branch = chosen.branch.clone();
            note_project(
                state.settings(),
                state.config_root(),
                orchestrator.repo_root(),
            );
            state.set_active_context(chosen.path, Some(orchestrator));
            Ok(branch)
        }
        KnownWorkspace::Folder(folder) => {
            note_project(state.settings(), state.config_root(), &folder);
            state.set_active_context(folder, None);
            Ok(None)
        }
    }
}

/// What one workspace can prove about itself: its changes, the executions
/// that ran in it, the test receipts stored against it and the decisions
/// somebody recorded.
///
/// The path is resolved the same way [`set_active_worktree`] resolves a click
/// — the catalog owners this window built first, git's own worktree list for
/// the stored projects second — so a webview cannot point this at an
/// arbitrary directory. A `host` names a checkout that lives on another
/// machine; it is reported as unsupported rather than looked for down here,
/// because the same path can exist on this disk and belong to something else.
///
/// Async because the git half of the read forks `status` and `hash-object`
/// over a whole checkout, and those must not be seconds the window cannot
/// paint in. Nothing is executed, created or written: the verifier's receipts
/// are read as they were left, the authority store is opened read-only and
/// never made, and no GitHub request is sent (`ci` comes back unsupported).
#[tauri::command(async)]
pub(crate) fn worktree_evidence(
    state: State<'_, AppState>,
    path: String,
    host: Option<String>,
) -> Result<
    zerocode_orchestrator::worktree_evidence::WorktreeEvidenceV1,
    worktree_evidence_runtime::EvidenceRefusal,
> {
    let _crumb = crate::crumbs::Command::enter("worktree_evidence");
    let now_ms = now_epoch_ms();
    if let Some(host) = host
        .as_deref()
        .map(str::trim)
        .filter(|held| !held.is_empty())
    {
        return Ok(worktree_evidence_runtime::for_remote(
            &format!("{host}:{path}"),
            now_ms,
        ));
    }
    let remembered = state
        .catalog_owners()
        .get(&path)
        .map(|(_, chosen)| chosen.path.clone());
    if let Some(chosen) = remembered.filter(|held| held.is_dir()) {
        return Ok(worktree_evidence_runtime::for_workspace(&chosen, now_ms));
    }
    match known_workspace_context(state.config_root(), &path) {
        Ok(KnownWorkspace::Git(_, chosen)) => Ok(worktree_evidence_runtime::for_workspace(
            &chosen.path,
            now_ms,
        )),
        // A folder workspace is catalogued and is not a repository. It is read
        // like any other local checkout; git simply has nothing to say, and
        // the snapshot section says so rather than pretending it is clean.
        Ok(KnownWorkspace::Folder(folder)) => {
            Ok(worktree_evidence_runtime::for_workspace(&folder, now_ms))
        }
        Err(_) => Err(worktree_evidence_runtime::EvidenceRefusal::not_catalogued()),
    }
}

/// Merge a worktree's branch into the base repository and remove the worktree (Squash & Delete).
#[tauri::command]
pub(crate) async fn merge_and_remove_worktree(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    path: String,
    target_branch: Option<String>,
    commit_message: Option<String>,
) -> Result<String, String> {
    let (orchestrator, chosen) = known_worktree_context(state.config_root(), &path)?;
    let branch = chosen
        .branch
        .clone()
        .ok_or_else(|| "워크트리의 브랜치를 찾을 수 없습니다".to_string())?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    let target = target_branch.unwrap_or_else(|| "main".to_string());

    // 1. 메인 저장소에서 squash 병합 실행
    let branch_clone = branch.clone();
    let repo_clone = repo_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let output = crate::proc::quiet_command(zerocode_orchestrator::GIT_EXECUTABLE)
            .current_dir(&repo_clone)
            .args(["merge", "--squash", &branch_clone])
            .output()
            .map_err(|e| format!("git merge 실행 실패: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("스쿼시 병합 실패: {stderr}"));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("병합 작업 스레드 오류: {e}"))??;

    // 2. 커밋 생성
    let default_msg = format!("merge(auto): {branch} 워크트리 변경 사항 병합");
    let msg = commit_message.unwrap_or(default_msg);
    let repo_clone2 = repo_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let output = crate::proc::quiet_command(zerocode_orchestrator::GIT_EXECUTABLE)
            .current_dir(&repo_clone2)
            .args(["commit", "-m", &msg])
            .output()
            .map_err(|e| format!("git commit 실행 실패: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("커밋 실패: {stderr}"));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("커밋 작업 스레드 오류: {e}"))??;

    // 3. 워크트리 제거
    remove_worktree(app, webview, state, path, true).await?;

    Ok(format!("{target}에 {branch} 병합 및 워크트리 정리 완료"))
}
