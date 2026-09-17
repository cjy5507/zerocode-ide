//! Project commands.

use crate::*;

/// Take a project off the sidebar — the LIST, never the disk.
///
/// Orca's own semantics (`repos:remove` → `store.removeProject`,
/// out/main/index.js:158537): the row disappears and the checkout stays
/// exactly where it was. The active project is refused — the window is
/// standing in it, and `project_catalog` would put it straight back anyway.
#[tauri::command(async)]
pub(crate) fn remove_project(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let here = state.active();
    let active_project = here
        .orchestrator
        .map_or_else(|| here.root.clone(), |open| open.repo_root().to_path_buf());
    let canonical = PathBuf::from(&path)
        .canonicalize()
        .unwrap_or(PathBuf::from(&path));
    if canonical == active_project {
        return Err("보고 있는 프로젝트는 목록에서 뺄 수 없습니다".to_string());
    }
    // The approval its scripts were given leaves with it. Anything else would
    // keep a stranger's repository trusted after the person had taken it off
    // the list, and a path they add again a year later is not the repository
    // they approved.
    forget_repo_trust(state.config_root(), &canonical);
    let file = recent_projects_file(state.config_root());
    let mut held = stored_projects(state.config_root());
    held.retain(|stored| {
        *stored != path
            && PathBuf::from(stored)
                .canonicalize()
                .map(|resolved| resolved != canonical)
                .unwrap_or(true)
    });
    let text = serde_json::to_string(&held).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// Make the given order the stored one — the sidebar's header drag.
///
/// 기본(catalog) order IS Orca's Manual, and this is the write that makes it
/// manual: the window sends every path it lists, in the order the person
/// arranged. Stored rows the window did not name keep their old relative
/// order BEHIND the named ones — a hidden repository must not lose its seat
/// because it was not on screen — and a named path the store never held (the
/// active project rides the catalog by appendix) is written in at its seat.
/// Deduped by the same canonical equality `remove_project` trusts.
#[tauri::command(async)]
pub(crate) fn reorder_projects(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(), String> {
    let canonical = |path: &str| {
        PathBuf::from(path)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(path))
    };
    let held = stored_projects(state.config_root());
    let mut next: Vec<String> = Vec::new();
    let mut seated: Vec<PathBuf> = Vec::new();
    for path in paths
        .iter()
        .map(String::as_str)
        .chain(held.iter().map(String::as_str))
    {
        let seat = canonical(path);
        if seated.contains(&seat) {
            continue;
        }
        seated.push(seat);
        next.push(path.to_string());
    }
    let file = recent_projects_file(state.config_root());
    let text = serde_json::to_string(&next).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// Every repository and every one of its worktrees, in one stable snapshot.
#[tauri::command]
pub(crate) async fn project_catalog(
    state: State<'_, AppState>,
) -> Result<Vec<ProjectEntry>, String> {
    let here = state.active();
    let active_root = here.root;
    let active_project = here.orchestrator.map_or_else(
        || active_root.clone(),
        |open| open.repo_root().to_path_buf(),
    );
    let mut paths = stored_projects(state.config_root());
    paths.push(active_project.to_string_lossy().into_owned());

    // Read once, not once per repository: the marks all live in one small file
    // and a window with eleven projects would otherwise read and parse it
    // eleven times on every refresh.
    let marks = stored_repo_marks(state.settings())?;
    // And each repository's answer about worktrees this window did not make,
    // read the same way for the same reason.
    let visibility = stored_external_visibility(state.settings())?;
    // And the layouts workspaces have ever been made under, for the same
    // once-not-per-repository reason.
    let creation_prefs = load_settings_resilient(state.settings())
        .document
        .workspace_creation_prefs;
    let linked_items = catalog_work_item_links(state.settings());
    // One repository's questions do not depend on another's. Opening resolves
    // the checkout and shared git directory together, listing is the only
    // second git process, and creation bases come from the shared config file
    // the open already located. They are asked side by side below and read
    // back in the catalog's own order, so the dedupe by shared root still keeps
    // the first-listed block.
    type Owner = (String, Orchestrator, Worktree);
    let gather = |path: &String| -> Option<(PathBuf, Vec<Owner>, ProjectEntry)> {
        let Ok(canonical) = PathBuf::from(path).canonicalize() else {
            return None;
        };
        let orchestrator = Orchestrator::open(&canonical).ok();
        let root = orchestrator
            .as_ref()
            .map_or_else(|| canonical.clone(), |open| open.repo_root().to_path_buf());
        let creation_bases = orchestrator.as_ref().map(Orchestrator::creation_bases);
        // 워크트리를 프로젝트로 열어도 같은 저장소는 한 블록이다(1-g42) —
        // toplevel이 아니라 모두가 공유하는 `.git`의 자리가 저장소의 이름이다.
        // git이 답하지 못하면 물었던 자리가 그대로 열쇠다: 폴더 프로젝트는
        // 저마다 한 블록이 맞다.
        let identity = orchestrator
            .as_ref()
            .and_then(|open| open.shared_root().ok())
            .unwrap_or_else(|| root.clone());
        // Whether a real listing happened. `unwrap_or_default` used to swallow
        // the difference, and the difference is what the counts stand on: a
        // repository git refused to list has NOT been found to hold zero
        // external worktrees, and a dialog that says "0 available to import"
        // about it is inventing a number.
        let listing = orchestrator.as_ref().map(Orchestrator::list);
        let authoritative = matches!(listing, Some(Ok(_)));
        // Who owns which checkout, remembered for `set_active_worktree`: the
        // row a person clicks was drawn from this very answer, so the click
        // need not ask git the same question again.
        let mut owners: Vec<Owner> = Vec::new();
        let mut worktrees = match listing {
            Some(Ok(listed)) => listed
                .into_iter()
                .map(|worktree| {
                    if let Some(open) = orchestrator.as_ref() {
                        owners.push((
                            worktree.path.to_string_lossy().into_owned(),
                            open.clone(),
                            worktree.clone(),
                        ));
                    }
                    WorktreeEntry::new(worktree, &active_root)
                })
                .collect(),
            Some(Err(_)) => Vec::new(),
            None => vec![WorktreeEntry::folder(&root, &active_root)],
        };
        if let Some(bases) = creation_bases.as_ref() {
            attach_creation_bases_from(&mut worktrees, bases);
        }
        // 블록의 이름은 저장소 본체(main 워크트리)의 것이다. 카탈로그에 먼저
        // 적힌 자리가 이름을 정하면, 워크트리를 프로젝트로 연 적이 있는
        // 저장소가 본체를 열어도 그 워크트리의 이름을 입는다 (라이브 보고
        // "프로젝트명이 snapshot이야?").
        let name = worktrees
            .iter()
            .find(|worktree| worktree.is_main)
            .and_then(|main| {
                Path::new(&main.path)
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                root.file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or_else(|| root.to_str().unwrap_or("project"))
                    .to_string()
            });
        let listed = root.to_string_lossy().into_owned();
        let (stored, added_at) = visibility.get(&listed).cloned().unwrap_or_default();
        let ours = our_worktree_marks(orchestrator.as_ref(), &creation_prefs);
        let external = decide_external_visibility(
            &mut worktrees,
            &listed,
            ours_of(&ours),
            &stored,
            added_at,
            authoritative,
        );
        // 마지막으로 살아 있었던 시각. 정리 화면이 이미 읽는 그 다섯 자국을
        // 같은 함수에 물어 온다 — 두 화면이 같은 사실을 두 규칙으로 재면 하나가
        // 언젠가 다른 답을 한다.
        //
        // `stored`가 `None`인 것은 의도다: 정리 화면은 자동화 실행 장부까지
        // 얹어 읽지만 그건 카탈로그 한 번에 파일 읽기 한 번이고, 이 함수는
        // 워크스페이스 클릭마다 도는 길이다. 자동화가 체크아웃을 건드리면 위
        // 다섯 자국이 이미 함께 움직인다.
        let host = Host::for_workspace(&root);
        for entry in &mut worktrees {
            entry.last_activity_ms = zerocode_core::workspace_cleanup::last_activity(
                None,
                &worktree_activity_marks(&host, Path::new(&entry.path)),
            );
            decorate_worktree_link(entry, &linked_items);
        }
        Some((
            identity,
            owners,
            ProjectEntry {
                mark_color: marks.get(&listed).cloned(),
                slug: project_slug(&root),
                path: listed,
                name,
                worktrees,
                external,
            },
        ))
    };
    let gathered: Vec<Option<(PathBuf, Vec<Owner>, ProjectEntry)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .iter()
            .map(|path| scope.spawn(|| gather(path)))
            .collect();
        // A repository whose walk panicked is a repository with no answer,
        // not a catalog with none.
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or(None))
            .collect()
    });
    let mut seen = HashSet::new();
    let mut projects = Vec::new();
    let mut owners = HashMap::new();
    for (identity, owned, entry) in gathered.into_iter().flatten() {
        if !seen.insert(identity) {
            continue;
        }
        for (path, open, worktree) in owned {
            owners.insert(path, (open, worktree));
        }
        projects.push(entry);
    }
    *state.catalog_owners() = owners;
    Ok(projects)
}

/// Move this window to another project.
///
/// A switch inside this window, not a new process — which is what Orca does,
/// and what a person opening a folder expects. It used to start another copy
/// of the app.
///
/// And a switch is ALL it is. Orca's own is one pointer write —
/// `setActiveRepo: (projectId) => set({ activeRepoId: projectId })`
/// (repos.ts:3960) — and adding a project is a list append that never touches
/// a terminal (`addRepoPath`, repos.ts:3175). The only door its ptys die
/// through is an explicit `shutdown` (local-pty-provider.ts:1239), reached by
/// closing a pane or REMOVING a project — removal kills that repo's tabs and
/// no other's (`killedTabIds`, repos.ts:3755). So this function moves what
/// the window is looking at and nothing else; the shells of the project just
/// left keep running behind `paneTabs`' worktree filter, exactly as they do
/// across a worktree switch, and come back through
/// `restoreActiveWorktreeTab` when the window returns. Killing them here is
/// how "새로운 프로젝트를 열면 기존에 돌고 있던 claude가 꺼짐" was reported.
///
/// The window's session server still stays keyed to the root it was started
/// on: both its address and its token are derived from that root
/// ([`zerocode_lane::default_bind_for`], [`zerocode_lane::project_token`]),
/// and every lane already attached is talking to it.
#[tauri::command(async)]
pub(crate) fn open_project(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let target = PathBuf::from(&path)
        .canonicalize()
        .map_err(|error| format!("{path}: {error}"))?;
    if !target.is_dir() {
        return Err("폴더가 아닙니다".to_string());
    }
    // This window moves, rather than a second one appearing. Opening a folder
    // used to spawn another copy of the app, on the argument that a window is
    // keyed to one project root; Orca switches in place and lists projects in
    // its sidebar, and being asked for a folder and getting a whole new window
    // is not what anybody means by "open".
    //
    // The terminal pool is deliberately not touched. This used to empty it —
    // on the argument that a window naming one project must not hold shells
    // typing into another — and every agent the person had running died with
    // the switch. That argument expired when the tab strip learned to filter
    // by checkout (`tab.worktree === activeWorktreePath`, `paneTabs`): a
    // shell of the project just left is hidden, not shown under the wrong
    // name, and its agent keeps working. `pane_agents` is term-keyed, so the
    // board keeps its card too — Orca's dashboard draws every repo's
    // workspaces the same way.
    let orchestrator = Orchestrator::open(&target).ok();
    let root = orchestrator
        .as_ref()
        .map_or_else(|| target.clone(), |open| open.repo_root().to_path_buf());
    note_project(state.settings(), state.config_root(), &root);
    state.set_active_context(root.clone(), orchestrator);
    Ok(root.to_string_lossy().into_owned())
}

/// Whether this path would open as a git repository or a plain folder.
///
/// Asked by the tree's "프로젝트로 추가" door BEFORE the add, because the
/// original confirms the capability downgrade rather than falling into it:
/// a non-git folder loses worktrees, SCM and PRs, so Orca interposes
/// NonGitFolderDialog before `addNonGitFolder` ever runs (repos.ts:3223-3229;
/// AddProjectFromFolderDialog.tsx:111-113 asks the same question of the repo
/// it just added). The answer is derived exactly the way `open_project` will
/// later decide it — `Orchestrator::open` — one rule, asked twice.
#[tauri::command(async)]
pub(crate) fn project_kind(path: String) -> Result<String, String> {
    let target = PathBuf::from(&path)
        .canonicalize()
        .map_err(|error| format!("{path}: {error}"))?;
    if !target.is_dir() {
        return Err("폴더가 아닙니다".to_string());
    }
    Ok(if Orchestrator::open(&target).is_ok() {
        "git"
    } else {
        "folder"
    }
    .to_string())
}

/// File-name search under the project root, capped and pruned.
#[tauri::command]
pub(crate) async fn search_files(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<String>, String> {
    let root = state.active_root();
    let policy = explorer_policy::ExplorerPolicy::overlay(
        &load_settings(state.settings())?.document.explorer,
    );
    tauri::async_runtime::spawn_blocking(move || {
        project_search::search_names(&root, &query, &policy)
    })
    .await
    .map_err(|error| format!("file search worker failed: {error}"))?
}

/// Content search under the checkout being looked at.
#[tauri::command]
pub(crate) async fn search_text(
    state: State<'_, AppState>,
    query: String,
    options: Option<project_search::SearchOptions>,
) -> Result<Vec<project_search::TextHit>, String> {
    let root = state.active_root();
    let policy = explorer_policy::ExplorerPolicy::overlay(
        &load_settings(state.settings())?.document.explorer,
    );
    tauri::async_runtime::spawn_blocking(move || {
        project_search::search(&root, &query, options.as_ref(), &policy)
    })
    .await
    .map_err(|error| format!("text search worker failed: {error}"))?
}

/// One pane-owned Zo process, after its private channel has answered
/// `session.info` but before that channel is published to the window.
pub(crate) struct OpenedZoPane {
    pub(crate) pty: PtyHandle,
    pub(crate) addr: String,
    pub(crate) session_id: String,
    pub(crate) token: Option<String>,
    pub(crate) observation: zo_integration_runtime::LaunchObservation,
}

/// Open the one Zo IDE process used by both live lanes and restored tabs.
///
/// The address file and readiness wait belong here with `open_pane`; replaying
/// a stored `zo ...` argv cannot recreate either one. The process is also the
/// authority for the session id: a durable id is offered through `resume`, but
/// `session.info` says what actually opened and is the key every later channel
/// lookup uses.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_zo_pane_process(
    supervisor: &LaneSupervisor,
    cwd: &Path,
    resume: Option<&str>,
    addr_file: &Path,
    env: &[(String, String)],
    extra_args: &[String],
    rows: u16,
    cols: u16,
) -> Result<OpenedZoPane, String> {
    let observation = zo_integration_runtime::LaunchObservation::capture(
        supervisor.zo_path(),
        DEFAULT_PANE_CHANNEL_READY_TIMEOUT,
        env,
    );
    let token = supervisor.token().map(str::to_string);
    let opened = supervisor
        .open_pane(
            Box::new(LocalPty),
            cwd,
            resume,
            addr_file,
            env,
            extra_args,
            rows,
            cols,
            DEFAULT_PANE_CHANNEL_READY_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
    let addr = opened.addr;
    let session_id = pane_channel_session_id(addr.clone(), token.clone())?;
    Ok(OpenedZoPane {
        pty: opened.pty,
        addr,
        session_id,
        token,
        observation,
    })
}

/// Publish a successfully opened pane's private channel and start its one
/// subscriber. Called only after the caller has seated the PTY, so a first
/// history/frame event can always resolve the pane it belongs to.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attach_zo_pane_channel(
    app: &AppHandle,
    state: &AppState,
    owner: ZoChannelOwner,
    addr: String,
    token: Option<String>,
    session_id: String,
    observation: Option<zo_integration_runtime::LaunchObservation>,
) -> bool {
    // Claim the session before publishing its address. The old order replaced
    // `channels[session]` and only then discovered that `spawn_subscriber`
    // had refused the duplicate, leaving replies pointed at a socket whose
    // frames this window was not reading.
    if !subscribed(state.subscriptions()).insert(session_id.clone()) {
        if state.channel_owners().get(&session_id) != Some(&owner)
            && let ZoChannelOwner::Term(term) = owner
        {
            zo_integration_runtime::discovery(
                app,
                term,
                0,
                zo_integration_runtime::Reason::OwnerConflict,
            );
        }
        return false;
    }
    state.channels().insert(session_id.clone(), addr.clone());
    state.channel_owners().insert(session_id.clone(), owner);
    let epoch = zo_integration_runtime::begin(state, owner, &session_id, observation);
    spawn_subscriber(
        app.clone(),
        owner,
        epoch,
        addr,
        token,
        session_id,
        Arc::clone(state.subscriptions()),
    );
    true
}

/// Open one pane-owned Zo process and its private events channel.
///
/// The pane opens its own session. The window waits for the port selected by
/// `--events-bind 127.0.0.1:0`, then asks that channel for `session.info`; it
/// never starts a shared server or calls the unsupported `session.create`.
#[tauri::command]
pub(crate) async fn open_lane(
    app: AppHandle,
    state: State<'_, AppState>,
    session: Option<String>,
    worktree: Option<String>,
    rows: u16,
    cols: u16,
) -> Result<Lane, String> {
    let supervisor = state
        .supervisor()
        .cloned()
        .ok_or("`zo`가 PATH에 없습니다 — 레인 없이 계속 쓸 수 있습니다")?;

    // A lane runs where its task lives. A named workspace is resolved from
    // git's list or an exact cataloged folder match rather than taken as
    // given — this path becomes a process's working directory, and the
    // webview naming one must never be enough to start a process in it.
    // Naming none means "where I am looking", the only answer that keeps a
    // lane and the file panel talking about the same workspace.
    let cwd = match worktree {
        Some(requested) => match known_workspace_context(state.config_root(), &requested)? {
            KnownWorkspace::Git(_, chosen) => chosen.path,
            KnownWorkspace::Folder(folder) => folder,
        },
        None => state.active_root(),
    };
    let worktree_id = Some(cwd.to_string_lossy().into_owned());

    let mut lane = Lane::new(AgentKind::Zo, PaneKey::random());
    let id = lane.id;
    let addr_file = std::env::temp_dir().join(format!("zerocode-events-{id}.addr"));
    let worker = supervisor.clone();
    // The launch env every zo road takes — the routers connected in settings
    // ride it — through the one door, not an empty list of its own.
    let env = crate::hooks::agent_launch_env(state.local_data_root(), AgentKind::Zo.slug());
    let OpenedZoPane {
        pty,
        addr,
        session_id,
        token,
        observation,
    } = tauri::async_runtime::spawn_blocking(move || {
        open_zo_pane_process(
            &worker,
            &cwd,
            session.as_deref(),
            &addr_file,
            &env,
            &[],
            rows,
            cols,
        )
    })
    .await
    .map_err(|join| join.to_string())??;

    lane.session_id = Some(session_id.clone());
    lane.worktree_id = worktree_id;
    let staged = {
        let mut registry = state.registry();
        registry
            .insert(lane.clone(), pty)
            .map_err(|error| error.to_string())?;
        // Opening a lane stages it: the person asked for this one, so the
        // stage moves — and focus hands back the full first frame.
        registry.focus(id).map_err(|error| error.to_string())?
    };
    let _ = app.emit("lane:updated", lane.clone());
    emit_lane_event(&app, staged);
    attach_zo_pane_channel(
        &app,
        state.inner(),
        ZoChannelOwner::Lane(id),
        addr,
        token,
        session_id,
        Some(observation),
    );
    Ok(lane)
}

/// Answer a permission prompt. The decision is one of the four wire tags;
/// anything else the server treats as a deny — there is no accidental allow.
#[tauri::command]
pub(crate) async fn respond_permission(
    state: State<'_, AppState>,
    session: String,
    prompt_id: u64,
    decision: String,
) -> Result<(), String> {
    let supervisor = state
        .supervisor()
        .cloned()
        .ok_or("`zo`가 PATH에 없습니다")?;
    let addr = channel_addr(&state, &session)?;
    let token = supervisor.token().map(str::to_string);
    tauri::async_runtime::spawn_blocking(move || {
        with_client(addr, token, async move |client| {
            client
                .respond_to_permission(prompt_id, &decision)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// Set the gate state the pty cannot see. The webview calls this from the
/// structured channel — a `permission_prompt` frame raises the gate, its
/// answer lowers it — and the rows repaint from the resulting event.
#[tauri::command]
pub(crate) fn gate_lane(
    app: AppHandle,
    state: State<'_, AppState>,
    id: LaneId,
    gate: zerocode_core::LaneState,
) -> Result<(), String> {
    let _crumb = crate::crumbs::Command::enter("gate_lane");
    let event = {
        let mut registry = state.registry();
        registry
            .set_state(id, gate)
            .map_err(|error| error.to_string())?
    };
    if let Some(event) = event {
        emit_lane_event(&app, event);
    }
    Ok(())
}
