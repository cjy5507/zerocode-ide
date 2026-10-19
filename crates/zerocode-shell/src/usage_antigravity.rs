//! Antigravity's plan figures over Google's Code Assist quota API.
//!
//! The only credential source is this machine's Google login, which zo and
//! this window share: [`crate::google_login`] owns the file, the key, the
//! OAuth client and the refresh, and this module is the quota reader on top
//! of it. A refreshed access token lives only for this scan — the login on
//! disk is written by the settings card and by zo, never by a gauge.

use std::path::Path;

use crate::google_login;
use crate::usage::{NamedWindow, ProviderUsage, UsageWindow};
use crate::usage_http;
use zerocode_core::civil::epoch_ms_of_iso;
use zerocode_core::usage_limit;

const LOAD_CODE_ASSIST_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const RETRIEVE_QUOTA_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const ANTIGRAVITY_VERSION: &str = "2.1.4";
const ANTIGRAVITY_API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";
const ANTIGRAVITY_CLIENT_METADATA: &str =
    r#"{"ideType":"ANTIGRAVITY","platform":"DARWIN_ARM64","pluginType":"GEMINI"}"#;
const BUCKET_WINDOW_MINUTES: u32 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
struct AntigravityHeaders {
    user_agent: String,
}

impl AntigravityHeaders {
    fn new() -> Self {
        Self {
            user_agent: format!("antigravity/{ANTIGRAVITY_VERSION} darwin/arm64"),
        }
    }

    fn pairs(&self) -> [(&str, &str); 3] {
        [
            ("User-Agent", &self.user_agent),
            ("X-Goog-Api-Client", ANTIGRAVITY_API_CLIENT),
            ("Client-Metadata", ANTIGRAVITY_CLIENT_METADATA),
        ]
    }
}

/// The seam the tests replace: everything this reader says to a server.
///
/// A refresh is named here rather than called inline because the fake wire is
/// how the 60-second margin and the retry-once rule are measured without a
/// network — the refresh itself is [`google_login::refreshed_access_token`],
/// which is also what the settings card renews with.
trait Wire {
    fn refresh(&mut self, refresh_token: &str, now: i64) -> Option<String>;

    fn post_json(
        &mut self,
        url: &str,
        access_token: &str,
        headers: &AntigravityHeaders,
        body: &serde_json::Value,
        now: i64,
    ) -> Result<serde_json::Value, usage_http::Failure>;
}

struct LiveWire;

impl Wire for LiveWire {
    fn refresh(&mut self, refresh_token: &str, now: i64) -> Option<String> {
        google_login::refreshed_access_token(refresh_token, now)
    }

    fn post_json(
        &mut self,
        url: &str,
        access_token: &str,
        headers: &AntigravityHeaders,
        body: &serde_json::Value,
        now: i64,
    ) -> Result<serde_json::Value, usage_http::Failure> {
        usage_http::post_json_bearer(url, access_token, &headers.pairs(), body, now)
    }
}

/// The Code Assist project this login's quota is billed to.
fn load_project_id(
    wire: &mut impl Wire,
    access_token: &str,
    headers: &AntigravityHeaders,
    now: i64,
) -> Result<String, String> {
    let body = wire
        .post_json(
            LOAD_CODE_ASSIST_URL,
            access_token,
            headers,
            &serde_json::json!({
                "metadata": {
                    "ideType": "ANTIGRAVITY",
                    "platform": "DARWIN_ARM64",
                    "pluginType": "GEMINI"
                }
            }),
            now,
        )
        .map_err(|failure| {
            format!(
                "Antigravity 프로젝트 ID를 읽지 못했습니다 ({})",
                failure.message
            )
        })?;
    body.get("cloudaicompanionProject")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "Antigravity 프로젝트 ID가 응답에 없습니다".to_string())
}

/// One quota bucket as the API speaks it, shape-checked the way Orca
/// shape-checks (`isQuotaBucket`) — a malformed row is dropped, not a crash.
struct QuotaBucket {
    remaining_fraction: f64,
    reset_time: String,
    model_id: String,
}

fn parse_quota_response(data: &serde_json::Value) -> Vec<QuotaBucket> {
    let rows = if let Some(rows) = data.as_array() {
        rows.as_slice()
    } else if let Some(rows) = data.get("buckets").and_then(serde_json::Value::as_array) {
        rows.as_slice()
    } else {
        &[]
    };
    rows.iter()
        .filter_map(|row| {
            let fraction = row.get("remainingFraction")?.as_f64()?;
            if !fraction.is_finite() {
                return None;
            }
            Some(QuotaBucket {
                remaining_fraction: fraction,
                reset_time: row.get("resetTime")?.as_str()?.to_string(),
                model_id: row.get("modelId")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/* ---- bucket dressing ---------------------------------------------------- */

/// The model ids Orca names by hand; anything else is humanized from its id.
const MODEL_BUCKET_NAMES: &[(&str, &str)] = &[
    ("gemini-3.1-pro", "3.1 Pro"),
    ("gemini-3.1-flash", "3.1 Flash"),
    ("gemini-3.1-flash-lite", "3.1 Flash Lite"),
    ("gemini-3.0-pro", "3.0 Pro"),
    ("gemini-3.0-flash", "3.0 Flash"),
    ("gemini-2.5-pro", "Pro"),
    ("gemini-2.5-flash", "Flash"),
    ("gemini-2.5-flash-lite", "Flash Lite"),
    ("gemini-2.0-pro", "2.0 Pro"),
    ("gemini-2.0-flash", "2.0 Flash"),
    ("gemini-2.0-flash-lite", "2.0 Flash Lite"),
    ("gemini-1.5-pro", "1.5 Pro"),
    ("gemini-1.5-flash", "1.5 Flash"),
    ("gemini-exp", "Exp"),
    ("gemini-experimental", "Exp"),
];

fn named_bucket(model_id: &str) -> Option<&'static str> {
    MODEL_BUCKET_NAMES
        .iter()
        .find(|(id, _)| *id == model_id)
        .map(|(_, name)| *name)
}

/// `gemini-2.5-something-new` → `2.5 Something New`.
fn humanize_model_id(model_id: &str) -> String {
    let without = model_id
        .strip_prefix("gemini-")
        .or_else(|| model_id.strip_prefix("Gemini-"))
        .unwrap_or(model_id);
    without
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn bucket_name(model_id: &str) -> String {
    named_bucket(model_id).map_or_else(|| humanize_model_id(model_id), str::to_string)
}

fn build_bucket(bucket: &QuotaBucket) -> NamedWindow {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let used_percent =
        (((1.0 - bucket.remaining_fraction) * 100.0).round()).clamp(0.0, 100.0) as u8;
    NamedWindow {
        name: bucket_name(&bucket.model_id),
        window: UsageWindow {
            used_percent,
            window_minutes: BUCKET_WINDOW_MINUTES,
            resets_at: epoch_ms_of_iso(&bucket.reset_time),
            reset_description: None,
        },
    }
}

/// Same percent, same reset: one row. The known-model name wins over the
/// derived one; between equals the shorter name stays (`deduplicateBuckets`).
fn deduplicate_buckets(buckets: Vec<(NamedWindow, String)>) -> Vec<NamedWindow> {
    let mut kept: Vec<(NamedWindow, String)> = Vec::new();
    for (window, model_id) in buckets {
        let key_of = |held: &NamedWindow| (held.window.used_percent, held.window.resets_at);
        let key = key_of(&window);
        match kept.iter_mut().find(|(held, _)| key_of(held) == key) {
            None => kept.push((window, model_id)),
            Some((held, held_model)) => {
                let held_known = named_bucket(held_model).is_some();
                let new_known = named_bucket(&model_id).is_some();
                if (new_known && !held_known)
                    || (new_known == held_known && window.name.len() < held.name.len())
                {
                    *held = window;
                    *held_model = model_id;
                }
            }
        }
    }
    kept.into_iter().map(|(window, _)| window).collect()
}

/// The segment's one figure: the most constrained bucket, nameless
/// (`deriveSessionSummary`).
fn session_summary(buckets: &[NamedWindow]) -> Option<UsageWindow> {
    buckets
        .iter()
        .max_by_key(|held| held.window.used_percent)
        .map(|held| held.window.clone())
}

/* ---- the fetch, assembled ----------------------------------------------- */

fn answer(status: &str, error: Option<String>, now: i64) -> ProviderUsage {
    ProviderUsage {
        provider: "antigravity".to_string(),
        session: None,
        weekly: None,
        fable_weekly: None,
        monthly: None,
        buckets: None,
        updated_at: now,
        error,
        status: status.to_string(),
        failure_kind: None,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: None,
    }
}

fn fetch_quota(
    wire: &mut impl Wire,
    access_token: &str,
    project_id: &str,
    headers: &AntigravityHeaders,
    now: i64,
) -> ProviderUsage {
    match wire.post_json(
        RETRIEVE_QUOTA_URL,
        access_token,
        headers,
        &serde_json::json!({ "project": project_id }),
        now,
    ) {
        // The KIND rides back on the answer. The refresh road below used to
        // find its 401 by reading `"HTTP 401"` back out of this sentence —
        // control flow through a formatted string, which stops working the
        // day the sentence is translated or a server puts a 401 in its prose.
        Err(failure) => ProviderUsage {
            failure_kind: Some(failure.recovery.kind),
            retry_at_ms: failure.retry_at_ms,
            ..answer(
                "error",
                Some(format!("Quota fetch failed ({})", failure.message)),
                now,
            )
        },
        Ok(body) => {
            let buckets = deduplicate_buckets(
                parse_quota_response(&body)
                    .iter()
                    .map(|bucket| (build_bucket(bucket), bucket.model_id.clone()))
                    .collect(),
            );
            ProviderUsage {
                provider: "antigravity".to_string(),
                session: session_summary(&buckets),
                weekly: None,
                fable_weekly: None,
                monthly: None,
                buckets: Some(buckets),
                updated_at: now,
                error: None,
                status: "ok".to_string(),
                failure_kind: None,
                retry_at_ms: None,
                plan_type: None,
                reset_credits: None,
                account: None,
            }
        }
    }
}

/// A 401 out of the quota call means the token died between the read and the
/// ask: refresh once and ask once more, exactly once.
fn retry_after_refresh(
    wire: &mut impl Wire,
    result: ProviderUsage,
    refresh_token: Option<&str>,
    fallback_project: &str,
    headers: &AntigravityHeaders,
    now: i64,
) -> ProviderUsage {
    // A dead token, said as a kind rather than sniffed out of a sentence.
    let unauthorized = result.status == "error"
        && result.failure_kind == Some(usage_limit::FailureKind::StaleToken);
    if !unauthorized {
        return result;
    }
    let Some(refresh_token) = refresh_token.filter(|token| !token.is_empty()) else {
        return result;
    };
    let Some(refreshed) = wire.refresh(refresh_token, now) else {
        return result;
    };
    let project = load_project_id(wire, &refreshed, headers, now)
        .unwrap_or_else(|_| fallback_project.to_string());
    if project.is_empty() {
        return result;
    }
    fetch_quota(wire, &refreshed, &project, headers, now)
}

/// No login on file at all.
///
/// The sentence names the surface that FIXES it. It used to name a zo command
/// in a terminal, which sent a person out of the window for something the
/// settings pane now does — and the status bar shows this sentence as the
/// segment's tooltip, so it is the only instruction most people ever see.
fn signed_out(now: i64) -> ProviderUsage {
    answer(
        "signed_out",
        Some("Antigravity 로그인 필요 — 설정 > AI 제공자 계정".to_string()),
        now,
    )
}

/// A token this scan can use, renewed through the wire when it is spent. The
/// margin itself is [`google_login::needs_refresh`] — shared with the card,
/// which renews the same login on its own road.
fn access_token(
    wire: &mut impl Wire,
    credentials: &google_login::Credentials,
    now: i64,
) -> Option<String> {
    if !google_login::needs_refresh(credentials, now) {
        return Some(credentials.access_token.clone());
    }
    wire.refresh(credentials.refresh_token.as_deref()?, now)
}

fn scan_with(path: Option<&Path>, now: i64, wire: &mut impl Wire) -> ProviderUsage {
    let Some(path) = path else {
        return signed_out(now);
    };
    let credentials = match google_login::read(path) {
        Ok(Some(credentials)) => credentials,
        Ok(None) => return signed_out(now),
        Err(error) => return answer("error", Some(error), now),
    };
    let Some(access_token) = access_token(wire, &credentials, now) else {
        return answer(
            "error",
            Some("Antigravity Google 토큰을 갱신하지 못했습니다".to_string()),
            now,
        );
    };
    let headers = AntigravityHeaders::new();
    let project = match load_project_id(wire, &access_token, &headers, now) {
        Ok(project) => project,
        Err(error) => return answer("error", Some(error), now),
    };
    let result = fetch_quota(wire, &access_token, &project, &headers, now);
    retry_after_refresh(
        wire,
        result,
        credentials.refresh_token.as_deref(),
        &project,
        &headers,
        now,
    )
}

/// Read this machine's Google login and fetch Antigravity quota on the
/// caller's clock.
pub fn scan(now: i64) -> ProviderUsage {
    let path = google_login::credentials_path();
    scan_with(path.as_deref(), now, &mut LiveWire)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bucket arithmetic, Orca's own: percent from the remaining
    /// fraction (rounded, clamped), an hour-wide window, the reset off the
    /// ISO stamp — and a stamp that does not parse keeps the row.
    #[test]
    fn a_bucket_is_dressed_from_the_remaining_fraction() {
        let dressed = build_bucket(&QuotaBucket {
            remaining_fraction: 0.25,
            reset_time: "2026-08-18T03:00:00Z".to_string(),
            model_id: "gemini-2.5-pro".to_string(),
        });
        assert_eq!(dressed.name, "Pro");
        assert_eq!(dressed.window.used_percent, 75);
        assert_eq!(dressed.window.window_minutes, 60);
        assert!(dressed.window.resets_at.is_some());

        let unstamped = build_bucket(&QuotaBucket {
            remaining_fraction: 1.0,
            reset_time: "sometime".to_string(),
            model_id: "gemini-9.9-new-thing".to_string(),
        });
        assert_eq!(unstamped.name, "9.9 New Thing", "the humanizer moved");
        assert_eq!(unstamped.window.used_percent, 0);
        assert_eq!(unstamped.window.resets_at, None);
    }

    /// Same percent and same reset are one row, the known-model name winning
    /// over the derived one, the shorter between equals — Orca's
    /// `deduplicateBuckets`, decision for decision.
    #[test]
    fn duplicate_buckets_collapse_and_the_known_name_wins() {
        let stamp = Some(1_755_500_000_000);
        let window = |name: &str| NamedWindow {
            name: name.to_string(),
            window: UsageWindow {
                used_percent: 40,
                window_minutes: 60,
                resets_at: stamp,
                reset_description: None,
            },
        };
        let kept = deduplicate_buckets(vec![
            (
                window("2.5 Preview Something"),
                "gemini-2.5-preview-something".to_string(),
            ),
            (window("Pro"), "gemini-2.5-pro".to_string()),
            (window("Flash"), "gemini-2.5-flash".to_string()),
        ]);
        assert_eq!(kept.len(), 1, "equal readings kept separate rows");
        // The known-name rule fires before the shorter-name rule ever could:
        // `Pro` replaced the derived row, and `Flash` (also known, longer)
        // then failed the equal-tier length test.
        assert_eq!(kept[0].name, "Pro");

        // Different resets are different rows even at the same percent.
        let mut other = window("Flash");
        other.window.resets_at = Some(1_755_500_000_001);
        let two = deduplicate_buckets(vec![
            (window("Pro"), "gemini-2.5-pro".to_string()),
            (other, "gemini-2.5-flash".to_string()),
        ]);
        assert_eq!(two.len(), 2);
    }

    /// The segment shows the most constrained bucket, nameless.
    #[test]
    fn the_session_summary_is_the_most_constrained_bucket() {
        let bucket = |name: &str, used: u8| NamedWindow {
            name: name.to_string(),
            window: UsageWindow {
                used_percent: used,
                window_minutes: 60,
                resets_at: None,
                reset_description: None,
            },
        };
        let summary =
            session_summary(&[bucket("Pro", 30), bucket("Flash", 80), bucket("Lite", 10)])
                .expect("summary");
        assert_eq!(summary.used_percent, 80);
        assert_eq!(session_summary(&[]), None);
    }

    struct PostedCall {
        url: String,
        access_token: String,
        headers: Vec<(String, String)>,
    }

    struct FakeWire {
        refreshes: usize,
        posts: Vec<PostedCall>,
    }

    impl Wire for FakeWire {
        fn refresh(&mut self, _refresh_token: &str, _now: i64) -> Option<String> {
            self.refreshes += 1;
            Some("fresh-access".to_string())
        }

        fn post_json(
            &mut self,
            url: &str,
            access_token: &str,
            headers: &AntigravityHeaders,
            _body: &serde_json::Value,
            _now: i64,
        ) -> Result<serde_json::Value, usage_http::Failure> {
            self.posts.push(PostedCall {
                url: url.to_string(),
                access_token: access_token.to_string(),
                headers: headers
                    .pairs()
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
            });
            if url == LOAD_CODE_ASSIST_URL {
                Ok(serde_json::json!({ "cloudaicompanionProject": "project-a" }))
            } else {
                Ok(serde_json::json!({ "buckets": [{
                    "remainingFraction": 0.5,
                    "resetTime": "2026-08-18T00:00:00Z",
                    "modelId": "gemini-2.5-pro"
                }] }))
            }
        }
    }

    fn fake_wire() -> FakeWire {
        FakeWire {
            refreshes: 0,
            posts: Vec::new(),
        }
    }

    #[test]
    fn missing_zo_google_login_is_signed_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut wire = fake_wire();
        let held = scan_with(Some(&dir.path().join("credentials.json")), 7, &mut wire);
        assert_eq!(held.provider, "antigravity");
        assert_eq!(held.status, "signed_out");
        // And the sentence points at the surface that fixes it, because this
        // is what the status bar shows on hover.
        assert_eq!(
            held.error.as_deref(),
            Some("Antigravity 로그인 필요 — 설정 > AI 제공자 계정")
        );
        assert_eq!(wire.refreshes, 0);
        assert!(wire.posts.is_empty());
    }

    #[test]
    fn an_expiring_token_refreshes_with_antigravity_headers_without_writing_zo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        let original = r#"{
  "kept": true,
  "google_code_assist_oauth": {
    "accessToken": "old-access",
    "refreshToken": "refresh-me",
    "expiresAt": 1800000030
  }
}"#;
        std::fs::write(&path, original).expect("credentials");
        let mut wire = fake_wire();

        let held = scan_with(Some(&path), 1_800_000_000_000, &mut wire);

        assert_eq!(held.status, "ok", "{:?}", held.error);
        assert_eq!(wire.refreshes, 1, "the 60-second refresh margin moved");
        assert_eq!(wire.posts.len(), 2);
        assert!(
            wire.posts
                .iter()
                .all(|request| request.access_token == "fresh-access")
        );
        let expected_headers = vec![
            (
                "User-Agent".to_string(),
                format!("antigravity/{ANTIGRAVITY_VERSION} darwin/arm64"),
            ),
            (
                "X-Goog-Api-Client".to_string(),
                ANTIGRAVITY_API_CLIENT.to_string(),
            ),
            (
                "Client-Metadata".to_string(),
                ANTIGRAVITY_CLIENT_METADATA.to_string(),
            ),
        ];
        assert!(
            wire.posts
                .iter()
                .all(|request| request.headers == expected_headers)
        );
        assert_eq!(wire.posts[0].url, LOAD_CODE_ASSIST_URL);
        assert_eq!(wire.posts[1].url, RETRIEVE_QUOTA_URL);
        assert_eq!(
            std::fs::read_to_string(path).expect("credentials after scan"),
            original,
            "the window wrote its refreshed access token into zo's store"
        );
    }

    /// The quota response reader takes both shapes the API answers with and
    /// drops malformed rows instead of failing the batch.
    #[test]
    fn the_quota_reader_takes_both_shapes_and_drops_broken_rows() {
        let wrapped: serde_json::Value = serde_json::json!({ "buckets": [
            { "remainingFraction": 0.5, "resetTime": "2026-08-18T00:00:00Z", "modelId": "gemini-2.5-pro" },
            { "remainingFraction": "not-a-number", "resetTime": "x", "modelId": "y" },
        ] });
        assert_eq!(parse_quota_response(&wrapped).len(), 1);
        let bare: serde_json::Value = serde_json::json!([
            { "remainingFraction": 0.1, "resetTime": "2026-08-18T00:00:00Z", "modelId": "gemini-2.5-flash" },
        ]);
        assert_eq!(parse_quota_response(&bare).len(), 1);
        assert_eq!(parse_quota_response(&serde_json::json!({})).len(), 0);
    }
}
