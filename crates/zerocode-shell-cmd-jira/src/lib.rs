//! Jira integration commands.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use zerocode_core::agent_spec;
use zerocode_orchestrator::{Orchestrator, Worktree};
use zerocode_shell_state::{AppState, jira_attachments, jira_store, work_item_store};

const RECENT_PROJECTS_FILE: &str = "recent-projects.json";
const NOT_A_WORKTREE: &str = "이 저장소의 워크트리가 아닙니다";

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            elapsed.as_millis().min(i64::MAX as u128) as i64
        })
}

fn note_window_event(local_data_root: &Path, line: &str) {
    use std::io::Write;

    let path = local_data_root.join("window-errors.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 1_000_000) {
        let _ = std::fs::rename(&path, local_data_root.join("window-errors.log.1"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(format!("{} {line}\n", now_epoch_ms()).as_bytes());
    }
}

fn known_project_roots(state: &AppState) -> Vec<PathBuf> {
    let path = state.settings().root().join(RECENT_PROJECTS_FILE);
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .collect()
}

fn matching_worktree<'a>(known: &'a [Worktree], requested: &str) -> Option<&'a Worktree> {
    let requested = Path::new(requested);
    known
        .iter()
        .find(|worktree| worktree.path.as_path() == requested)
}

fn known_worktree_context(
    state: &AppState,
    requested: &str,
) -> Result<(Orchestrator, Worktree), String> {
    for root in known_project_roots(state) {
        let Ok(orchestrator) = Orchestrator::open(root) else {
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

pub mod commands {
    use super::{
        AppHandle, AppState, Deserialize, Duration, Emitter, Instant, Manager, Mutex, OnceLock,
        Path, PathBuf, Serialize, State, Worktree, agent_spec, jira_attachments, jira_store,
        known_worktree_context, note_window_event, now_epoch_ms, work_item_store,
    };

    /// Full Jira description and recent comments, bounded and wrapped as untrusted
    /// agent context. Optional orchestration guidance keeps the launched agent as
    /// the real coordinator; the webview never creates a Run under a forged actor.
    #[tauri::command]
    pub async fn jira_agent_context(
        state: State<'_, AppState>,
        site_id: String,
        key: String,
        worker_agent: Option<String>,
        max_workers: Option<u8>,
    ) -> Result<JiraAgentContextReport, zerocode_core::jira::Failure> {
        let (detail, comments) = tokio::join!(
            jira_issue_detail_from_store(state.jira(), Some(&site_id), &key),
            jira_issue_comments_from_store(state.jira(), Some(&site_id), &key),
        );
        let detail = detail?;
        let comments = comments?;
        // 첨부는 여기서만 실체화된다 — 워크트리 생성과 자동 orchestration이
        // 이 한 함수를 지나므로 둘은 언제나 같은 manifest를 읽는다.
        let (site, token, kind) = jira_card_site(state.jira(), Some(&site_id))?;
        let local_data = state.local_data_root().to_path_buf();
        let notes =
            jira_attachment_notes(&local_data, &site, &token, kind, &key, &detail.attachments)
                .await;
        let prompt = zerocode_core::jira::agent_context(&detail, &comments, &notes);
        let worker_agent = worker_agent.unwrap_or_else(|| "codex".to_string());
        if agent_spec(&worker_agent).is_none() {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: format!("알 수 없는 에이전트입니다: {worker_agent}"),
            });
        }
        let orchestration_prompt = zerocode_core::jira::orchestration_context(
            &prompt,
            &format!("jira:{site_id}:{key}"),
            &worker_agent,
            max_workers.unwrap_or(2),
        );
        Ok(JiraAgentContextReport {
            bytes: prompt.len(),
            orchestration_bytes: orchestration_prompt.len(),
            title: detail.head.title,
            status: detail.head.status,
            comment_count: comments.len(),
            attachment_count: detail.attachments.len(),
            prompt,
            orchestration_prompt,
        })
    }

    /// 카드의 이미지 미리보기 — 썸네일 한 장, 캐시 우선, 실측된 그림일 때만.
    /// 캐시된 바이트도 매번 검문된다: 그림이 아니면 캐시 히트가 아니라 다시
    /// 내려받을 일이다.
    #[tauri::command]
    pub async fn jira_attachment_preview(
        state: State<'_, AppState>,
        site_id: Option<String>,
        key: Option<String>,
        attachment_id: Option<String>,
    ) -> Result<JiraAttachmentPreview, zerocode_core::jira::Failure> {
        let refused = |message: &str| zerocode_core::jira::Failure {
            kind: zerocode_core::jira::FailureKind::Unknown,
            message: message.to_string(),
        };
        let key = key.unwrap_or_default();
        let id = attachment_id.unwrap_or_default();
        if !zerocode_core::jira::valid_issue_key(&key) {
            return Err(refused("이슈 키가 아닙니다"));
        }
        let (site, token, kind) = jira_card_site(state.jira(), site_id.as_deref())?;
        let local_data = state.local_data_root().to_path_buf();
        let Some(target) = jira_attachments::thumbnail_file(&local_data, &site.id, &key, &id)
        else {
            return Err(refused("첨부 id가 아닙니다"));
        };
        let cached = {
            let held = target.clone();
            tokio::task::spawn_blocking(move || std::fs::read(&held).ok())
                .await
                .ok()
                .flatten()
        };
        let bytes = match cached {
            Some(bytes) if jira_attachments::sniff_image(&bytes).is_some() => bytes,
            _ => {
                let bytes = jira_fetch_bytes(
                    JiraCall {
                        url: zerocode_core::jira::attachment_thumbnail_url(
                            &site.site_url,
                            kind,
                            &id,
                        ),
                        method: JiraMethod::Get,
                        body: None,
                        email: &site.email,
                        token: &token,
                        kind,
                        timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
                    },
                    jira_attachments::PREVIEW_LIMIT_BYTES,
                )
                .await?;
                // 그림으로 실측된 바이트만 캐시에 앉는다 — 오류 페이지가 캐시를
                // 오염시켜 다음 열람까지 망치는 일이 없도록.
                if jira_attachments::sniff_image(&bytes).is_some() {
                    let stored = target.clone();
                    let kept = bytes.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        jira_attachments::store_bytes(&stored, &kept)
                    })
                    .await;
                }
                bytes
            }
        };
        let data_uri = jira_attachments::image_data_uri(&bytes)
            .ok_or_else(|| refused("미리보기로 그릴 수 없는 첨부입니다"))?;
        Ok(JiraAttachmentPreview { data_uri })
    }

    /// 새 목소리 하나 — 줄마다 한 문단의 ADF로 POST.
    #[tauri::command]
    pub async fn jira_comment_issue(
        state: State<'_, AppState>,
        site_id: Option<String>,
        key: Option<String>,
        body: Option<String>,
    ) -> Result<(), zerocode_core::jira::Failure> {
        let key = key.unwrap_or_default();
        if !zerocode_core::jira::valid_issue_key(&key) {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "이슈 키가 아닙니다".to_string(),
            });
        }
        let said = body.unwrap_or_default();
        if said.trim().is_empty() {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "댓글 내용이 없습니다".to_string(),
            });
        }
        let (site, token, kind) = jira_card_site(state.jira(), site_id.as_deref())?;
        jira_call(JiraCall {
            url: zerocode_core::jira::issue_comment_post_url(&site.site_url, kind, &key),
            method: JiraMethod::Post,
            body: Some(zerocode_core::jira::comment_post_body(&said)),
            email: &site.email,
            token: &token,
            kind,
            timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
        })
        .await?;
        Ok(())
    }

    /// Register one site, after proving the credential works.
    ///
    /// `/myself` is the check, which is Orca's own (`:126077`) and is the right
    /// one: it needs no project to exist, and its answer IS the display name and
    /// account id the site record wants. Storing first and discovering later that
    /// the token was mistyped would leave a window claiming a connection it does
    /// not have.
    #[tauri::command]
    pub async fn jira_connect(
        state: State<'_, AppState>,
        site_url: String,
        email: String,
        api_token: String,
        auth_type: String,
    ) -> Result<JiraStatus, String> {
        let kind = zerocode_core::jira::SiteKind::try_from_auth_type(&auth_type)
            .map_err(str::to_string)?;
        let origin = jira_store::https_origin(&site_url).map_err(|error| error.to_string())?;
        if api_token.is_empty() {
            return Err("토큰을 입력하세요".to_string());
        }
        let me = jira_call(JiraCall {
            url: zerocode_core::jira::myself_url(&origin, kind),
            method: JiraMethod::Get,
            body: None,
            email: &email,
            token: &api_token,
            kind,
            timeout_ms: zerocode_core::jira::CONNECT_TIMEOUT_MS,
        })
        .await
        .map_err(|failure| failure.message)?;
        let (display_name, account_id) = zerocode_core::jira::read_myself(&me);
        let id = jira_site_id(&origin, &email);
        let site = jira_store::JiraSite {
            id: id.clone(),
            site_url: origin,
            email: email.trim().to_string(),
            display_name,
            account_id,
            auth_type: kind.as_str().to_string(),
        };
        state
            .jira()
            .connect_commit(site, &api_token)
            .map(jira_status_view)
            .map_err(|error| error.to_string())
    }

    /// Create one ordinary Jira issue through the same credential and HTTPS
    /// boundary as every read/update command.
    ///
    /// `issue_type` accepts a numeric Jira type id or a type name. Dynamic create
    /// metadata remains a renderer concern; this command validates the bounded
    /// wire values and never turns them into a URL or query fragment.
    #[tauri::command]
    pub async fn jira_create_issue(
        state: State<'_, AppState>,
        site_id: Option<String>,
        project_key: String,
        issue_type: String,
        summary: String,
        description: Option<String>,
    ) -> Result<JiraCreatedIssue, zerocode_core::jira::Failure> {
        let invalid = |message: &str| zerocode_core::jira::Failure {
            kind: zerocode_core::jira::FailureKind::Unknown,
            message: message.to_string(),
        };
        let project_key = project_key.trim();
        if !valid_jira_project_key(project_key) {
            return Err(invalid("Jira 프로젝트 키가 올바르지 않습니다"));
        }
        let issue_type = issue_type.trim();
        if issue_type.is_empty() || issue_type.chars().count() > 128 {
            return Err(invalid("Jira 이슈 유형이 올바르지 않습니다"));
        }
        let summary = summary.trim();
        if summary.is_empty() || summary.chars().count() > 255 {
            return Err(invalid("Jira 이슈 제목은 1~255자여야 합니다"));
        }
        let description = description
            .as_deref()
            .filter(|text| !text.trim().is_empty());
        if description.is_some_and(|text| text.len() > JIRA_CREATE_DESCRIPTION_MAX_BYTES) {
            return Err(invalid("Jira 이슈 설명이 너무 깁니다"));
        }

        let (site, token, kind) = jira_card_site(state.jira(), site_id.as_deref())?;
        let answer = jira_call(JiraCall {
            url: jira_create_issue_url(&site.site_url, kind),
            method: JiraMethod::Post,
            body: Some(jira_create_issue_body(
                kind,
                project_key,
                issue_type,
                summary,
                description,
            )),
            email: &site.email,
            token: &token,
            kind,
            timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
        })
        .await?;
        let key = answer
            .get("key")
            .and_then(serde_json::Value::as_str)
            .filter(|key| zerocode_core::jira::valid_issue_key(key))
            .ok_or_else(|| invalid("Jira가 생성된 이슈 키를 보내지 않았습니다"))?
            .to_string();
        let id = answer
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        Ok(JiraCreatedIssue {
            site_id: site.id,
            url: format!(
                "{}/browse/{key}",
                zerocode_core::jira::site_origin(&site.site_url)
            ),
            key,
            id,
        })
    }

    #[tauri::command(async)]
    pub fn jira_disconnect(
        state: State<'_, AppState>,
        site_id: String,
    ) -> Result<JiraStatus, String> {
        state
            .jira()
            .disconnect(&site_id)
            .map(jira_status_view)
            .map_err(|error| error.to_string())
    }

    /// 대화 전부 — 페이지를 걸어 모은다. 네 페이지(200)에서 멈추는 것은 이
    /// 창의 상한이다: 그보다 긴 대화는 카드가 아니라 브라우저의 몫이다.
    #[tauri::command]
    pub async fn jira_issue_comments(
        state: State<'_, AppState>,
        site_id: Option<String>,
        key: Option<String>,
    ) -> Result<Vec<zerocode_core::jira::IssueComment>, zerocode_core::jira::Failure> {
        let key = key.unwrap_or_default();
        jira_issue_comments_from_store(state.jira(), site_id.as_deref(), &key).await
    }

    /// 행 밑의 카드 한 장 — 상세 한 번 읽기.
    #[tauri::command]
    pub async fn jira_issue_detail(
        state: State<'_, AppState>,
        site_id: Option<String>,
        key: Option<String>,
    ) -> Result<zerocode_core::jira::IssueDetail, zerocode_core::jira::Failure> {
        let key = key.unwrap_or_default();
        jira_issue_detail_from_store(state.jira(), site_id.as_deref(), &key).await
    }

    /// 편집이 고를 것들 — 전이·우선순위·후보를 한 번에. 각각의 실패는 빈
    /// 목록이다(실측의 그 관대함): 우선순위를 못 읽었다고 전이의 문까지 닫지
    /// 않는다.
    #[tauri::command]
    pub async fn jira_issue_options(
        state: State<'_, AppState>,
        site_id: Option<String>,
        key: Option<String>,
    ) -> Result<serde_json::Value, zerocode_core::jira::Failure> {
        let key = key.unwrap_or_default();
        if !zerocode_core::jira::valid_issue_key(&key) {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "이슈 키가 아닙니다".to_string(),
            });
        }
        let (site, token, kind) = jira_card_site(state.jira(), site_id.as_deref())?;
        let ask = |url: String| {
            jira_call(JiraCall {
                url,
                method: JiraMethod::Get,
                body: None,
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
        };
        let transitions = ask(zerocode_core::jira::issue_transitions_url(
            &site.site_url,
            kind,
            &key,
        ))
        .await
        .map(|body| zerocode_core::jira::read_transitions(&body))
        .unwrap_or_default();
        let priorities = ask(zerocode_core::jira::priorities_url(&site.site_url, kind))
            .await
            .map(|body| zerocode_core::jira::read_priorities(&body))
            .unwrap_or_default();
        let users = ask(zerocode_core::jira::assignable_users_url(
            &site.site_url,
            kind,
            &key,
        ))
        .await
        .map(|body| zerocode_core::jira::read_assignable(kind, &body))
        .unwrap_or_default();
        Ok(serde_json::json!({
            "transitions": transitions,
            "priorities": priorities,
            "users": users,
        }))
    }

    #[tauri::command]
    pub async fn jira_issues(
        state: State<'_, AppState>,
        preset: Option<String>,
        site_id: Option<String>,
    ) -> Result<JiraIssuesReport, zerocode_core::jira::Failure> {
        let preset = preset.as_deref().map_or_else(
            zerocode_core::jira::Preset::default,
            zerocode_core::jira::Preset::from_name,
        );
        jira_issue_report(state.jira(), site_id, preset.jql(), false).await
    }

    #[tauri::command]
    pub async fn jira_search_issues(
        state: State<'_, AppState>,
        jql: String,
        site_id: Option<String>,
    ) -> Result<JiraIssuesReport, zerocode_core::jira::Failure> {
        let Some(jql) = jira_search_jql(&jql) else {
            return Ok(JiraIssuesReport {
                issues: Vec::new(),
                failures: Vec::new(),
                interpreted_as_text: false,
            });
        };
        jira_issue_report(state.jira(), site_id, jql, true).await
    }

    /// Projects available to the selected creation site. Cloud pages until
    /// Jira says the page bean is complete; Server/DC answers with one array.
    /// Successful answers live briefly in-process so opening and reopening the
    /// dialog does not spend another API request immediately.
    #[tauri::command]
    pub async fn jira_projects(
        state: State<'_, AppState>,
        site_id: String,
    ) -> Result<Vec<zerocode_core::jira::Project>, zerocode_core::jira::Failure> {
        let (site, token, kind) = jira_card_site(state.jira(), Some(&site_id))?;
        if let Some(projects) = jira_project_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(&site.id).cloned())
            .filter(|entry| entry.loaded_at.elapsed() <= JIRA_PROJECT_CACHE_TTL)
            .map(|entry| entry.projects)
        {
            return Ok(projects);
        }

        let mut projects = Vec::new();
        let mut start_at = 0;
        let mut complete = false;
        for _ in 0..JIRA_PROJECT_PAGE_MAX {
            let answer = jira_call(JiraCall {
                url: zerocode_core::jira::project_url(&site.site_url, kind, start_at),
                method: JiraMethod::Get,
                body: None,
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
            .await?;
            let page = zerocode_core::jira::read_project_page(kind, &answer);
            projects.extend(page.projects);
            match page.next_start {
                Some(next) if next > start_at => start_at = next,
                Some(_) => {
                    return Err(zerocode_core::jira::Failure {
                        kind: zerocode_core::jira::FailureKind::Unknown,
                        message: "Jira 프로젝트 페이지가 앞으로 진행되지 않았습니다.".to_string(),
                    });
                }
                None => {
                    complete = true;
                    break;
                }
            }
        }
        if !complete {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "Jira 프로젝트 목록이 허용된 페이지 수를 초과했습니다.".to_string(),
            });
        }
        let mut keys = std::collections::HashSet::new();
        projects.retain(|project| keys.insert(project.key.clone()));
        projects.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.key.cmp(&right.key))
        });
        if let Ok(mut cache) = jira_project_cache().lock() {
            cache.insert(
                site.id,
                JiraProjectCacheEntry {
                    loaded_at: Instant::now(),
                    projects: projects.clone(),
                },
            );
        }
        Ok(projects)
    }

    #[tauri::command(async)]
    pub fn jira_select_site(
        state: State<'_, AppState>,
        site_id: String,
    ) -> Result<JiraStatus, String> {
        let selection = if site_id == "all" {
            jira_store::JiraSelection::All
        } else {
            jira_store::JiraSelection::Site(site_id)
        };
        state
            .jira()
            .select(selection)
            .map(jira_status_view)
            .map_err(|error| error.to_string())
    }

    #[tauri::command(async)]
    pub fn jira_status(state: State<'_, AppState>) -> Result<JiraStatus, String> {
        state
            .jira()
            .status()
            .map(jira_status_view)
            .map_err(|error| error.to_string())
    }

    /// Re-arm one event's failed actions and attempt them again. Only a person asks
    /// for this: a lost `Unknown` write cannot be retried automatically without
    /// risking a duplicate, so the choice to accept that risk is always theirs.
    #[tauri::command(async)]
    pub fn jira_sync_retry(
        app: AppHandle,
        state: State<'_, AppState>,
        event: String,
    ) -> Result<Option<zerocode_core::JiraSyncEvent>, String> {
        let rearmed = work_item_store::requeue_failed(state.settings(), &event, now_epoch_ms())?;
        if rearmed.is_some() {
            schedule_jira_sync(&app, event);
        }
        Ok(rearmed)
    }

    /// Hold or resume outbound Jira writes. Resuming drains everything pending so a
    /// person's decision to sync takes effect at once; holding simply stops sends.
    #[tauri::command(async)]
    pub fn jira_sync_set_paused(
        app: AppHandle,
        state: State<'_, AppState>,
        paused: bool,
    ) -> Result<bool, String> {
        let now = work_item_store::set_paused(state.settings(), paused)?;
        if !now {
            for event_id in work_item_store::pending_event_ids(state.settings())? {
                schedule_jira_sync(&app, event_id);
            }
        }
        let _ = app.emit("jira:sync-paused", now);
        Ok(now)
    }

    #[tauri::command]
    pub async fn jira_test_connection(
        state: State<'_, AppState>,
        site_id: String,
    ) -> Result<JiraTestReport, String> {
        let file = state.jira().load().map_err(|error| error.to_string())?;
        let site = file
            .sites
            .iter()
            .find(|site| site.id == site_id)
            .ok_or_else(|| format!("Jira 사이트를 찾지 못했습니다: {site_id}"))?;
        Ok(match jira_test_one(state.jira(), site).await {
            Ok(()) => JiraTestReport {
                site_id,
                standing: "connected",
                message: None,
            },
            Err(failure) => JiraTestReport {
                site_id,
                standing: if failure.kind == zerocode_core::jira::FailureKind::Auth {
                    "auth_error"
                } else {
                    "unavailable"
                },
                message: Some(failure.message),
            },
        })
    }

    /// 고쳐쓰기 — 실측의 세 걸음 그대로: 본체 PUT(온 것만), 담당자 PUT(빈 id는
    /// 해제), 전이 POST. 아무것도 안 왔으면 아무것도 안 나간다.
    #[allow(clippy::too_many_arguments)] // Tauri command arguments are the public IPC wire.
    #[tauri::command]
    pub async fn jira_update_issue(
        state: State<'_, AppState>,
        site_id: Option<String>,
        key: Option<String>,
        title: Option<String>,
        labels: Option<Vec<String>>,
        priority_id: Option<String>,
        assignee_id: Option<String>,
        transition_id: Option<String>,
    ) -> Result<(), zerocode_core::jira::Failure> {
        let key = key.unwrap_or_default();
        if !zerocode_core::jira::valid_issue_key(&key) {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "이슈 키가 아닙니다".to_string(),
            });
        }
        if title.as_deref().is_some_and(|said| said.trim().is_empty()) {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "제목은 비울 수 없습니다".to_string(),
            });
        }
        let (site, token, kind) = jira_card_site(state.jira(), site_id.as_deref())?;
        if let Some(body) = zerocode_core::jira::issue_update_body(
            title.as_deref(),
            labels.as_deref(),
            priority_id.as_deref(),
        ) {
            jira_call(JiraCall {
                url: zerocode_core::jira::issue_url(&site.site_url, kind, &key),
                method: JiraMethod::Put,
                body: Some(body),
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
            .await?;
        }
        if let Some(assignee) = assignee_id.as_deref() {
            jira_call(JiraCall {
                url: zerocode_core::jira::assignee_url(&site.site_url, kind, &key),
                method: JiraMethod::Put,
                body: Some(zerocode_core::jira::assignee_body(kind, assignee)),
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
            .await?;
        }
        if let Some(transition) = transition_id.as_deref().filter(|id| !id.is_empty()) {
            jira_call(JiraCall {
                url: zerocode_core::jira::issue_transitions_url(&site.site_url, kind, &key),
                method: JiraMethod::Post,
                body: Some(zerocode_core::jira::transition_body(transition)),
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
            .await?;
        }
        Ok(())
    }

    /// Called only after an agent terminal has actually started in the linked
    /// worktree. Enqueue first, then let the network run independently of terminal
    /// startup; a crash leaves a visible pending row rather than losing the intent.
    #[tauri::command(async)]
    pub fn jira_worktree_started(
        app: AppHandle,
        state: State<'_, AppState>,
        worktree: String,
    ) -> Result<Option<String>, String> {
        let (_, identity) = worktree_identity_for_link(&state, &worktree)?;
        let event = work_item_store::enqueue_started(
            state.settings(),
            &identity.worktree_id,
            now_epoch_ms(),
        )?;
        if let Some(event_id) = &event {
            schedule_jira_sync(&app, event_id.clone());
        }
        Ok(event)
    }

    /// Attach one Jira issue to one known worktree. The site URL comes from the
    /// credential store rather than the webview, so a renderer cannot turn a link
    /// operation into an arbitrary external URL record.
    #[tauri::command(async)]
    pub fn link_jira_worktree(
        state: State<'_, AppState>,
        worktree: String,
        draft: JiraWorktreeLinkDraft,
    ) -> Result<zerocode_core::LinkedWorkItem, String> {
        if !zerocode_core::jira::valid_issue_key(&draft.key) {
            return Err("이슈 키가 아닙니다".to_string());
        }
        draft.sync.validate()?;
        let (worktree, identity) = worktree_identity_for_link(&state, &worktree)?;
        let sites = state.jira().load().map_err(|error| error.to_string())?;
        let site = sites
            .sites
            .iter()
            .find(|site| site.id == draft.site_id)
            .ok_or_else(|| format!("Jira 사이트를 찾지 못했습니다: {}", draft.site_id))?;
        let now = now_epoch_ms();
        let prior = work_item_store::link_for_worktree(state.settings(), &identity.worktree_id)?;
        let same_issue = prior.as_ref().is_some_and(|held| {
            let (site_id, key) = held.jira_site_and_key();
            site_id == draft.site_id && key == draft.key
        });
        let linked_at_ms = prior
            .as_ref()
            .filter(|_| same_issue)
            .map_or(now, |held| held.linked_at_ms);
        let orchestration = prior.as_ref().filter(|_| same_issue).map_or_else(
            || zerocode_core::LinkedOrchestration {
                requested: draft.orchestration_requested,
                ..Default::default()
            },
            |held| {
                let mut orchestration = held.orchestration.clone();
                orchestration.requested |= draft.orchestration_requested;
                orchestration
            },
        );
        let link_id =
            zerocode_core::jira_link_id(&identity.worktree_id, &draft.site_id, &draft.key);
        let linked = zerocode_core::LinkedWorkItem {
            link_id,
            repository_id: identity.repository_id,
            worktree_id: identity.worktree_id,
            worktree_path: worktree.path.to_string_lossy().into_owned(),
            source: zerocode_core::LinkedWorkItemSource::Jira {
                site_id: draft.site_id,
                key: draft.key.clone(),
                url: format!(
                    "{}/browse/{}",
                    zerocode_core::jira::site_origin(&site.site_url),
                    draft.key
                ),
                title: draft.title,
                status: draft.status,
                updated: draft.updated,
            },
            sync: draft.sync,
            orchestration,
            linked_at_ms,
            updated_at_ms: now,
        };
        work_item_store::put(state.settings(), linked)
    }

    #[tauri::command(async)]
    pub fn set_jira_link_sync(
        state: State<'_, AppState>,
        worktree: String,
        sync: zerocode_core::JiraSyncPolicy,
        orchestration_requested: bool,
    ) -> Result<zerocode_core::LinkedWorkItem, String> {
        sync.validate()?;
        let (_, identity) = worktree_identity_for_link(&state, &worktree)?;
        let mut link = work_item_store::link_for_worktree(state.settings(), &identity.worktree_id)?
            .ok_or_else(|| "이 워크트리에 연결된 작업 항목이 없습니다".to_string())?;
        link.sync = sync;
        link.orchestration.requested = orchestration_requested;
        link.updated_at_ms = now_epoch_ms();
        work_item_store::put(state.settings(), link)
    }

    #[tauri::command(async)]
    pub fn unlink_worktree_item(
        state: State<'_, AppState>,
        worktree: String,
    ) -> Result<bool, String> {
        let (_, identity) = worktree_identity_for_link(&state, &worktree)?;
        work_item_store::remove(state.settings(), &identity.worktree_id)
    }

    #[tauri::command(async)]
    pub fn work_item_links(state: State<'_, AppState>) -> Result<work_item_store::Report, String> {
        work_item_store::report(state.settings())
    }

    /* ---- Jira ----------------------------------------------------------------
     *
     * The one integration this product cannot borrow somebody else's credential
     * for. `gh.rs` reaches GitHub through the user's own authenticated `gh`, which
     * is both what Orca does and the better shape — the token stays in the tool
     * that owns it. There is no `jira` binary to hand that job to, so this window
     * holds the credential itself, and everything below exists to make that as
     * small as possible: one site record, one token file at 0600, one door out to
     * the network, and no `Debug` on anything the token passes through.
     *
     * What is asked, and where, is measured (1-i, and the endpoints re-read from
     * `asar-1.4.164/out/main/index.js`): the REST version is a Cloud/Server
     * negotiation, the issue list is a POST with a JQL body, and the default query
     * is what is assigned to me. Those four facts live in
     * [`zerocode_core::jira`] as pure functions so they are tested without a site,
     * an account or a socket. */

    /// The name this window gives itself on the wire. Orca sends `Orca`; a client
    /// that lies about who it is makes an admin reading their access log unable to
    /// tell which program is spending their rate limit.
    const JIRA_USER_AGENT: &str = concat!("ZeroCode/", env!("CARGO_PKG_VERSION"));

    /// An authenticated error body is untrusted and never enters an error string.
    /// Retain only this much inside the redacted classifier, then drop it. A
    /// successful issue response still needs room for the requested 50 rows,
    /// under its own ceiling.
    const JIRA_ERROR_BODY_LIMIT_BYTES: usize = 16 * 1024;
    const JIRA_SUCCESS_BODY_LIMIT_BYTES: usize = 1024 * 1024;
    const JIRA_PROJECT_CACHE_TTL: Duration = Duration::from_secs(60);
    const JIRA_PROJECT_PAGE_MAX: usize = 200;

    #[derive(Clone)]
    struct JiraProjectCacheEntry {
        loaded_at: Instant,
        projects: Vec<zerocode_core::jira::Project>,
    }

    fn jira_project_cache()
    -> &'static Mutex<std::collections::HashMap<String, JiraProjectCacheEntry>> {
        static CACHE: OnceLock<Mutex<std::collections::HashMap<String, JiraProjectCacheEntry>>> =
            OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
    }

    /// One connected site, in the shape it is stored and the shape the window
    /// reads. The token is NOT here — it lives in its own file, so a status
    /// answer travelling to the renderer cannot carry a credential by accident.
    /// How much of the digest names the file. Orca keeps 24 characters of its own
    /// (`siteId`), which is 144 bits — far past what a person's own handful of
    /// sites could collide in, and short enough to read in a directory listing.
    pub const JIRA_SITE_ID_CHARS: usize = 24;

    /// The id a site and account pair answers to.
    ///
    /// Hashed rather than composed, because this string is a file name: a site URL
    /// has slashes and colons in it, and an email has whatever the person's
    /// provider allows. Case-folded on the email for the reason two spellings of
    /// one address are one account — otherwise reconnecting with a capital letter
    /// leaves the old token file behind, orphaned and still readable.
    pub fn jira_site_id(site_url: &str, email: &str) -> String {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};

        let mut digest = Sha256::new();
        digest.update(zerocode_core::jira::site_origin(site_url).as_bytes());
        digest.update(b"\n");
        digest.update(email.trim().to_ascii_lowercase().as_bytes());
        let full = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.finalize());
        full.chars().take(JIRA_SITE_ID_CHARS).collect()
    }

    /// The `Authorization` line for one site, and the only place a token is put
    /// into a string.
    ///
    /// Measured (`authHeader`, `asar-1.4.164/out/main/index.js:125932-125935`): a
    /// self-hosted site with no identity is a personal access token and goes as
    /// `Bearer`; everything else — Cloud always, self-hosted with a username — is
    /// `Basic base64(identity:secret)`.
    pub fn jira_authorization(
        email: &str,
        token: &str,
        kind: zerocode_core::jira::SiteKind,
    ) -> String {
        use base64::Engine as _;

        let email = email.trim();
        if matches!(kind, zerocode_core::jira::SiteKind::Server) && email.is_empty() {
            return format!("Bearer {token}");
        }
        let pair = format!("{email}:{token}");
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(pair)
        )
    }

    async fn drain_jira_response(mut response: reqwest::Response, limit: usize) {
        let mut remaining = limit;
        while remaining > 0 {
            let Ok(Some(chunk)) = response.chunk().await else {
                break;
            };
            if chunk.is_empty() {
                break;
            }
            remaining = remaining.saturating_sub(chunk.len());
        }
    }

    async fn read_jira_response_text(
        mut response: reqwest::Response,
        limit: usize,
    ) -> Result<(String, bool), reqwest::Error> {
        let mut body = Vec::with_capacity(limit.min(8 * 1024));
        let mut truncated = false;
        while let Some(chunk) = response.chunk().await? {
            let remaining = limit.saturating_sub(body.len());
            if chunk.len() > remaining {
                body.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
            if body.len() == limit {
                // Ask for one more frame to distinguish an exactly-sized response
                // from a larger one without retaining another byte of it.
                truncated = response.chunk().await?.is_some();
                break;
            }
        }
        Ok((String::from_utf8_lossy(&body).into_owned(), truncated))
    }

    /// One request to a Jira site. Held apart from its answer so the door below
    /// takes exactly one argument, and deliberately without `Debug`: a struct that
    /// holds a token and can be printed is a token in a log the day somebody adds
    /// a `dbg!` while chasing something else.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum JiraMethod {
        Get,
        Post,
        Put,
    }

    pub fn jira_http_method(method: JiraMethod) -> reqwest::Method {
        match method {
            JiraMethod::Get => reqwest::Method::GET,
            JiraMethod::Post => reqwest::Method::POST,
            JiraMethod::Put => reqwest::Method::PUT,
        }
    }

    pub struct JiraCall<'a> {
        url: String,
        /// The verb is explicit: Jira uses JSON bodies for both POST and PUT, and
        /// inferring the verb from body presence silently turns issue edits into
        /// creates/actions.
        method: JiraMethod,
        body: Option<serde_json::Value>,
        email: &'a str,
        token: &'a str,
        kind: zerocode_core::jira::SiteKind,
        timeout_ms: u64,
    }

    /// Parse the address at the credential boundary, not only in the connect UI.
    ///
    /// Site metadata is a user-owned file and can outlive the version that wrote
    /// it.  Rechecking here means a hand-edited or legacy `http://` record can
    /// never make an Authorization header leave this process in plaintext.
    pub fn jira_https_url(raw: &str) -> Result<reqwest::Url, String> {
        const ERROR: &str = "Jira 사이트 주소는 https:// 로 시작해야 합니다";
        let url = reqwest::Url::parse(raw.trim()).map_err(|_| ERROR.to_string())?;
        if url.scheme() != "https"
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ERROR.to_string());
        }
        Ok(url)
    }

    /// **The one door out to a Jira site.** Every read this window does goes
    /// through here.
    ///
    /// One function rather than a call at each command for three reasons that are
    /// all the same reason: the timeout is applied once (so no future caller can
    /// forget it and hang the panel), the credential is attached once (so no
    /// future caller can put it somewhere else), and a failure is classified once
    /// (so `401` means "reconnect" and a dead network means "quiet" everywhere,
    /// rather than in whichever branch remembered).
    /// The one Jira client. Both roads out — the JSON door below and the bounded
    /// attachment-bytes door — are built here, so the deadline, the user agent and
    /// the redirect refusal cannot fork per caller. Jira traffic has no legitimate
    /// cross-origin redirect, and refusing all of them also guarantees an HTTPS
    /// response cannot bounce a credential-bearing request onto plaintext HTTP.
    fn jira_client(timeout_ms: u64) -> Result<reqwest::Client, zerocode_core::jira::Failure> {
        reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .user_agent(JIRA_USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| zerocode_core::jira::offline_failure(&error.to_string()))
    }

    async fn jira_call(
        call: JiraCall<'_>,
    ) -> Result<serde_json::Value, zerocode_core::jira::Failure> {
        let url = jira_https_url(&call.url).map_err(|message| zerocode_core::jira::Failure {
            kind: zerocode_core::jira::FailureKind::Unknown,
            message,
        })?;
        let client = jira_client(call.timeout_ms)?;
        let mut request = client.request(jira_http_method(call.method), url);
        request = request.header("accept", "application/json").header(
            "authorization",
            jira_authorization(call.email, call.token, call.kind),
        );
        if let Some(body) = &call.body {
            request = request.json(body);
        }
        // A transport failure is the offline branch whether it was a refused
        // connection, a name that would not resolve, or the deadline above — from
        // where the person is sitting those are one event, and the answer to all
        // three is "try again in a moment".
        let answer = request
            .send()
            .await
            .map_err(|error| zerocode_core::jira::offline_failure(&error.to_string()))?;
        let status = answer.status().as_u16();
        let successful = (200..300).contains(&status);
        if !successful {
            return Err(jira_failure_from_response(answer, status).await);
        }
        // Jira's edit and assignee endpoints commonly answer success with no
        // representation. A 204 is not malformed JSON; it is the complete answer.
        if status == 204 {
            return Ok(serde_json::Value::Null);
        }
        let (said, truncated) = read_jira_response_text(answer, JIRA_SUCCESS_BODY_LIMIT_BYTES)
            .await
            .map_err(|error| zerocode_core::jira::offline_failure(&error.to_string()))?;
        if truncated {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "Jira 응답이 허용 크기를 초과했습니다.".to_string(),
            });
        }
        serde_json::from_str(&said).map_err(|_| zerocode_core::jira::Failure {
            kind: zerocode_core::jira::FailureKind::Unknown,
            message: "Jira가 JSON이 아닌 답을 보냈습니다.".to_string(),
        })
    }

    /// Read only enough of a failed response to recognize Jira's parser
    /// vocabulary, then discard it. The classifier deliberately builds its
    /// message from the status and kind alone: no representation of an
    /// authenticated upstream value can cross IPC into product text.
    async fn jira_failure_from_response(
        answer: reqwest::Response,
        status: u16,
    ) -> zerocode_core::jira::Failure {
        let raw = read_jira_response_text(answer, JIRA_ERROR_BODY_LIMIT_BYTES)
            .await
            .map(|(body, _)| body)
            .unwrap_or_default();
        zerocode_core::jira::failure_for_status_redacted(status, &raw)
    }

    /// The second road through the one client: attachment bytes, bounded, GET
    /// only. A failure is classified from the status alone with the authenticated
    /// body drained unread — the JSON door's exact posture — and a body that
    /// outgrows `limit_bytes` is refused rather than truncated: half an attachment
    /// on disk is a lie the cache would repeat forever.
    async fn jira_fetch_bytes(
        call: JiraCall<'_>,
        limit_bytes: usize,
    ) -> Result<Vec<u8>, zerocode_core::jira::Failure> {
        let url = jira_https_url(&call.url).map_err(|message| zerocode_core::jira::Failure {
            kind: zerocode_core::jira::FailureKind::Unknown,
            message,
        })?;
        let client = jira_client(call.timeout_ms)?;
        let request = client.request(reqwest::Method::GET, url).header(
            "authorization",
            jira_authorization(call.email, call.token, call.kind),
        );
        let mut answer = request
            .send()
            .await
            .map_err(|error| zerocode_core::jira::offline_failure(&error.to_string()))?;
        let status = answer.status().as_u16();
        if !(200..300).contains(&status) {
            drain_jira_response(answer, JIRA_ERROR_BODY_LIMIT_BYTES).await;
            return Err(zerocode_core::jira::failure_for_status(status, ""));
        }
        let mut body = Vec::with_capacity(limit_bytes.min(64 * 1024));
        loop {
            let chunk = answer
                .chunk()
                .await
                .map_err(|error| zerocode_core::jira::offline_failure(&error.to_string()))?;
            let Some(chunk) = chunk else { break };
            if body.len().saturating_add(chunk.len()) > limit_bytes {
                drain_jira_response(answer, JIRA_ERROR_BODY_LIMIT_BYTES).await;
                return Err(zerocode_core::jira::Failure {
                    kind: zerocode_core::jira::FailureKind::Unknown,
                    message: "첨부가 허용 크기를 초과했습니다.".to_string(),
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    #[derive(Clone, Serialize)]
    pub struct JiraSiteView {
        #[serde(flatten)]
        site: jira_store::JiraSite,
        active: bool,
        selected: bool,
        credential: jira_store::JiraCredentialStanding,
        /// Storage standing is not a live probe. The UI promotes this field to
        /// `connected/auth_error/unavailable` after an explicit test/read.
        standing: &'static str,
    }

    #[derive(Clone, Serialize)]
    pub struct JiraStatus {
        connected: bool,
        active_site_id: Option<String>,
        selected_site_id: String,
        credential_protection: jira_store::JiraCredentialProtection,
        sites: Vec<JiraSiteView>,
    }

    fn jira_status_view(status: jira_store::JiraStoreStatus) -> JiraStatus {
        let selected_site_id = match status.selected {
            jira_store::JiraSelection::All => "all".to_string(),
            jira_store::JiraSelection::Site(id) => id,
        };
        JiraStatus {
            connected: status.connected,
            active_site_id: status.active_site_id,
            selected_site_id,
            credential_protection: status.credential_protection,
            sites: status
                .sites
                .into_iter()
                .map(|row| {
                    let standing = match row.credential {
                        jira_store::JiraCredentialStanding::Available => "connected",
                        jira_store::JiraCredentialStanding::Missing
                        | jira_store::JiraCredentialStanding::Unreadable => "auth_error",
                    };
                    JiraSiteView {
                        site: row.site,
                        active: row.active,
                        selected: row.selected,
                        credential: row.credential,
                        standing,
                    }
                })
                .collect(),
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct JiraWorktreeLinkDraft {
        site_id: String,
        key: String,
        title: String,
        status: String,
        updated: Option<String>,
        #[serde(default)]
        sync: zerocode_core::JiraSyncPolicy,
        #[serde(default)]
        orchestration_requested: bool,
    }

    fn worktree_identity_for_link(
        state: &AppState,
        requested: &str,
    ) -> Result<(Worktree, zerocode_core::git_dir::Identity), String> {
        let (_, worktree) = known_worktree_context(state, requested)?;
        let identity = zerocode_core::git_dir::identity(&worktree.path)
            .ok_or_else(|| "워크트리의 Git 신원을 읽지 못했습니다".to_string())?;
        Ok((worktree, identity))
    }

    #[derive(Serialize)]
    pub struct JiraTestReport {
        site_id: String,
        standing: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    }

    async fn jira_test_one(
        store: &jira_store::JiraStore,
        site: &jira_store::JiraSite,
    ) -> Result<(), zerocode_core::jira::Failure> {
        let token = store
            .read_token(&site.id)
            .map_err(|error| zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Auth,
                message: error.to_string(),
            })?;
        let kind = zerocode_core::jira::SiteKind::from_auth_type(&site.auth_type);
        jira_call(JiraCall {
            url: zerocode_core::jira::myself_url(&site.site_url, kind),
            method: JiraMethod::Get,
            body: None,
            email: &site.email,
            token: &token,
            kind,
            timeout_ms: zerocode_core::jira::CONNECT_TIMEOUT_MS,
        })
        .await
        .map(|_| ())
    }

    /// The issue list, asked of the connected site.
    ///
    /// The error type is structured rather than a string because the window draws
    /// two different things with it: a refusal is a red banner that names an
    /// action, and an unreachable site is a quiet line. A `String` here would
    /// force the renderer to sniff words out of a sentence to tell them apart,
    /// which is the same bug in a different language.
    #[derive(Serialize)]
    pub struct JiraIssueView {
        site_id: String,
        site_name: String,
        #[serde(flatten)]
        issue: zerocode_core::jira::Issue,
    }

    #[derive(Serialize)]
    pub struct JiraSiteFailure {
        site_id: String,
        #[serde(flatten)]
        failure: zerocode_core::jira::Failure,
    }

    #[derive(Serialize)]
    pub struct JiraIssuesReport {
        issues: Vec<JiraIssueView>,
        failures: Vec<JiraSiteFailure>,
        interpreted_as_text: bool,
    }

    async fn jira_search_with_text_fallback<T, Ask, Asked>(
        jql: &str,
        mut ask: Ask,
    ) -> (Result<T, zerocode_core::jira::Failure>, bool)
    where
        Ask: FnMut(String) -> Asked,
        Asked: std::future::Future<Output = Result<T, zerocode_core::jira::Failure>>,
    {
        match ask(jql.to_string()).await {
            Err(failure) if failure.kind == zerocode_core::jira::FailureKind::Jql => {
                (ask(zerocode_core::jira::text_search_jql(jql)).await, true)
            }
            answer => (answer, false),
        }
    }

    async fn jira_issues_one(
        store: &jira_store::JiraStore,
        site: &jira_store::JiraSite,
        jql: &str,
    ) -> Result<Vec<JiraIssueView>, zerocode_core::jira::Failure> {
        let token = store
            .read_token(&site.id)
            .map_err(|error| zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Auth,
                message: error.to_string(),
            })?;
        let kind = zerocode_core::jira::SiteKind::from_auth_type(&site.auth_type);
        let answer = jira_call(JiraCall {
            url: zerocode_core::jira::search_url(&site.site_url, kind),
            method: JiraMethod::Post,
            body: Some(zerocode_core::jira::search_body(
                jql,
                zerocode_core::jira::ITEM_LIMIT,
            )),
            email: &site.email,
            token: &token,
            kind,
            timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
        })
        .await?;
        Ok(zerocode_core::jira::read_issues(&site.site_url, &answer)
            .into_iter()
            .map(|issue| JiraIssueView {
                site_id: site.id.clone(),
                site_name: site.display_name.clone(),
                issue,
            })
            .collect())
    }

    /// One JQL, asked of every selected site.
    ///
    /// The preset list and the typed search differ in exactly one value — the JQL —
    /// so they share this walk rather than each keeping their own. A second copy
    /// would be a second place to decide what a single failing site means, and that
    /// judgement (below: one site asked and one site refused is a refusal, not a
    /// report with an empty list in it) is the whole reason the panel can tell a
    /// bad token from a site that simply has nothing to show.
    async fn jira_issue_report(
        store: &jira_store::JiraStore,
        site_id: Option<String>,
        jql: &str,
        allow_text_fallback: bool,
    ) -> Result<JiraIssuesReport, zerocode_core::jira::Failure> {
        let mut selection =
            store
                .resolve_selection()
                .map_err(|error| zerocode_core::jira::Failure {
                    kind: zerocode_core::jira::FailureKind::Unknown,
                    message: error.to_string(),
                })?;
        if let Some(site_id) = site_id.filter(|id| id != "all") {
            selection.sites.retain(|site| site.id == site_id);
        }
        if selection.sites.is_empty() {
            return Err(zerocode_core::jira::disconnected_failure());
        }
        let attempted = selection.sites.len();
        let mut issues = Vec::new();
        let mut failures = Vec::new();
        let mut interpreted_as_text = false;
        for site in selection.sites {
            let (answer, fell_back) = if allow_text_fallback {
                let site = &site;
                jira_search_with_text_fallback(jql, |candidate| async move {
                    jira_issues_one(store, site, &candidate).await
                })
                .await
            } else {
                (jira_issues_one(store, &site, jql).await, false)
            };
            interpreted_as_text |= fell_back;
            match answer {
                Ok(mut rows) => issues.append(&mut rows),
                Err(failure) => failures.push(JiraSiteFailure {
                    site_id: site.id,
                    failure,
                }),
            }
        }
        if attempted == 1 && issues.is_empty() && failures.len() == 1 {
            return Err(failures.remove(0).failure);
        }
        Ok(JiraIssuesReport {
            issues,
            failures,
            interpreted_as_text,
        })
    }

    /// 카드가 물을 한 사이트와 그 자격 — 행이 이미 아는 site_id로 찾는다.
    /// 선택 밖의 사이트는 끊긴 것과 같다: 행이 화면에 있었다면 선택 안이다.
    fn jira_card_site(
        store: &jira_store::JiraStore,
        site_id: Option<&str>,
    ) -> Result<
        (jira_store::JiraSite, String, zerocode_core::jira::SiteKind),
        zerocode_core::jira::Failure,
    > {
        let file = store.load().map_err(|error| zerocode_core::jira::Failure {
            kind: zerocode_core::jira::FailureKind::Unknown,
            message: error.to_string(),
        })?;
        let wanted = site_id.or(file.active_site_id.as_deref());
        let site = file
            .sites
            .into_iter()
            .find(|site| Some(site.id.as_str()) == wanted)
            .ok_or_else(zerocode_core::jira::disconnected_failure)?;
        let token = store
            .read_token(&site.id)
            .map_err(|error| zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Auth,
                message: error.to_string(),
            })?;
        let kind = zerocode_core::jira::SiteKind::from_auth_type(&site.auth_type);
        Ok((site, token, kind))
    }

    async fn jira_issue_detail_from_store(
        store: &jira_store::JiraStore,
        site_id: Option<&str>,
        key: &str,
    ) -> Result<zerocode_core::jira::IssueDetail, zerocode_core::jira::Failure> {
        if !zerocode_core::jira::valid_issue_key(key) {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "이슈 키가 아닙니다".to_string(),
            });
        }
        let (site, token, kind) = jira_card_site(store, site_id)?;
        let answer = jira_call(JiraCall {
            url: zerocode_core::jira::issue_detail_url(&site.site_url, kind, key),
            method: JiraMethod::Get,
            body: None,
            email: &site.email,
            token: &token,
            kind,
            timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
        })
        .await?;
        zerocode_core::jira::read_issue_detail(&site.site_url, &answer).ok_or_else(|| {
            zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "카드로 읽을 수 없는 답입니다".to_string(),
            }
        })
    }

    async fn jira_issue_comments_from_store(
        store: &jira_store::JiraStore,
        site_id: Option<&str>,
        key: &str,
    ) -> Result<Vec<zerocode_core::jira::IssueComment>, zerocode_core::jira::Failure> {
        if !zerocode_core::jira::valid_issue_key(key) {
            return Err(zerocode_core::jira::Failure {
                kind: zerocode_core::jira::FailureKind::Unknown,
                message: "이슈 키가 아닙니다".to_string(),
            });
        }
        let (site, token, kind) = jira_card_site(store, site_id)?;
        let mut voices = Vec::new();
        let mut start = 0u32;
        loop {
            let answer = jira_call(JiraCall {
                url: zerocode_core::jira::issue_comments_url(&site.site_url, kind, key, start),
                method: JiraMethod::Get,
                body: None,
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
            .await?;
            let (mut page, total) = zerocode_core::jira::read_comments(&answer);
            let walked = page.len();
            voices.append(&mut page);
            start += zerocode_core::jira::COMMENT_PAGE;
            if walked == 0 || voices.len() as u64 >= total || start >= 200 {
                break;
            }
        }
        Ok(voices)
    }

    /* ---- 첨부 실체화 — 프롬프트 manifest와 카드 미리보기가 같은 캐시를 본다.
     *
     * 경로·한도·바이트 검문은 [`jira_attachments`]의 순수 함수들이고, 여기는
     * 그 판단들을 한 사이트의 자격 증명과 이어 붙이는 자리다. 내려받기는 카드가
     * 아니라 **에이전트 컨텍스트가 청해질 때만**(lazy) 일어나고, 어떤 실패도
     * 컨텍스트 전체를 쓰러뜨리지 않는다 — manifest의 사연 한 줄이 된다. */

    /// Host words for the prompt manifest. English on purpose: the manifest rides
    /// an English prompt, and these lines are the host's own, never Jira's.
    const ATTACHMENT_NOTE_COUNT: &str = "skipped: over the attachment count limit";
    const ATTACHMENT_NOTE_MIME: &str = "skipped: this type is not materialized";
    const ATTACHMENT_NOTE_FILE_LIMIT: &str = "skipped: over the per-file size limit";
    const ATTACHMENT_NOTE_BUDGET: &str = "skipped: over the issue attachment budget";

    /// One attachment, cache-first. `Ok` is the materialized path and how many
    /// bytes of the issue budget this call actually spent — a cache hit spends
    /// neither network nor budget: it was admitted under these same limits when
    /// it landed. `Err` is a manifest reason, never a failure of the whole
    /// context.
    async fn materialize_jira_attachment(
        local_data: &Path,
        site: &jira_store::JiraSite,
        token: &str,
        kind: zerocode_core::jira::SiteKind,
        key: &str,
        one: &zerocode_core::jira::Attachment,
        budget_bytes: usize,
    ) -> Result<(PathBuf, usize), String> {
        let Some(target) =
            jira_attachments::content_file(local_data, &site.id, key, &one.id, &one.name)
        else {
            return Err("skipped: not an addressable attachment".to_string());
        };
        if let Ok(meta) = std::fs::metadata(&target)
            && meta.is_file()
            && (one.size == 0 || meta.len() == one.size)
        {
            return Ok((target, 0));
        }
        if !jira_attachments::mime_allowed(&one.mime) {
            return Err(ATTACHMENT_NOTE_MIME.to_string());
        }
        if one.size > jira_attachments::FILE_LIMIT_BYTES as u64 {
            return Err(ATTACHMENT_NOTE_FILE_LIMIT.to_string());
        }
        if one.size > budget_bytes as u64 {
            return Err(ATTACHMENT_NOTE_BUDGET.to_string());
        }
        let bytes = jira_fetch_bytes(
            JiraCall {
                // 응답이 실어 온 `content` URL이 아니라 id에서 다시 만든 주소 —
                // 조작된 응답이 자격 증명을 남의 호스트로 데려가지 못한다.
                url: zerocode_core::jira::attachment_content_url(&site.site_url, kind, &one.id),
                method: JiraMethod::Get,
                body: None,
                email: &site.email,
                token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            },
            jira_attachments::FILE_LIMIT_BYTES.min(budget_bytes),
        )
        .await
        .map_err(|failure| format!("failed: {}", failure.message))?;
        let spent = bytes.len();
        let stored = target.clone();
        tokio::task::spawn_blocking(move || jira_attachments::store_bytes(&stored, &bytes))
            .await
            .map_err(|error| format!("failed: {error}"))?
            .map_err(|error| format!("failed: {error}"))?;
        Ok((target, spent))
    }

    /// The manifest the prompt carries: every attachment appears exactly once,
    /// with a local path or the reason it has none — an attachment never simply
    /// vanishes from the agent's view.
    async fn jira_attachment_notes(
        local_data: &Path,
        site: &jira_store::JiraSite,
        token: &str,
        kind: zerocode_core::jira::SiteKind,
        key: &str,
        attachments: &[zerocode_core::jira::Attachment],
    ) -> Vec<zerocode_core::jira::AttachmentNote> {
        let mut notes = Vec::with_capacity(attachments.len());
        let mut budget = jira_attachments::TOTAL_LIMIT_BYTES;
        for (index, one) in attachments.iter().enumerate() {
            let placed = if index >= jira_attachments::COUNT_MAX {
                Err(ATTACHMENT_NOTE_COUNT.to_string())
            } else {
                materialize_jira_attachment(local_data, site, token, kind, key, one, budget).await
            };
            notes.push(match placed {
                Ok((path, spent)) => {
                    budget = budget.saturating_sub(spent);
                    zerocode_core::jira::AttachmentNote {
                        name: one.name.clone(),
                        mime: one.mime.clone(),
                        size: one.size,
                        local_path: Some(path.display().to_string()),
                        note: None,
                    }
                }
                Err(reason) => zerocode_core::jira::AttachmentNote {
                    name: one.name.clone(),
                    mime: one.mime.clone(),
                    size: one.size,
                    local_path: None,
                    note: Some(reason),
                },
            });
        }
        notes
    }

    #[derive(Serialize)]
    pub struct JiraAttachmentPreview {
        /// A bounded `data:` image URI — the one image form the window's CSP
        /// already admits. Never a remote URL: the renderer must not learn where
        /// the bytes came from, only what they draw.
        data_uri: String,
    }

    #[derive(Serialize)]
    pub struct JiraAgentContextReport {
        prompt: String,
        orchestration_prompt: String,
        title: String,
        status: String,
        comment_count: usize,
        attachment_count: usize,
        bytes: usize,
        orchestration_bytes: usize,
    }

    fn jira_sync_failure_state(
        failure: zerocode_core::jira::Failure,
    ) -> zerocode_core::JiraSyncActionState {
        match failure.kind {
            zerocode_core::jira::FailureKind::Offline
            | zerocode_core::jira::FailureKind::Server
            | zerocode_core::jira::FailureKind::Unknown => {
                zerocode_core::JiraSyncActionState::Unknown(failure.message)
            }
            _ => zerocode_core::JiraSyncActionState::Refused(failure.message),
        }
    }

    fn jira_sync_comment(event: &zerocode_core::JiraSyncEvent) -> String {
        let heading = match event.succeeded {
            Some(true) => "ZeroCode orchestration completed successfully.",
            Some(false) => "ZeroCode orchestration reported a failed attempt.",
            None => "ZeroCode orchestration started work.",
        };
        if event.summary.trim().is_empty() {
            heading.to_string()
        } else {
            format!("{heading}\n\n{}", event.summary.trim())
        }
    }

    pub fn schedule_jira_sync(app: &AppHandle, event_id: String) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = process_jira_sync_event(&app, &event_id).await {
                note_window_event(
                    app.state::<AppState>().local_data_root(),
                    &format!("Jira sync {event_id} could not be processed: {error}"),
                );
            }
        });
    }

    /// Event ids whose Jira POST is in flight in this process. A lease here keeps a
    /// boot drain and a live enqueue — or two rapid retries — from POSTing the same
    /// transition or comment twice; the settings file lock alone guards the store,
    /// not the network call between read and settle.
    fn jira_sync_inflight() -> &'static Mutex<std::collections::HashSet<String>> {
        static INFLIGHT: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
        INFLIGHT.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
    }

    /// Claims the lease for `event_id`, releasing it on drop. `None` means another
    /// task already holds it, so this one must stand down.
    fn claim_jira_sync(event_id: &str) -> Option<JiraSyncLease> {
        let mut held = jira_sync_inflight().lock().expect("jira sync lease");
        if held.insert(event_id.to_string()) {
            Some(JiraSyncLease(event_id.to_string()))
        } else {
            None
        }
    }

    struct JiraSyncLease(String);

    impl Drop for JiraSyncLease {
        fn drop(&mut self) {
            if let Ok(mut held) = jira_sync_inflight().lock() {
                held.remove(&self.0);
            }
        }
    }

    async fn process_jira_sync_event(app: &AppHandle, event_id: &str) -> Result<(), String> {
        let state = app.state::<AppState>();
        // Held outbox: leave the event Pending and untouched. A person resuming is
        // what sends it — nothing posts to Jira on its own.
        if work_item_store::report(state.settings())?.paused {
            return Ok(());
        }
        // One POST per event at a time. If a peer task holds the lease, it will do
        // the work; standing down here is not an error.
        let Some(_lease) = claim_jira_sync(event_id) else {
            return Ok(());
        };
        let Some((link, event)) = work_item_store::pending_event(state.settings(), event_id)?
        else {
            return Ok(());
        };
        let (site_id, key) = link.jira_site_and_key();
        let (site, token, kind) = match jira_card_site(state.jira(), Some(site_id)) {
            Ok(site) => site,
            Err(failure) => {
                let refused = zerocode_core::JiraSyncActionState::Refused(failure.message);
                let mut latest = None;
                if event.transition.is_pending() {
                    latest = Some(work_item_store::settle_action(
                        state.settings(),
                        event_id,
                        work_item_store::SyncAction::Transition,
                        refused.clone(),
                        now_epoch_ms(),
                    )?);
                }
                if event.comment.is_pending() {
                    latest = Some(work_item_store::settle_action(
                        state.settings(),
                        event_id,
                        work_item_store::SyncAction::Comment,
                        refused,
                        now_epoch_ms(),
                    )?);
                }
                if let Some(settled) = latest {
                    let _ = app.emit("jira:sync", settled);
                }
                return Ok(());
            }
        };

        if event.transition.is_pending() {
            let transition_id = event
                .transition_id
                .as_deref()
                .ok_or_else(|| "pending Jira transition has no transition id".to_string())?;
            let outcome = match jira_call(JiraCall {
                url: zerocode_core::jira::issue_transitions_url(&site.site_url, kind, key),
                method: JiraMethod::Post,
                body: Some(zerocode_core::jira::transition_body(transition_id)),
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
            .await
            {
                Ok(_) => zerocode_core::JiraSyncActionState::Applied,
                Err(failure) => jira_sync_failure_state(failure),
            };
            let settled = work_item_store::settle_action(
                state.settings(),
                event_id,
                work_item_store::SyncAction::Transition,
                outcome,
                now_epoch_ms(),
            )?;
            let _ = app.emit("jira:sync", settled);
        }

        if event.comment.is_pending() {
            let outcome = match jira_call(JiraCall {
                url: zerocode_core::jira::issue_comment_post_url(&site.site_url, kind, key),
                method: JiraMethod::Post,
                body: Some(zerocode_core::jira::comment_post_body(&jira_sync_comment(
                    &event,
                ))),
                email: &site.email,
                token: &token,
                kind,
                timeout_ms: zerocode_core::jira::SEARCH_TIMEOUT_MS,
            })
            .await
            {
                Ok(_) => zerocode_core::JiraSyncActionState::Applied,
                Err(failure) => jira_sync_failure_state(failure),
            };
            let settled = work_item_store::settle_action(
                state.settings(),
                event_id,
                work_item_store::SyncAction::Comment,
                outcome,
                now_epoch_ms(),
            )?;
            let _ = app.emit("jira:sync", settled);
        }
        Ok(())
    }

    pub const JIRA_CREATE_DESCRIPTION_MAX_BYTES: usize = 32 * 1024;

    pub fn jira_create_issue_url(site_url: &str, kind: zerocode_core::jira::SiteKind) -> String {
        format!("{}/issue", zerocode_core::jira::api_base(site_url, kind))
    }

    pub fn valid_jira_project_key(key: &str) -> bool {
        !key.is_empty()
            && key.len() <= 64
            && key
                .chars()
                .all(|glyph| glyph.is_ascii_alphanumeric() || glyph == '_' || glyph == '-')
    }

    pub fn jira_create_issue_body(
        kind: zerocode_core::jira::SiteKind,
        project_key: &str,
        issue_type: &str,
        summary: &str,
        description: Option<&str>,
    ) -> serde_json::Value {
        let issue_type = if issue_type.chars().all(|glyph| glyph.is_ascii_digit()) {
            serde_json::json!({ "id": issue_type })
        } else {
            serde_json::json!({ "name": issue_type })
        };
        let mut fields = serde_json::Map::from_iter([
            (
                "project".to_string(),
                serde_json::json!({ "key": project_key }),
            ),
            ("issuetype".to_string(), issue_type),
            ("summary".to_string(), serde_json::Value::from(summary)),
        ]);
        if let Some(description) = description.filter(|description| !description.is_empty()) {
            fields.insert(
                "description".to_string(),
                match kind {
                    zerocode_core::jira::SiteKind::Cloud => {
                        zerocode_core::jira::text_as_adf(description)
                    }
                    zerocode_core::jira::SiteKind::Server => serde_json::Value::from(description),
                },
            );
        }
        serde_json::json!({ "fields": fields })
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct JiraCreatedIssue {
        site_id: String,
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        url: String,
    }

    /// What the person typed, or nothing at all.
    ///
    /// Trimmed and otherwise **untouched**: quoting or escaping a JQL here would
    /// refuse queries Jira would have answered, and would do it in a dialect nobody
    /// documented. Jira is the authority on its own grammar, so even text that
    /// looks malformed travels as the first request. Only Jira's own parser `400`
    /// allows the caller to make the separate, visibly labelled text-search retry.
    ///
    /// Whitespace alone is not a question. Jira reads an empty `jql` as "every
    /// issue on this site", so sending one would answer a query nobody asked with
    /// the heaviest request the API has.
    pub fn jira_search_jql(raw: &str) -> Option<&str> {
        let jql = raw.trim();
        (!jql.is_empty()).then_some(jql)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn jira_wire_helpers_keep_auth_query_and_create_rules_together() {
            assert!(
                jira_authorization("", "token", zerocode_core::jira::SiteKind::Server)
                    .starts_with("Bearer ")
            );
            assert!(
                jira_authorization("hana", "token", zerocode_core::jira::SiteKind::Cloud)
                    .starts_with("Basic ")
            );
            assert_eq!(jira_search_jql("  project = OPS  "), Some("project = OPS"));
            assert_eq!(jira_search_jql("  \t"), None);
            assert!(valid_jira_project_key("OPS_2"));
            assert!(!valid_jira_project_key("OPS/2"));
        }

        #[test]
        fn jira_create_payload_uses_the_site_kind_for_description_shape() {
            let cloud = jira_create_issue_body(
                zerocode_core::jira::SiteKind::Cloud,
                "OPS",
                "Task",
                "Ship",
                Some("details"),
            );
            let server = jira_create_issue_body(
                zerocode_core::jira::SiteKind::Server,
                "OPS",
                "10001",
                "Ship",
                Some("details"),
            );
            assert!(cloud["fields"]["description"]["content"].is_array());
            assert_eq!(server["fields"]["description"], "details");
            assert_eq!(
                jira_create_issue_url(
                    "https://acme.atlassian.net/",
                    zerocode_core::jira::SiteKind::Cloud
                ),
                "https://acme.atlassian.net/rest/api/3/issue"
            );
        }

        #[tokio::test]
        async fn search_retries_once_only_for_a_jql_parse_failure() {
            use std::cell::RefCell;
            use std::future::ready;

            let valid_calls = RefCell::new(Vec::new());
            let (valid, valid_fallback) =
                jira_search_with_text_fallback("project = OPS", |query| {
                    valid_calls.borrow_mut().push(query.clone());
                    ready(Ok::<_, zerocode_core::jira::Failure>(query))
                })
                .await;
            assert_eq!(valid.expect("valid JQL"), "project = OPS");
            assert!(!valid_fallback);
            assert_eq!(valid_calls.into_inner(), ["project = OPS"]);

            let text_calls = RefCell::new(Vec::new());
            let (text, text_fallback) = jira_search_with_text_fallback(
                r#"card "quoted" \ lane"#,
                |query| {
                    text_calls.borrow_mut().push(query.clone());
                    let attempt = text_calls.borrow().len();
                    ready(if attempt == 1 {
                        Err(zerocode_core::jira::failure_for_status_redacted(
                            400,
                            r#"{"errorMessages":["Error in the JQL Query: Expecting an operator"]}"#,
                        ))
                    } else {
                        Ok(query)
                    })
                },
            )
            .await;
            assert!(text_fallback);
            assert_eq!(
                text.expect("text retry"),
                r#"text ~ "card \"quoted\" \\ lane*" ORDER BY updated DESC"#
            );
            assert_eq!(text_calls.into_inner().len(), 2);

            for refusal in [
                zerocode_core::jira::failure_for_status(401, ""),
                zerocode_core::jira::offline_failure("network down"),
                zerocode_core::jira::failure_for_status_redacted(
                    400,
                    r#"{"errorMessages":["The project value is unavailable"]}"#,
                ),
            ] {
                let calls = RefCell::new(Vec::new());
                let (answer, fell_back) = jira_search_with_text_fallback("card", |query| {
                    calls.borrow_mut().push(query);
                    ready(Err::<String, _>(refusal.clone()))
                })
                .await;
                assert_eq!(answer.expect_err("refusal stays a refusal"), refusal);
                assert!(!fell_back);
                assert_eq!(calls.into_inner().len(), 1);
            }
        }
    }
}
