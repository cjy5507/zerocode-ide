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

const LOAD_CODE_ASSIST_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const RETRIEVE_QUOTA_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
/// The two method names, as a refusal names the call it came back from.
const LOAD_CODE_ASSIST: &str = "loadCodeAssist";
const RETRIEVE_USER_QUOTA: &str = "retrieveUserQuota";
const BUCKET_WINDOW_MINUTES: u32 = 60;

/// The headers of every Code Assist call. They name this client and nothing
/// else: its own User-Agent, and no Antigravity, VS Code or Cloud Shell identity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AntigravityHeaders {
    user_agent: String,
}

impl AntigravityHeaders {
    fn new() -> Self {
        Self {
            user_agent: concat!("zerocode/", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }

    fn pairs(&self) -> [(&str, &str); 1] {
        [("User-Agent", &self.user_agent)]
    }
}

/// The seam the tests replace: everything this reader says to a server.
///
/// A refresh is named here rather than called inline because the fake wire is
/// how the 60-second margin is measured without a network — the refresh itself
/// is [`google_login::refreshed_access_token`], which is also what the settings
/// card renews with.
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

/// The closed words of a refusal, for the window's log: the status Google
/// answered with and the call it refused. Never its body, never a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) status: u16,
    pub(crate) call: &'static str,
}

/// Google refuses a call it will not serve with 400, 401 or 403. Anything else
/// is a failure with a sentence, not a refusal.
fn refusal_of(failure: &usage_http::Failure, call: &'static str) -> Option<Refusal> {
    match failure.http_status {
        Some(status @ (400 | 401 | 403)) => Some(Refusal { status, call }),
        _ => None,
    }
}

/// Where a read stopped short of its figures.
enum Stop {
    /// Google refused a call. The gauge says it could not verify the quota.
    Refused(Refusal),
    /// The call failed for another reason, and this is the answer that says so.
    Failed(Box<ProviderUsage>),
}

impl Stop {
    fn into_read(self, now: i64) -> (ProviderUsage, Option<Refusal>) {
        match self {
            Self::Refused(refusal) => (refused_answer(now), Some(refusal)),
            Self::Failed(usage) => (*usage, None),
        }
    }
}

/// Google refused a call this client made. The figures cannot be verified, and
/// no sentence names a fix a person can make, so the window does not ask again.
fn refused_answer(now: i64) -> ProviderUsage {
    answer(
        "refused",
        Some("Antigravity 사용량을 확인할 수 없습니다 — Google이 요청을 거절했습니다".to_string()),
        now,
    )
}

/// The Code Assist project this login's quota is billed to. The client metadata
/// names no IDE and no platform: every field is optional in Google's public
/// schema, and the unspecified values are the ones its own CLI sends outside an
/// IDE. `pluginType` is left out, because it names the Gemini plugin and this
/// client is not that plugin.
fn load_project_id(
    wire: &mut impl Wire,
    access_token: &str,
    headers: &AntigravityHeaders,
    now: i64,
) -> Result<String, Stop> {
    let body = wire
        .post_json(
            LOAD_CODE_ASSIST_URL,
            access_token,
            headers,
            &serde_json::json!({
                "metadata": {
                    "ideType": "IDE_UNSPECIFIED",
                    "platform": "PLATFORM_UNSPECIFIED"
                }
            }),
            now,
        )
        .map_err(|failure| match refusal_of(&failure, LOAD_CODE_ASSIST) {
            Some(refusal) => Stop::Refused(refusal),
            None => Stop::Failed(Box::new(answer(
                "error",
                Some(format!(
                    "Antigravity 프로젝트 ID를 읽지 못했습니다 ({})",
                    failure.message
                )),
                now,
            ))),
        })?;
    body.get("cloudaicompanionProject")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            Stop::Failed(Box::new(answer(
                "error",
                Some("Antigravity 프로젝트 ID가 응답에 없습니다".to_string()),
                now,
            )))
        })
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
) -> Result<ProviderUsage, Stop> {
    match wire.post_json(
        RETRIEVE_QUOTA_URL,
        access_token,
        headers,
        &serde_json::json!({ "project": project_id }),
        now,
    ) {
        // The KIND rides back on the answer, so the bar words it without
        // reading a sentence. A refusal is not a kind: it ends the read.
        Err(failure) => match refusal_of(&failure, RETRIEVE_USER_QUOTA) {
            Some(refusal) => Err(Stop::Refused(refusal)),
            None => Ok(ProviderUsage {
                failure_kind: Some(failure.recovery.kind),
                retry_at_ms: failure.retry_at_ms,
                ..answer(
                    "error",
                    Some(format!("Quota fetch failed ({})", failure.message)),
                    now,
                )
            }),
        },
        Ok(body) => {
            let buckets = deduplicate_buckets(
                parse_quota_response(&body)
                    .iter()
                    .map(|bucket| (build_bucket(bucket), bucket.model_id.clone()))
                    .collect(),
            );
            Ok(ProviderUsage {
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
            })
        }
    }
}

/// No login on file at all.
///
/// The sentence names the login that is missing and the surface that FIXES
/// it. It used to name a zo command in a terminal, which sent a person out of
/// the window for something the settings pane now does — and the status bar
/// shows this sentence as the segment's tooltip, so it is the only instruction
/// most people ever see. The login is Google's, not Antigravity's: the card
/// that makes it is the Google card, and the bar's word says the same
/// (t-70039).
fn signed_out(now: i64) -> ProviderUsage {
    answer(
        "signed_out",
        Some("Google 로그인 필요 — 설정 > AI 제공자 계정".to_string()),
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

/// One read: the reading, and the refusal behind it when Google refused a call.
/// A refused login is not renewed and the refused call is not sent again.
fn scan_with(
    path: Option<&Path>,
    now: i64,
    wire: &mut impl Wire,
) -> (ProviderUsage, Option<Refusal>) {
    let Some(path) = path else {
        return (signed_out(now), None);
    };
    let credentials = match google_login::read(path) {
        Ok(Some(credentials)) => credentials,
        Ok(None) => return (signed_out(now), None),
        Err(error) => return (answer("error", Some(error), now), None),
    };
    let Some(access_token) = access_token(wire, &credentials, now) else {
        return (
            answer(
                "error",
                Some("Antigravity Google 토큰을 갱신하지 못했습니다".to_string()),
                now,
            ),
            None,
        );
    };
    let headers = AntigravityHeaders::new();
    let project = match load_project_id(wire, &access_token, &headers, now) {
        Ok(project) => project,
        Err(stop) => return stop.into_read(now),
    };
    match fetch_quota(wire, &access_token, &project, &headers, now) {
        Ok(usage) => (usage, None),
        Err(stop) => stop.into_read(now),
    }
}

/// Read this machine's Google login and fetch Antigravity quota on the
/// caller's clock. A refused read names the call and the status it was refused
/// with, for the window's log and nothing else.
pub(crate) fn scan(now: i64) -> (ProviderUsage, Option<Refusal>) {
    let path = google_login::credentials_path();
    scan_with(path.as_deref(), now, &mut LiveWire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::usage_limit;

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
        body: serde_json::Value,
    }

    struct FakeWire {
        refreshes: usize,
        posts: Vec<PostedCall>,
        /// The call the server refuses, and the status it refuses with.
        refusal: Option<(&'static str, u16)>,
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
            body: &serde_json::Value,
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
                body: body.clone(),
            });
            if let Some((_, status)) = self.refusal.filter(|(refused, _)| *refused == url) {
                return Err(usage_http::Failure {
                    recovery: usage_limit::classify_http(status, ""),
                    retry_at_ms: None,
                    message: format!("HTTP {status}"),
                    skip_cli_fallback: usage_limit::skips_pty_fallback(status),
                    http_status: Some(status),
                });
            }
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
            refusal: None,
        }
    }

    #[test]
    fn missing_zo_google_login_is_signed_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut wire = fake_wire();
        let (held, _) = scan_with(Some(&dir.path().join("credentials.json")), 7, &mut wire);
        assert_eq!(held.provider, "antigravity");
        assert_eq!(held.status, "signed_out");
        // And the sentence points at the surface that fixes it, because this
        // is what the status bar shows on hover.
        assert_eq!(
            held.error.as_deref(),
            Some("Google 로그인 필요 — 설정 > AI 제공자 계정")
        );
        assert_eq!(wire.refreshes, 0);
        assert!(wire.posts.is_empty());
    }

    #[test]
    fn an_expiring_token_refreshes_with_zerocode_headers_without_writing_zo() {
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

        let (held, _) = scan_with(Some(&path), 1_800_000_000_000, &mut wire);

        assert_eq!(held.status, "ok", "{:?}", held.error);
        assert_eq!(wire.refreshes, 1, "the 60-second refresh margin moved");
        assert_eq!(wire.posts.len(), 2);
        assert!(
            wire.posts
                .iter()
                .all(|request| request.access_token == "fresh-access")
        );
        // Honest identity: this client's own User-Agent and nothing else, no
        // Antigravity or VS Code headers.
        let expected_headers = vec![(
            "User-Agent".to_string(),
            format!("zerocode/{}", env!("CARGO_PKG_VERSION")),
        )];
        assert!(
            wire.posts
                .iter()
                .all(|request| request.headers == expected_headers)
        );
        assert_eq!(wire.posts[0].url, LOAD_CODE_ASSIST_URL);
        assert_eq!(wire.posts[1].url, RETRIEVE_QUOTA_URL);
        assert_eq!(
            wire.posts[0].body,
            serde_json::json!({ "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED"
            } }),
            "the loadCodeAssist body names an IDE and a platform that are not this client's"
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("credentials after scan"),
            original,
            "the window wrote its refreshed access token into zo's store"
        );
    }

    /// A refusal from Google is an answer, not a reason to ask again. The gauge
    /// says it could not verify the quota, the refused request is not sent a
    /// second time in this scan, and the login is not renewed to try again. The
    /// old retry-once road did both, so every poll of a refused login renewed it.
    #[test]
    fn a_refused_read_is_unverified_and_is_not_asked_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            r#"{
  "google_code_assist_oauth": {
    "accessToken": "live-access",
    "refreshToken": "refresh-me",
    "expiresAt": 9999999999
  }
}"#,
        )
        .expect("credentials");
        let now = 1_800_000_000_000;
        for (refused, call, status, asked) in [
            (LOAD_CODE_ASSIST_URL, LOAD_CODE_ASSIST, 400, 1),
            (LOAD_CODE_ASSIST_URL, LOAD_CODE_ASSIST, 401, 1),
            (LOAD_CODE_ASSIST_URL, LOAD_CODE_ASSIST, 403, 1),
            (RETRIEVE_QUOTA_URL, RETRIEVE_USER_QUOTA, 401, 2),
            (RETRIEVE_QUOTA_URL, RETRIEVE_USER_QUOTA, 403, 2),
        ] {
            let mut wire = FakeWire {
                refusal: Some((refused, status)),
                ..fake_wire()
            };
            let (held, refusal) = scan_with(Some(&path), now, &mut wire);
            assert_eq!(
                refusal,
                Some(Refusal { status, call }),
                "HTTP {status} from {refused} was not named as the refusal it is"
            );
            assert_eq!(
                held.status, "refused",
                "HTTP {status} from {refused} was read as {}",
                held.status
            );
            assert_eq!(
                held.failure_kind, None,
                "a refusal named a failure kind, and the bar words that kind as a sign-in prompt"
            );
            assert_eq!(
                wire.refreshes, 0,
                "a refused login was renewed to ask again"
            );
            assert_eq!(
                wire.posts.len(),
                asked,
                "HTTP {status} from {refused} was sent again within one scan"
            );
        }
    }

    /// A refusal reaches the window's log as one line of closed words: the road,
    /// the HTTP status and the call that was refused, then the time and the
    /// answer. The body, the account and the token are not in it.
    #[test]
    fn a_refusal_is_one_log_line_of_its_status_and_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            r#"{
  "google_code_assist_oauth": {
    "accessToken": "live-access",
    "refreshToken": "refresh-me",
    "expiresAt": 9999999999
  }
}"#,
        )
        .expect("credentials");
        let mut wire = FakeWire {
            refusal: Some((LOAD_CODE_ASSIST_URL, 401)),
            ..fake_wire()
        };
        let (held, refusal) = scan_with(Some(&path), 1_800_000_000_000, &mut wire);
        let refusal = refusal.expect("the refusal was not named");
        let line = crate::usage_runtime::usage_read_line(
            "antigravity",
            crate::usage_runtime::UsageRoad::Refused {
                status: refusal.status,
                call: refusal.call,
            },
            12,
            Some(&held),
            false,
        );
        assert_eq!(
            line, "usage antigravity road=api http=401 call=loadCodeAssist ms=12 status=refused",
            "a refusal wrote more than its status and its call to the log"
        );
        for secret in ["live-access", "refresh-me", "account"] {
            assert!(
                !line.contains(secret),
                "the log line carries `{secret}`: {line}"
            );
        }
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
