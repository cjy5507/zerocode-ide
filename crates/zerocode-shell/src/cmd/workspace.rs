//! Workspace commands.

use crate::*;

/// 지울 만한 워크스페이스를 훑고, 각각을 네 뷰 중 하나로 가른다.
///
/// **main 워크트리는 애초에 목록에 오르지 않는다.** git이 지우지도 못하고,
/// 저장소 자체를 지우자고 제안하는 화면은 그 화면을 두 번 다시 열지 않게
/// 만든다. 분류기는 `main-worktree`를 하드 블로커로도 알고 있는데, 그것은
/// 이중의 자물쇠다 — 이 훑기가 언젠가 새 경로를 얻어도 규칙 쪽이 혼자 버틴다.
///
/// `dismiss`와 `restore`는 스캔에 얹혀 간다. "무시"는 곧바로 다시 훑는
/// 몸짓이고, 두 번의 왕복으로 나누면 창이 방금 자기가 바꾼 것을 모르는 목록을
/// 한 프레임 그린다.
///
/// `only`는 preflight를 위한 것이다. 확인 직후 한 행만 다시 재어 보는데 저장소
/// 전부에 git을 돌리면, 스무 개를 지우는 동안 스무 번의 전체 스캔이 돈다.
///
/// 비동기인 이유는 이것이 디스크와 git이기 때문이다. 저장소 열 개의 워크트리를
/// stat하고 후보마다 git을 두 번 부르는 일은 창이 그리지 못하는 초가 되어서는
/// 안 된다.
#[tauri::command]
pub(crate) async fn workspace_cleanup_scan(
    state: State<'_, AppState>,
    dismiss: Option<CleanupDismissal>,
    restore: Option<String>,
    only: Option<Vec<String>>,
    // 창만이 아는 사실 하나. 저장되지 않은 타이핑은 git이 모르고, 모르는 것을
    // 깨끗하다고 부르면 이 다이얼로그가 사람이 방금 친 문장을 지운다.
    unsaved: Option<Vec<String>>,
) -> Result<CleanupReport, String> {
    use zerocode_core::workspace_cleanup as cleanup;

    let here = state.active();
    let active_root = here.root.clone();
    let active_project = here.orchestrator.map_or_else(
        || active_root.clone(),
        |open| open.repo_root().to_path_buf(),
    );
    // 이 창이 아는 "돌고 있는 것": 아직 끝나지 않은 레인 **과** 이 창이 열어 둔
    // 에이전트 터미널이 서 있는 체크아웃. 레인만 세던 시절에는 원장이 소환한
    // 워커가 몇 시간째 일하는 체크아웃이 방해물 하나 없는 행으로 이 목록에
    // 올랐다 — 워커의 판은 다른 레지스트리에 살기 때문이다.
    //
    // 락은 여기서 놓는다 — 아래는 git과 디스크이고, 그 동안 레지스트리를 쥐고
    // 있으면 모든 레인이 스캔을 기다린다.
    let busy = occupied_checkouts(&state);
    let unsaved = space_unsaved(unsaved);
    let config_root = state.config_root().to_path_buf();
    let local_data_root = state.local_data_root().to_path_buf();

    tauri::async_runtime::spawn_blocking(move || {
        // 사람이 방금 말한 것을 먼저 적는다. 적히지 않은 무시로 훑으면 목록은
        // 방금 누른 버튼을 되돌린 것처럼 보인다.
        if dismiss.is_some() || restore.is_some() {
            let mut held = stored_workspace_dismissals(&config_root);
            if let Some(one) = dismiss {
                held.insert(one.worktree_id.clone(), one);
            }
            if let Some(id) = restore {
                held.remove(&id);
            }
            write_workspace_dismissals(&config_root, &held)?;
        }
        let dismissals = stored_workspace_dismissals(&config_root);
        let runs = stored_automation_runs(&local_data_root);
        let now = now_epoch_ms();
        let wanted: Option<Vec<PathBuf>> =
            only.map(|paths| paths.into_iter().map(PathBuf::from).collect());

        let mut projects = stored_projects(&config_root);
        projects.push(active_project.to_string_lossy().into_owned());
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut subjects: Vec<CleanupSubject> = Vec::new();
        for stored in projects {
            let Ok(orchestrator) = Orchestrator::open(Path::new(&stored)) else {
                continue;
            };
            let repo_root = orchestrator.repo_root().to_path_buf();
            if !seen.insert(repo_root.clone()) {
                continue;
            }
            let project_name = repo_root
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("project")
                .to_string();
            for worktree in orchestrator.list().unwrap_or_default() {
                if worktree.is_main {
                    continue;
                }
                if let Some(wanted) = &wanted
                    && !wanted.iter().any(|one| one == &worktree.path)
                {
                    continue;
                }
                let listed = worktree.path.to_string_lossy().into_owned();
                let stored_activity = cleanup_stored_activity(&runs, &listed);
                // 이 워크스페이스가 어느 기계에 있는가. 아래의 mtime 읽기가
                // 이 답을 들고 가고, 뒤의 git 증거 수집도 **같은 함수에 같은
                // 경로로** 묻는다 — 두 번 부르는 것은 값싸고, 두 번 *결정하는*
                // 것이 아니다.
                let host = Host::for_workspace(&worktree.path);
                let facts = cleanup::WorktreeFacts {
                    is_main: worktree.is_main,
                    pinned: worktree.locked,
                    active: worktree.path == active_root,
                    running_terminal: busy.contains_key(&worktree.path),
                    unsaved_edits: unsaved.contains(&worktree.path),
                    archived: cleanup_looks_archived(&runs, &listed),
                    last_activity_ms: cleanup::last_activity(
                        stored_activity,
                        &worktree_activity_marks(&host, &worktree.path),
                    ),
                    // 아직 모른다. 후보로 남은 것만 git에게 묻는다.
                    dismissed: false,
                    git: cleanup::GitEvidence::unknown(),
                };
                subjects.push(CleanupSubject {
                    worktree,
                    project: repo_root.clone(),
                    project_name: project_name.clone(),
                    facts,
                });
            }
        }

        // 사유가 하나도 없는 것은 후보가 아니고, 후보가 아닌 것에는 git을
        // 부르지 않는다. 이 한 줄이 스캔의 값을 정한다 — 워크트리 백 개짜리
        // 기계에서 후보는 보통 서넛이다.
        subjects.retain(|subject| {
            !cleanup::reasons(
                subject.facts.archived,
                cleanup::idle_days(subject.facts.last_activity_ms, now),
            )
            .is_empty()
        });

        // 동시에 셋. 묶음마다 기다렸다 다음 묶음으로 가므로 넷이 함께 도는
        // 순간은 없다.
        let mut evidence: Vec<cleanup::GitEvidence> = Vec::with_capacity(subjects.len());
        for chunk in subjects.chunks(GIT_EVIDENCE_LANES) {
            let mut asked = std::thread::scope(|scope| {
                let asking: Vec<_> = chunk
                    .iter()
                    .map(|subject| {
                        scope.spawn(|| {
                            cleanup_git_evidence(
                                &Host::for_workspace(&subject.worktree.path),
                                &subject.worktree.path,
                            )
                        })
                    })
                    .collect();
                asking
                    .into_iter()
                    .map(|one| {
                        one.join().unwrap_or(cleanup::GitEvidence {
                            errored: true,
                            ..cleanup::GitEvidence::unknown()
                        })
                    })
                    .collect::<Vec<_>>()
            });
            evidence.append(&mut asked);
        }

        let rows = subjects
            .into_iter()
            .zip(evidence)
            .map(|(subject, git)| {
                let CleanupSubject {
                    worktree,
                    project,
                    project_name,
                    mut facts,
                } = subject;
                facts.git = git;
                let listed = worktree.path.to_string_lossy().into_owned();
                let fingerprint = cleanup::fingerprint(
                    worktree.branch.as_deref(),
                    worktree.head.as_deref(),
                    git.provably_clean(),
                    facts.last_activity_ms,
                );
                facts.dismissed = cleanup::dismissal_holds(dismissals.get(&listed), &fingerprint);
                let said = cleanup::classify(&facts, now);
                CleanupRow {
                    name: worktree.branch.clone().unwrap_or_else(|| {
                        worktree
                            .path
                            .file_name()
                            .and_then(std::ffi::OsStr::to_str)
                            .unwrap_or("workspace")
                            .to_string()
                    }),
                    id: listed.clone(),
                    path: listed,
                    project: project.to_string_lossy().into_owned(),
                    project_name,
                    branch: worktree.branch,
                    head: worktree.head,
                    tier: said.tier,
                    reasons: said.reasons,
                    blockers: said.blockers,
                    last_activity_ms: facts.last_activity_ms,
                    idle_days: said.idle_days,
                    git: CleanupGit {
                        dirty_files: git.dirty_files,
                        ahead: git.ahead,
                        has_upstream: git.has_upstream,
                        provably_clean: git.provably_clean(),
                    },
                    force: said.force,
                    fingerprint,
                }
            })
            .collect();

        Ok(CleanupReport {
            rows,
            scanned_at_ms: now,
            classifier_version: cleanup::CLASSIFIER_VERSION,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// 워크스페이스마다 디스크를 얼마나 쓰는지 훑는다.
///
/// **main 워크트리도 목록에 오른다.** 저 위의 청소 다이얼로그와 반대인데, 이유가
/// 반대이기 때문이다: 저기서 main은 "제안할 수 없는 것"이라 목록에 없어야 하고,
/// 여기서 main은 보통 **가장 큰 행**이라 없으면 합계가 거짓말이 된다. 지울 수
/// 없다는 사실은 배지가 말한다.
///
/// 비동기인 이유는 이것이 디스크이기 때문이다. 워크스페이스 스무 개를 훑는 일이
/// 창이 그리지 못하는 초가 되어서는 안 되고, 그래서 진행률이 따로 나간다.
#[tauri::command]
pub(crate) async fn workspace_space_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    unsaved: Option<Vec<String>>,
) -> Result<SpaceReport, String> {
    use std::sync::atomic::Ordering;
    use zerocode_core::workspace_cleanup as cleanup;

    // 두 스캔이 같이 돌면 상한이 둘로 갈리고 진행률이 서로를 덮어쓴다.
    if SPACE_SCANNING.swap(true, Ordering::SeqCst) {
        return Err(SPACE_ALREADY.to_string());
    }
    let flag = ScanFlag(&SPACE_SCANNING);

    let here = state.active();
    let active_root = here.root.clone();
    let active_project = here.orchestrator.map_or_else(
        || active_root.clone(),
        |open| open.repo_root().to_path_buf(),
    );
    // 락은 여기서 놓는다 — 아래는 디스크이고, 그 동안 레지스트리를 쥐고 있으면
    // 모든 레인이 이 스캔을 기다린다. 레인과 에이전트 터미널을 함께 센다:
    // 크기를 재는 화면이 가장 큰 행을 "비어 있음"으로 그리면, 사람이 디스크를
    // 비우려고 누르는 그 버튼이 일하는 에이전트의 땅을 걷는다.
    let lanes = occupied_checkouts(&state);
    let unsaved = space_unsaved(unsaved);
    let config_root = state.config_root().to_path_buf();
    let local_data_root = state.local_data_root().to_path_buf();

    tauri::async_runtime::spawn_blocking(move || {
        let _flag = flag;
        // 앞선 취소가 남아 있으면 이 스캔은 시작하자마자 멈춘다.
        SPACE_CANCEL.store(false, Ordering::SeqCst);
        let runs = stored_automation_runs(&local_data_root);
        let now = now_epoch_ms();

        let mut projects = stored_projects(&config_root);
        projects.push(active_project.to_string_lossy().into_owned());
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut subjects: Vec<SpaceSubject> = Vec::new();
        let mut repo_errors: Vec<SpaceRepoError> = Vec::new();
        for stored in projects {
            // 열리지 않는 것은 폴더 워크스페이스다. 배너로 올리지 않는다 —
            // 저장소가 아닌 것을 "답하지 못한 저장소"라고 부르는 배너는 언제나
            // 떠 있고, 언제나 떠 있는 경고는 경고가 아니다.
            let Ok(orchestrator) = Orchestrator::open(Path::new(&stored)) else {
                continue;
            };
            let repo_root = orchestrator.repo_root().to_path_buf();
            if !seen.insert(repo_root.clone()) {
                continue;
            }
            let project_name = repo_root
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("project")
                .to_string();
            match orchestrator.list() {
                Ok(worktrees) => {
                    for worktree in worktrees {
                        let name = worktree.branch.clone().unwrap_or_else(|| {
                            worktree
                                .path
                                .file_name()
                                .and_then(std::ffi::OsStr::to_str)
                                .unwrap_or("workspace")
                                .to_string()
                        });
                        subjects.push(SpaceSubject {
                            worktree,
                            name,
                            project: repo_root.clone(),
                            project_name: project_name.clone(),
                        });
                    }
                }
                // 저장소는 배너다, 행이 아니다 — Orca도 그렇게 한다
                // (`analysis.repos.filter(repo => repo.error !== null)`).
                Err(error) => repo_errors.push(SpaceRepoError {
                    path: repo_root.to_string_lossy().into_owned(),
                    name: project_name,
                    error: error.to_string(),
                }),
            }
        }

        let total_count = subjects.len();
        let tally = SpaceTally {
            app: &app,
            total: total_count,
            inner: Mutex::new((0, None)),
        };
        tally.announced("");

        let mut measured: Vec<SpaceMeasured> = Vec::with_capacity(total_count);
        for chunk in subjects.chunks(SPACE_WORKTREE_LANES) {
            if SPACE_CANCEL.load(Ordering::SeqCst) {
                break;
            }
            let mut asked = std::thread::scope(|scope| {
                let asking: Vec<_> = chunk
                    .iter()
                    .map(|subject| {
                        let tally = &tally;
                        scope.spawn(move || {
                            let held = space_measure(
                                &Host::for_workspace(&subject.worktree.path),
                                &subject.worktree.path,
                            );
                            tally.stepped(&subject.name);
                            held
                        })
                    })
                    .collect();
                asking
                    .into_iter()
                    .map(|one| {
                        one.join()
                            .unwrap_or(SpaceMeasured::Stopped(space::Status::Failed))
                    })
                    .collect::<Vec<_>>()
            });
            measured.append(&mut asked);
        }
        let cancelled = SPACE_CANCEL.load(Ordering::SeqCst);
        tally.announced("");

        let mut total_bytes = 0u64;
        let mut scanned_count = 0usize;
        // `zip`이 짧은 쪽에서 멈춘다 — 취소되면 아직 재지 않은 워크스페이스는
        // 아예 행이 되지 않는다. 0바이트짜리 행으로 실어 보내면 그것은 "쟀고
        // 비어 있다"와 구별되지 않는다.
        let rows: Vec<SpaceRow> = subjects
            .into_iter()
            .zip(measured)
            .map(|(subject, held)| {
                let (status, entries, size_bytes) = match held {
                    SpaceMeasured::Ok(entries, size) => (space::Status::Ok, entries, size),
                    SpaceMeasured::Stopped(status) => (status, Vec::new(), 0),
                };
                if status == space::Status::Ok {
                    scanned_count += 1;
                    total_bytes += size_bytes;
                }
                let listed = subject.worktree.path.to_string_lossy().into_owned();
                let facts = cleanup::WorktreeFacts {
                    is_main: subject.worktree.is_main,
                    pinned: subject.worktree.locked,
                    active: subject.worktree.path == active_root,
                    running_terminal: lanes.contains_key(&subject.worktree.path),
                    unsaved_edits: unsaved.contains(&subject.worktree.path),
                    // 이 화면의 물음이 아니다. 보관 여부는 "유휴"의 사유이고,
                    // 무시 서랍은 저쪽 다이얼로그가 사람에게 준 것이다 —
                    // 크기를 묻는 화면이 저쪽의 서랍을 열어 행을 감추면, 사람은
                    // 자기가 어디서 무엇을 눌렀는지 영영 알 수 없다.
                    archived: false,
                    dismissed: false,
                    last_activity_ms: cleanup::last_activity(
                        cleanup_stored_activity(&runs, &listed),
                        &worktree_activity_marks(
                            &Host::for_workspace(&subject.worktree.path),
                            &subject.worktree.path,
                        ),
                    ),
                    git: cleanup::GitEvidence::unknown(),
                };
                let said = cleanup::classify(&facts, now);
                SpaceRow {
                    id: listed.clone(),
                    name: subject.name,
                    path: listed,
                    project: subject.project.to_string_lossy().into_owned(),
                    project_name: subject.project_name,
                    branch: subject.worktree.branch,
                    size_bytes,
                    size_text: space::format_bytes(size_bytes),
                    status,
                    entries: entries.into_iter().map(SpaceEntry::new).collect(),
                    is_main: subject.worktree.is_main,
                    is_active: facts.active,
                    last_activity_ms: facts.last_activity_ms,
                    ready: space::ready_to_delete(status, &said.blockers),
                    blockers: said.blockers,
                    force: said.force,
                    // 아직 묻지 않았다. 이 화면이 열리면 곧바로 뒤에서 묻는다.
                    git_checked: false,
                    dirty_files: 0,
                    ahead: 0,
                    lanes: lanes
                        .get(&subject.worktree.path)
                        .copied()
                        .unwrap_or_default(),
                }
            })
            .collect();

        Ok(SpaceReport {
            rows,
            repo_errors,
            scanned_at_ms: now,
            total_bytes,
            total_text: space::format_bytes(total_bytes),
            scanned_count,
            total_count,
            cancelled,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// 도는 스캔에게 멈추라고 말한다.
///
/// 프로세스를 죽이는 것이 아니라 플래그를 세우는 것뿐이고, 순회가 그것을 읽고
/// 스스로 선다. 도는 스캔이 없으면 아무 일도 일어나지 않는다 — 다음 스캔이
/// 시작할 때 플래그를 내리므로, 남은 참 하나가 다음 스캔을 죽이지 않는다.
#[tauri::command]
pub(crate) fn workspace_space_cancel() {
    let _crumb = crate::crumbs::Command::enter("workspace_space_cancel");
    SPACE_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 삭제 후보 행들의 git 증거를 뒤에서 채운다.
///
/// 이것이 없으면 이 화면의 모든 행이 "git 미확인"이다 — `GitEvidence::unknown()`이
/// `unknown-base`를 세우기 때문이고, 그것은 옳다: 묻지 않은 것을 깨끗하다고
/// 부르는 화면은 사람의 커밋을 지운다. 그래서 물어서 지우는 길이 따로 있다.
///
/// 창은 **크기를 잰 행만** 여기에 싣는다. 재지 못한 행은 애초에 후보가 아니고,
/// 그 규칙은 게이트가 붙잡는다.
#[tauri::command]
pub(crate) async fn workspace_space_git(
    state: State<'_, AppState>,
    paths: Vec<String>,
    unsaved: Option<Vec<String>>,
) -> Result<Vec<SpaceGitRow>, String> {
    use zerocode_core::workspace_cleanup as cleanup;

    let here = state.active();
    let active_root = here.root.clone();
    let active_project = here.orchestrator.map_or_else(
        || active_root.clone(),
        |open| open.repo_root().to_path_buf(),
    );
    let lanes = occupied_checkouts(&state);
    let unsaved = space_unsaved(unsaved);
    let config_root = state.config_root().to_path_buf();

    tauri::async_runtime::spawn_blocking(move || {
        let now = now_epoch_ms();
        let wanted: HashSet<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        // 이 창이 아는 워크트리만. 웹뷰가 준 문자열로 git을 부르는 길은 없다.
        //
        // 목록을 **한 번** 읽고 거른다. 경로마다 `known_worktree_context`를
        // 부르면 그 함수가 매번 저장소 전부에 `git worktree list`를 돌리므로,
        // 후보 스물 × 저장소 다섯이면 git을 세기도 전에 백 번 부르게 된다 —
        // 이 화면이 프로세스를 스폰하지 않으려고 만들어졌다는 것을 생각하면
        // 특히 나쁜 종류의 되돌림이다.
        let mut subjects: Vec<Worktree> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut projects = stored_projects(&config_root);
        projects.push(active_project.to_string_lossy().into_owned());
        for stored in projects {
            let Ok(orchestrator) = Orchestrator::open(Path::new(&stored)) else {
                continue;
            };
            if !seen.insert(orchestrator.repo_root().to_path_buf()) {
                continue;
            }
            for worktree in orchestrator.list().unwrap_or_default() {
                if wanted.contains(&worktree.path) {
                    subjects.push(worktree);
                }
            }
        }

        // 여섯씩. 청소 다이얼로그의 셋과 다른 것은 이 일이 사람이 보고 있는
        // 동안 뒤에서 도는 일이기 때문이다.
        let mut evidence: Vec<cleanup::GitEvidence> = Vec::with_capacity(subjects.len());
        for chunk in subjects.chunks(SPACE_GIT_LANES) {
            let mut asked = std::thread::scope(|scope| {
                let asking: Vec<_> = chunk
                    .iter()
                    .map(|worktree| {
                        scope.spawn(|| {
                            cleanup_git_evidence(
                                &Host::for_workspace(&worktree.path),
                                &worktree.path,
                            )
                        })
                    })
                    .collect();
                asking
                    .into_iter()
                    .map(|one| {
                        one.join().unwrap_or(cleanup::GitEvidence {
                            errored: true,
                            ..cleanup::GitEvidence::unknown()
                        })
                    })
                    .collect::<Vec<_>>()
            });
            evidence.append(&mut asked);
        }

        Ok(subjects
            .into_iter()
            .zip(evidence)
            .map(|(worktree, git)| {
                let facts = cleanup::WorktreeFacts {
                    is_main: worktree.is_main,
                    pinned: worktree.locked,
                    active: worktree.path == active_root,
                    running_terminal: lanes.contains_key(&worktree.path),
                    unsaved_edits: unsaved.contains(&worktree.path),
                    archived: false,
                    dismissed: false,
                    last_activity_ms: now,
                    git,
                };
                let said = cleanup::classify(&facts, now);
                SpaceGitRow {
                    id: worktree.path.to_string_lossy().into_owned(),
                    // 창이 이 행을 실었다는 것은 크기를 쟀다는 뜻이고, 그
                    // 규칙은 게이트가 붙잡는다.
                    ready: space::ready_to_delete(space::Status::Ok, &said.blockers),
                    blockers: said.blockers,
                    force: said.force,
                    git_checked: true,
                    dirty_files: git.dirty_files,
                    ahead: git.ahead,
                }
            })
            .collect())
    })
    .await
    .map_err(|join| join.to_string())?
}
