//! Linear integration commands and the single authenticated GraphQL boundary.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::State;
use zerocode_shell_state::{AppState, linear_store};

const USER_AGENT: &str = concat!("ZeroCode/", env!("CARGO_PKG_VERSION"));
const CREATE_DESCRIPTION_MAX_BYTES: usize = 32 * 1024;
const COMMENT_MAX_BYTES: usize = 16 * 1024;

pub mod commands {
    use super::*;

    #[tauri::command(async)]
    pub fn linear_status(
        state: State<'_, AppState>,
    ) -> Result<linear_store::LinearStoreStatus, String> {
        state.linear().status().map_err(|error| error.to_string())
    }

    /// Validate the personal API key against `viewer`, collect its teams, then
    /// commit the secret and renderer-safe connection metadata separately.
    #[tauri::command]
    pub async fn linear_connect(
        state: State<'_, AppState>,
        api_key: String,
    ) -> Result<linear_store::LinearStoreStatus, String> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err("Linear API 키를 입력하세요.".to_string());
        }
        let mut after = None;
        let mut connection: Option<zerocode_core::linear::Connection> = None;
        let mut complete = false;
        for _ in 0..zerocode_core::linear::MAX_PAGES {
            let data = linear_call(LinearCall {
                api_key,
                body: zerocode_core::linear::request_body(
                    zerocode_core::linear::VIEWER_QUERY,
                    zerocode_core::linear::page_variables(after.as_deref()),
                ),
                timeout_ms: zerocode_core::linear::CONNECT_TIMEOUT_MS,
            })
            .await
            .map_err(|failure| failure.message)?;
            let (page_connection, page) =
                zerocode_core::linear::read_connection(&data).map_err(|failure| failure.message)?;
            match &mut connection {
                Some(current) => current.teams.extend(page_connection.teams),
                None => connection = Some(page_connection),
            }
            if !page.has_next_page {
                complete = true;
                break;
            }
            after = next_cursor(page).map_err(|failure| failure.message)?;
        }
        if !complete {
            return Err("Linear 팀 목록이 허용된 페이지 수를 초과했습니다.".to_string());
        }
        let mut connection =
            connection.ok_or_else(|| "Linear 연결 정보를 읽지 못했습니다.".to_string())?;
        dedupe_teams(&mut connection.teams);
        state
            .linear()
            .connect_commit(connection.into(), api_key)
            .map_err(|error| error.to_string())
    }

    #[tauri::command(async)]
    pub fn linear_disconnect(
        state: State<'_, AppState>,
    ) -> Result<linear_store::LinearStoreStatus, String> {
        state
            .linear()
            .disconnect()
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub async fn linear_issues(
        state: State<'_, AppState>,
        preset: Option<String>,
        team_id: Option<String>,
    ) -> Result<Vec<zerocode_core::linear::Issue>, zerocode_core::linear::Failure> {
        let preset = preset.as_deref().map_or_else(
            zerocode_core::linear::Preset::default,
            zerocode_core::linear::Preset::from_name,
        );
        let (api_key, connection) = connected(state.linear())?;
        let team_id = team_id.as_deref().or(connection.active_team_id.as_deref());
        issue_pages(
            &api_key,
            IssueAsk::Preset {
                preset,
                team_id,
                user_id: &connection.user_id,
            },
        )
        .await
    }

    #[tauri::command]
    pub async fn linear_search_issues(
        state: State<'_, AppState>,
        query: String,
    ) -> Result<Vec<zerocode_core::linear::Issue>, zerocode_core::linear::Failure> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let (api_key, _) = connected(state.linear())?;
        issue_pages(&api_key, IssueAsk::Search(query)).await
    }

    #[tauri::command]
    pub async fn linear_issue_detail(
        state: State<'_, AppState>,
        id: String,
    ) -> Result<zerocode_core::linear::IssueDetail, zerocode_core::linear::Failure> {
        validate_opaque_id(&id)?;
        let (api_key, _) = connected(state.linear())?;
        let data = linear_call(LinearCall {
            api_key: &api_key,
            body: zerocode_core::linear::request_body(
                zerocode_core::linear::ISSUE_DETAIL_QUERY,
                zerocode_core::linear::detail_variables(&id),
            ),
            timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
        })
        .await?;
        zerocode_core::linear::read_issue_detail(&data)
    }

    #[tauri::command]
    pub async fn linear_issue_comments(
        state: State<'_, AppState>,
        id: String,
    ) -> Result<Vec<zerocode_core::linear::IssueComment>, zerocode_core::linear::Failure> {
        validate_opaque_id(&id)?;
        let (api_key, _) = connected(state.linear())?;
        let mut after = None;
        let mut comments = Vec::new();
        for _ in 0..zerocode_core::linear::MAX_PAGES {
            let data = linear_call(LinearCall {
                api_key: &api_key,
                body: zerocode_core::linear::request_body(
                    zerocode_core::linear::COMMENTS_QUERY,
                    zerocode_core::linear::comment_variables(&id, after.as_deref()),
                ),
                timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
            })
            .await?;
            let (mut page_comments, page) = zerocode_core::linear::read_comments(&data)?;
            comments.append(&mut page_comments);
            if !page.has_next_page {
                return Ok(comments);
            }
            after = next_cursor(page)?;
        }
        Err(page_limit_failure("댓글"))
    }

    #[tauri::command]
    pub async fn linear_comment_issue(
        state: State<'_, AppState>,
        id: String,
        body: String,
    ) -> Result<zerocode_core::linear::IssueComment, zerocode_core::linear::Failure> {
        validate_opaque_id(&id)?;
        let body = body.trim();
        if body.is_empty() || body.len() > COMMENT_MAX_BYTES {
            return Err(input_failure("Linear 댓글은 1~16384바이트여야 합니다."));
        }
        let (api_key, _) = connected(state.linear())?;
        let data = linear_call(LinearCall {
            api_key: &api_key,
            body: zerocode_core::linear::request_body(
                zerocode_core::linear::CREATE_COMMENT_MUTATION,
                zerocode_core::linear::create_comment_variables(&id, body),
            ),
            timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
        })
        .await?;
        zerocode_core::linear::read_created_comment(&data)
    }

    #[tauri::command]
    pub async fn linear_create_issue(
        state: State<'_, AppState>,
        team: String,
        title: String,
        desc: Option<String>,
    ) -> Result<zerocode_core::linear::CreatedIssue, zerocode_core::linear::Failure> {
        validate_opaque_id(&team)?;
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 255 {
            return Err(input_failure("Linear 이슈 제목은 1~255자여야 합니다."));
        }
        let description = desc
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if description.is_some_and(|value| value.len() > CREATE_DESCRIPTION_MAX_BYTES) {
            return Err(input_failure("Linear 이슈 설명이 너무 깁니다."));
        }
        let (api_key, connection) = connected(state.linear())?;
        if !connection.teams.iter().any(|held| held.id == team) {
            return Err(input_failure("연결된 Linear 팀이 아닙니다."));
        }
        let data = linear_call(LinearCall {
            api_key: &api_key,
            body: zerocode_core::linear::request_body(
                zerocode_core::linear::CREATE_ISSUE_MUTATION,
                zerocode_core::linear::create_issue_variables(&team, title, description),
            ),
            timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
        })
        .await?;
        zerocode_core::linear::read_created_issue(&data)
    }

    #[tauri::command]
    pub async fn linear_issue_options(
        state: State<'_, AppState>,
        team_id: Option<String>,
    ) -> Result<zerocode_core::linear::IssueOptions, zerocode_core::linear::Failure> {
        let (api_key, connection) = connected(state.linear())?;
        let team_id = team_id.as_deref().or(connection.active_team_id.as_deref());
        if let Some(id) = team_id {
            validate_opaque_id(id)?;
        }
        let mut after = None;
        let mut options: Option<zerocode_core::linear::IssueOptions> = None;
        for _ in 0..zerocode_core::linear::MAX_PAGES {
            let data = linear_call(LinearCall {
                api_key: &api_key,
                body: zerocode_core::linear::request_body(
                    zerocode_core::linear::OPTIONS_QUERY,
                    zerocode_core::linear::options_variables(team_id, after.as_deref()),
                ),
                timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
            })
            .await?;
            let (page_options, page) = zerocode_core::linear::read_options(&data)?;
            match &mut options {
                Some(current) => current.teams.extend(page_options.teams),
                None => options = Some(page_options),
            }
            if !page.has_next_page {
                let mut options = options.unwrap_or(zerocode_core::linear::IssueOptions {
                    teams: Vec::new(),
                    states: Vec::new(),
                    priorities: zerocode_core::linear::PRIORITIES.to_vec(),
                });
                dedupe_teams(&mut options.teams);
                return Ok(options);
            }
            after = next_cursor(page)?;
        }
        Err(page_limit_failure("옵션"))
    }

    #[tauri::command]
    pub async fn linear_agent_context(
        state: State<'_, AppState>,
        id: String,
    ) -> Result<LinearAgentContextReport, zerocode_core::linear::Failure> {
        let (detail, comments) = tokio_join_detail_comments(state.linear(), &id).await?;
        let prompt = zerocode_core::linear::agent_context(&detail, &comments);
        Ok(LinearAgentContextReport {
            bytes: prompt.len(),
            title: detail.head.title,
            status: detail.head.status,
            comment_count: comments.len(),
            prompt,
        })
    }

    #[derive(Serialize)]
    pub struct LinearAgentContextReport {
        prompt: String,
        title: String,
        status: String,
        comment_count: usize,
        bytes: usize,
    }

    enum IssueAsk<'a> {
        Preset {
            preset: zerocode_core::linear::Preset,
            team_id: Option<&'a str>,
            user_id: &'a str,
        },
        Search(&'a str),
    }

    async fn issue_pages(
        api_key: &str,
        ask: IssueAsk<'_>,
    ) -> Result<Vec<zerocode_core::linear::Issue>, zerocode_core::linear::Failure> {
        let mut after = None;
        let mut issues = Vec::new();
        for _ in 0..zerocode_core::linear::MAX_PAGES {
            let (query, variables) = match ask {
                IssueAsk::Preset {
                    preset,
                    team_id,
                    user_id,
                } => (
                    zerocode_core::linear::ISSUES_QUERY,
                    zerocode_core::linear::issue_variables(
                        preset,
                        team_id,
                        user_id,
                        after.as_deref(),
                    ),
                ),
                IssueAsk::Search(query) => (
                    zerocode_core::linear::SEARCH_QUERY,
                    zerocode_core::linear::search_variables(query, after.as_deref()),
                ),
            };
            let data = linear_call(LinearCall {
                api_key,
                body: zerocode_core::linear::request_body(query, variables),
                timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
            })
            .await?;
            let connection = match ask {
                IssueAsk::Preset { .. } => data.get("issues"),
                IssueAsk::Search(_) => data.get("issueSearch"),
            }
            .ok_or_else(malformed_failure)?;
            let (mut page_issues, page) = zerocode_core::linear::read_issues(connection);
            issues.append(&mut page_issues);
            if !page.has_next_page {
                return Ok(issues);
            }
            after = next_cursor(page)?;
        }
        Err(page_limit_failure("이슈"))
    }

    async fn tokio_join_detail_comments(
        store: &linear_store::LinearStore,
        id: &str,
    ) -> Result<
        (
            zerocode_core::linear::IssueDetail,
            Vec<zerocode_core::linear::IssueComment>,
        ),
        zerocode_core::linear::Failure,
    > {
        validate_opaque_id(id)?;
        let (api_key, _) = connected(store)?;
        let detail = linear_call(LinearCall {
            api_key: &api_key,
            body: zerocode_core::linear::request_body(
                zerocode_core::linear::ISSUE_DETAIL_QUERY,
                zerocode_core::linear::detail_variables(id),
            ),
            timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
        });
        let comments = async {
            let mut after = None;
            let mut all = Vec::new();
            for _ in 0..zerocode_core::linear::MAX_PAGES {
                let data = linear_call(LinearCall {
                    api_key: &api_key,
                    body: zerocode_core::linear::request_body(
                        zerocode_core::linear::COMMENTS_QUERY,
                        zerocode_core::linear::comment_variables(id, after.as_deref()),
                    ),
                    timeout_ms: zerocode_core::linear::REQUEST_TIMEOUT_MS,
                })
                .await?;
                let (mut rows, page) = zerocode_core::linear::read_comments(&data)?;
                all.append(&mut rows);
                if !page.has_next_page {
                    return Ok(all);
                }
                after = next_cursor(page)?;
            }
            Err(page_limit_failure("댓글"))
        };
        let (detail, comments) = tokio::join!(detail, comments);
        Ok((
            zerocode_core::linear::read_issue_detail(&detail?)?,
            comments?,
        ))
    }

    fn connected(
        store: &linear_store::LinearStore,
    ) -> Result<
        (zeroize::Zeroizing<String>, linear_store::LinearConnection),
        zerocode_core::linear::Failure,
    > {
        let connection = store
            .load()
            .map_err(|error| input_failure(&error.to_string()))?
            .ok_or_else(zerocode_core::linear::disconnected_failure)?;
        let api_key = store
            .read_api_key()
            .map_err(|error| zerocode_core::linear::Failure {
                kind: zerocode_core::linear::FailureKind::Auth,
                message: error.to_string(),
                retry_after_seconds: None,
            })?;
        Ok((api_key, connection))
    }

    fn validate_opaque_id(id: &str) -> Result<(), zerocode_core::linear::Failure> {
        if id.is_empty()
            || id.len() > 256
            || id
                .chars()
                .any(|glyph| glyph.is_control() || glyph.is_whitespace())
        {
            return Err(input_failure("Linear 이슈 또는 팀 ID가 올바르지 않습니다."));
        }
        Ok(())
    }

    fn next_cursor(
        page: zerocode_core::linear::PageInfo,
    ) -> Result<Option<String>, zerocode_core::linear::Failure> {
        if page.has_next_page && page.end_cursor.as_deref().is_none_or(str::is_empty) {
            return Err(input_failure("Linear 페이지 커서가 비어 있습니다."));
        }
        Ok(page.end_cursor)
    }

    fn dedupe_teams(teams: &mut Vec<zerocode_core::linear::Team>) {
        let mut seen = std::collections::HashSet::new();
        teams.retain(|team| seen.insert(team.id.clone()));
    }

    fn input_failure(message: &str) -> zerocode_core::linear::Failure {
        zerocode_core::linear::Failure {
            kind: zerocode_core::linear::FailureKind::Unknown,
            message: message.to_string(),
            retry_after_seconds: None,
        }
    }

    fn malformed_failure() -> zerocode_core::linear::Failure {
        input_failure("Linear 응답을 읽을 수 없습니다.")
    }

    fn page_limit_failure(subject: &str) -> zerocode_core::linear::Failure {
        input_failure(&format!(
            "Linear {subject} 목록이 허용된 페이지 수를 초과했습니다."
        ))
    }

    struct LinearCall<'a> {
        api_key: &'a str,
        body: Value,
        timeout_ms: u64,
    }

    fn linear_client(timeout_ms: u64) -> Result<reqwest::Client, zerocode_core::linear::Failure> {
        reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| zerocode_core::linear::offline_failure())
    }

    /// The only outbound Linear request. It owns the fixed endpoint, timeout,
    /// authorization header, response bound, and HTTP/GraphQL failure mapping.
    async fn linear_call(call: LinearCall<'_>) -> Result<Value, zerocode_core::linear::Failure> {
        let answer = linear_client(call.timeout_ms)?
            .post(zerocode_core::linear::GRAPHQL_ENDPOINT)
            .header("accept", "application/json")
            .header("authorization", call.api_key)
            .json(&call.body)
            .send()
            .await
            .map_err(|_| zerocode_core::linear::offline_failure())?;
        let status = answer.status().as_u16();
        let retry_after = retry_after_seconds(answer.headers());
        let successful = (200..300).contains(&status);
        let limit = if successful {
            zerocode_core::linear::RESPONSE_LIMIT_BYTES
        } else {
            zerocode_core::linear::ERROR_LIMIT_BYTES
        };
        let text = read_response_text(answer, limit).await?;
        let envelope = serde_json::from_str::<Value>(&text).ok();
        if !successful {
            if let Some(envelope) = &envelope
                && let Err(mut failure) = zerocode_core::linear::graphql_data(envelope)
                && matches!(
                    failure.kind,
                    zerocode_core::linear::FailureKind::RateLimited
                        | zerocode_core::linear::FailureKind::Auth
                )
            {
                failure.retry_after_seconds = retry_after;
                return Err(failure);
            }
            return Err(zerocode_core::linear::failure_for_status(
                status,
                retry_after,
            ));
        }
        let envelope = envelope.ok_or_else(malformed_failure)?;
        zerocode_core::linear::graphql_data(&envelope).cloned()
    }

    fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
        if let Some(seconds) = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
        {
            return Some(seconds);
        }
        let reset_ms: u128 = [
            "x-ratelimit-endpoint-requests-reset",
            "x-ratelimit-requests-reset",
            "x-ratelimit-complexity-reset",
        ]
        .into_iter()
        .find_map(|name| headers.get(name))?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        let remaining_ms = reset_ms.saturating_sub(now_ms);
        Some(remaining_ms.div_ceil(1_000).min(u64::MAX as u128) as u64)
    }

    async fn drain_response(mut response: reqwest::Response, limit: usize) {
        let mut remaining = limit;
        while remaining > 0 {
            let Ok(Some(chunk)) = response.chunk().await else {
                break;
            };
            remaining = remaining.saturating_sub(chunk.len());
        }
    }

    async fn read_response_text(
        mut response: reqwest::Response,
        limit: usize,
    ) -> Result<String, zerocode_core::linear::Failure> {
        let mut body = Vec::with_capacity(limit.min(8 * 1024));
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| zerocode_core::linear::offline_failure())?
        {
            if body.len().saturating_add(chunk.len()) > limit {
                drain_response(response, zerocode_core::linear::ERROR_LIMIT_BYTES).await;
                return Err(input_failure("Linear 응답이 허용 크기를 초과했습니다."));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn retry_after_reads_linear_rate_limit_header() {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(reqwest::header::RETRY_AFTER, "23".parse().expect("header"));
            assert_eq!(retry_after_seconds(&headers), Some(23));
        }

        #[test]
        fn opaque_id_rejects_graphql_shaped_input() {
            let failure = validate_opaque_id("issue } mutation {").expect_err("invalid id");
            assert_eq!(failure.kind, zerocode_core::linear::FailureKind::Unknown);
        }
    }
}
