//! Kimi Code's subscription usage — the third vendor on the gauge, and the
//! first one this window reads **without ever writing back**.
//!
//! Orca states the rule and the reason in the same breath: *"Orca must NEVER
//! refresh or rewrite that file — a rotated refresh token would log out a live
//! `kimi` session"* (`kimi-fetcher.ts:298-301`). The CLI owns the token's
//! lifecycle: a 15-minute access token behind a rotating refresh token, which
//! means a refresh performed by anybody else invalidates the one the CLI
//! holds. So this road reads `credentials/kimi-code.json`, uses the token that
//! is there, and when it has expired it says so and waits for the person to
//! run `kimi` again. Reporting a stale reading is the correct outcome; taking
//! the session down to avoid a stale reading is not.
//!
//! Only the host road is here. Orca also reads a WSL home on Windows
//! (`kimi-runtime-home.ts:38-56`), which is that platform's separate lane and
//! is recorded on the map rather than guessed at from macOS.

use std::path::{Path, PathBuf};

use crate::usage::{ProviderUsage, UsageWindow};
use crate::usage_http::{self, Failure};
use zerocode_core::civil::epoch_ms_of_iso;
use zerocode_core::usage_limit::FailureKind;

/// The endpoint the CLI's own `/usage` command calls, and the env var the CLI
/// honours for a self-hosted or staging base (`kimi-fetcher.ts:20`).
const BASE_URL_VAR: &str = zerocode_core::cli_login_files::kimi::BASE_URL_VAR;
const DEFAULT_BASE_URL: &str = zerocode_core::cli_login_files::kimi::DEFAULT_BASE_URL;
const USAGE_PATH: &str = "/usages";

/// `KIMI_CODE_HOME ?? ~/.kimi-code`, the CLI's own resolution — read the same
/// files the running CLI writes (`kimi-runtime-home.ts:17-19`).
pub(crate) const HOME_VAR: &str = zerocode_core::cli_login_files::kimi::HOME_VAR;
const HOME_DIR: &str = zerocode_core::cli_login_files::kimi::HOME_DIR;
const CREDENTIALS_TAIL: [&str; 2] = zerocode_core::cli_login_files::kimi::CREDENTIALS_TAIL;

/// A token expiring inside this margin is treated as already gone: firing a
/// request against a token that expires mid-flight spends a round trip to
/// learn what the stamp already said (`kimi-fetcher.ts:126-127`).
const EXPIRY_SKEW_SECONDS: i64 = zerocode_core::cli_login_files::kimi::EXPIRY_SKEW_SECONDS;

/// The two windows Kimi reports, in the durations this window already speaks.
const SESSION_WINDOW_MINUTES: u32 = crate::usage::SESSION_WINDOW_MINUTES;
const WEEKLY_WINDOW_MINUTES: u32 = crate::usage::WEEKLY_WINDOW_MINUTES;

/// What the credentials file said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Credentials {
    /// No file, or a home that is not there — not signed in, which is an
    /// answer rather than a failure.
    Absent,
    /// A file that would not parse, or one with no token in it.
    Unreadable,
    /// A token whose stamp has passed. The CLI refreshes on its next run;
    /// nothing here may.
    Expired,
    Fresh(String),
}

/// `<kimi home>/credentials/kimi-code.json`.
pub(crate) fn credentials_file(home: &Path) -> PathBuf {
    CREDENTIALS_TAIL
        .iter()
        .fold(home.to_path_buf(), |at, part| at.join(part))
}

/// The Kimi home this machine's CLI uses.
pub(crate) fn kimi_home() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os(HOME_VAR)
        && !named.is_empty()
    {
        let path = PathBuf::from(named);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    Some(dirs::home_dir()?.join(HOME_DIR))
}

/// Read the token, and judge only its stamp.
///
/// `now_seconds` is passed in rather than read here so the judgement is
/// testable without waiting for a clock.
pub(crate) fn read_credentials(file: &Path, now_seconds: i64) -> Credentials {
    let Ok(raw) = std::fs::read_to_string(file) else {
        return Credentials::Absent;
    };
    parse_credentials(&raw, now_seconds)
}

/// [`read_credentials`] over the file's text — the same judgement for a
/// caller that already holds the bytes (the login road watches the file
/// change). Reads nothing and writes nothing.
pub(crate) fn parse_credentials(raw: &str, now_seconds: i64) -> Credentials {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Credentials::Unreadable;
    };
    let token = parsed
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty());
    let Some(token) = token else {
        return Credentials::Unreadable;
    };
    // The stamp is required, not optional: a token with no expiry is a token
    // this road cannot tell apart from one that expired an hour ago, and
    // spending the request to find out is the thing the skew exists to avoid.
    let Some(expires_at) = parsed.get("expires_at").and_then(serde_json::Value::as_i64) else {
        return Credentials::Unreadable;
    };
    if expires_at - now_seconds <= EXPIRY_SKEW_SECONDS {
        return Credentials::Expired;
    }
    Credentials::Fresh(token.to_string())
}

/// `{base}/usages`, with any trailing slash on the base folded away
/// (`kimi-fetcher.ts:337`).
pub(crate) fn usage_url() -> String {
    let base = std::env::var(BASE_URL_VAR)
        .ok()
        .map(|held| held.trim().to_string())
        .filter(|held| !held.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    format!("{}{USAGE_PATH}", base.trim_end_matches('/'))
}

/// A figure the API may send as a number or as a string — both shapes ship
/// (`toInt`, `kimi-fetcher.ts:157-166`).
fn as_int(raw: Option<&serde_json::Value>) -> Option<f64> {
    let raw = raw?;
    if let Some(number) = raw.as_f64() {
        return number.is_finite().then_some(number);
    }
    let text = raw.as_str()?.trim();
    let parsed = text.parse::<f64>().ok()?;
    parsed.is_finite().then_some(parsed)
}

/// A window's duration in minutes, whatever unit it arrived in
/// (`windowToMinutes`, `kimi-fetcher.ts:168-183`).
///
/// An unrecognised unit is taken as minutes, which is Orca's own fallback —
/// and the caller still has to decide which seat the window takes, so a wrong
/// guess here cannot silently become a wrong percentage.
fn window_minutes(window: Option<&serde_json::Value>) -> Option<u32> {
    let duration = as_int(window?.get("duration"))?;
    if duration <= 0.0 {
        return None;
    }
    let unit = window?
        .get("timeUnit")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_uppercase();
    let minutes = if unit.contains("MINUTE") {
        duration
    } else if unit.contains("HOUR") {
        duration * 60.0
    } else if unit.contains("DAY") {
        duration * 60.0 * 24.0
    } else if unit.contains("SECOND") {
        (duration / 60.0).round()
    } else {
        duration
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped into u32's range on the line above the cast"
    )]
    Some(minutes.clamp(0.0, f64::from(u32::MAX)) as u32)
}

/// One quota block as a window.
///
/// `used` is preferred; when it is absent the API's `remaining` is subtracted
/// from the limit instead, which is the same figure said the other way round
/// (`mapWindow`, `kimi-fetcher.ts:203-224`). A limit of zero is not a window —
/// dividing by it would produce a percentage out of nothing.
fn quota_window(detail: Option<&serde_json::Value>, window_minutes: u32) -> Option<UsageWindow> {
    let detail = detail?;
    let limit = as_int(detail.get("limit"))?;
    if limit <= 0.0 {
        return None;
    }
    let used = as_int(detail.get("used")).or_else(|| {
        let remaining = as_int(detail.get("remaining"))?;
        Some(limit - remaining)
    })?;
    let resets_at = detail
        .get("resetTime")
        .or_else(|| detail.get("resetAt"))
        .and_then(serde_json::Value::as_str)
        .and_then(epoch_ms_of_iso);
    let reset_description = detail
        .get("resetTime")
        .or_else(|| detail.get("resetAt"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=100 before the cast"
    )]
    Some(UsageWindow {
        used_percent: ((used / limit) * 100.0).round().clamp(0.0, 100.0) as u8,
        window_minutes,
        resets_at,
        // The words as sent, kept beside the parsed stamp for the same reason
        // every other provider keeps them: they are what a person reads when
        // the parse failed, and the evidence when it did not.
        reset_description,
    })
}

/// The two windows out of one `/usages` body.
///
/// The top-level `usage` block is the weekly quota; the entries in `limits`
/// carry shorter rolling windows and the one nearest five hours is the session
/// (`mapUsageResponse`, `kimi-fetcher.ts:226-253`).
pub(crate) fn read_usage(body: &serde_json::Value) -> (Option<UsageWindow>, Option<UsageWindow>) {
    let weekly = quota_window(body.get("usage"), WEEKLY_WINDOW_MINUTES);
    let mut session: Option<UsageWindow> = None;
    let empty = Vec::new();
    let limits = body
        .get("limits")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);
    for limit in limits {
        let minutes = window_minutes(limit.get("window")).unwrap_or(SESSION_WINDOW_MINUTES);
        let Some(mapped) = quota_window(limit.get("detail"), minutes) else {
            continue;
        };
        // Nearest to a session, otherwise the first that parsed.
        let nearer = session.as_ref().is_none_or(|held| {
            minutes.abs_diff(SESSION_WINDOW_MINUTES)
                < held.window_minutes.abs_diff(SESSION_WINDOW_MINUTES)
        });
        if nearer {
            session = Some(mapped);
        }
    }
    (session, weekly)
}

/// One reading of Kimi's plan usage.
///
/// Never writes anything, anywhere. The failure kinds are chosen so the
/// scheduler above behaves the way the situation deserves: an expired token is
/// `DelegatedRefreshRequired`, which keeps the last good snapshot standing
/// while the CLI's next run fixes it, and a missing file is `unavailable`
/// rather than an error — a machine with no Kimi on it has nothing to report,
/// which is not the same as a machine whose report failed.
pub fn scan(now_ms: i64) -> ProviderUsage {
    let answer = |status: &str, error: Option<String>, kind: Option<FailureKind>| ProviderUsage {
        provider: "kimi".to_string(),
        session: None,
        weekly: None,
        fable_weekly: None,
        monthly: None,
        buckets: None,
        updated_at: now_ms,
        error,
        status: status.to_string(),
        failure_kind: kind,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: None,
    };
    let Some(home) = kimi_home() else {
        return answer(
            "error",
            Some("홈 디렉터리를 찾지 못했습니다".to_string()),
            None,
        );
    };
    let token = match read_credentials(&credentials_file(&home), now_ms / 1_000) {
        Credentials::Absent => {
            return answer(
                crate::usage::SIGNED_OUT_STATUS,
                Some("Kimi Code에 로그인되어 있지 않습니다".to_string()),
                None,
            );
        }
        Credentials::Unreadable => {
            return answer(
                "error",
                Some("Kimi 자격증명 파일을 읽지 못했습니다".to_string()),
                Some(FailureKind::MissingCredentials),
            );
        }
        Credentials::Expired => {
            // Orca's own wording for the same state: the CLI owns the token,
            // so the person's next `kimi` run is the repair.
            return answer(
                "error",
                Some(
                    "Kimi 세션이 만료되었습니다 — kimi를 한 번 실행한 뒤 다시 읽습니다".to_string(),
                ),
                Some(FailureKind::DelegatedRefreshRequired),
            );
        }
        Credentials::Fresh(token) => token,
    };
    let read = usage_http::get_json(
        &usage_url(),
        &[
            ("Authorization", &format!("Bearer {token}")),
            // The CLI sends no User-Agent here; the endpoint authenticates by
            // token alone (`kimi-fetcher.ts:339-341`).
            ("Accept", "application/json"),
        ],
        now_ms,
    );
    match read {
        Ok(body) => {
            let (session, weekly) = read_usage(&body);
            if session.is_none() && weekly.is_none() {
                return answer(
                    "error",
                    Some("Kimi 응답에 사용량 창이 없습니다".to_string()),
                    None,
                );
            }
            ProviderUsage {
                session,
                weekly,
                ..answer("ok", None, None)
            }
        }
        Err(Failure {
            recovery,
            retry_at_ms,
            message,
            ..
        }) => ProviderUsage {
            retry_at_ms,
            ..answer("error", Some(message), Some(recovery.kind))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(token: &str, expires_at: i64) -> String {
        serde_json::json!({ "access_token": token, "expires_at": expires_at }).to_string()
    }

    /// The file is read, judged by its stamp, and never written.
    #[test]
    fn the_token_is_read_and_its_stamp_believed() {
        let home = tempfile::tempdir().expect("tempdir");
        let file = credentials_file(home.path());
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");

        // Nothing there is "not signed in", not a failure.
        assert_eq!(read_credentials(&file, 1_000), Credentials::Absent);

        std::fs::write(&file, "{not json").expect("write");
        assert_eq!(read_credentials(&file, 1_000), Credentials::Unreadable);

        std::fs::write(&file, credentials("", 9_999)).expect("write");
        assert_eq!(read_credentials(&file, 1_000), Credentials::Unreadable);

        // A token with no stamp is refused rather than tried: this road cannot
        // tell it apart from one that expired an hour ago.
        std::fs::write(&file, r#"{"access_token":"opener"}"#).expect("write");
        assert_eq!(read_credentials(&file, 1_000), Credentials::Unreadable);

        std::fs::write(&file, credentials("opener", 2_000)).expect("write");
        assert_eq!(
            read_credentials(&file, 1_000),
            Credentials::Fresh("opener".to_string())
        );

        // The skew: a token expiring inside five seconds is already gone.
        assert_eq!(read_credentials(&file, 1_995), Credentials::Expired);
        assert_eq!(
            read_credentials(&file, 1_994),
            Credentials::Fresh("opener".to_string())
        );
        assert_eq!(read_credentials(&file, 3_000), Credentials::Expired);

        // And the file is byte-for-byte what it was — the whole contract.
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            credentials("opener", 2_000),
            "the credentials file was rewritten"
        );
    }

    /// Both shapes of figure, both spellings of reset, and the window that
    /// wins the session seat.
    #[test]
    fn the_windows_come_out_of_the_body_in_this_windows_units() {
        let body = serde_json::json!({
            "usage": { "limit": 1000, "used": 250, "resetTime": "2026-08-25T00:00:00Z" },
            "limits": [
                // A day-long window: parsed, but further from a session.
                { "window": { "duration": 1, "timeUnit": "DAYS" },
                  "detail": { "limit": "400", "remaining": "300" } },
                // Five hours: the session seat.
                { "window": { "duration": 5, "timeUnit": "HOURS" },
                  "detail": { "limit": 200, "used": 50, "resetAt": "2026-08-19T12:00:00Z" } },
            ],
        });
        let (session, weekly) = read_usage(&body);
        let session = session.expect("session");
        assert_eq!(
            session.window_minutes, 300,
            "the nearest window did not win"
        );
        assert_eq!(session.used_percent, 25);
        assert_eq!(session.resets_at, epoch_ms_of_iso("2026-08-19T12:00:00Z"));

        let weekly = weekly.expect("weekly");
        assert_eq!(weekly.window_minutes, 10080);
        assert_eq!(weekly.used_percent, 25);

        // `remaining` says the same thing as `used`, from the other end.
        let (only_remaining, _) = read_usage(&serde_json::json!({
            "limits": [{ "window": { "duration": 300, "timeUnit": "MINUTES" },
                          "detail": { "limit": 400, "remaining": 300 } }],
        }));
        assert_eq!(only_remaining.expect("window").used_percent, 25);

        // A limit of zero is not a window — a percentage out of nothing.
        let (none, empty) = read_usage(&serde_json::json!({
            "usage": { "limit": 0, "used": 5 },
            "limits": [{ "detail": { "limit": 0, "used": 1 } }],
        }));
        assert!(none.is_none() && empty.is_none());

        // Nothing at all is nothing, not a zero.
        let (nothing, also) = read_usage(&serde_json::json!({}));
        assert!(nothing.is_none() && also.is_none());
    }

    /// Durations arrive in four units and one of them is a lie by 60.
    #[test]
    fn a_window_duration_reads_in_whatever_unit_it_wears() {
        let held = |duration: i64, unit: &str| {
            window_minutes(Some(&serde_json::json!({
                "duration": duration, "timeUnit": unit
            })))
        };
        assert_eq!(held(90, "MINUTES"), Some(90));
        assert_eq!(held(5, "HOURS"), Some(300));
        assert_eq!(held(7, "DAYS"), Some(10080));
        assert_eq!(held(18_000, "SECONDS"), Some(300));
        // Lowercase and singular both reach the same branch.
        assert_eq!(held(5, "hour"), Some(300));
        // An unrecognised unit is minutes, which is Orca's own fallback.
        assert_eq!(held(42, "FORTNIGHTS"), Some(42));
        // A duration that is not one is no window.
        assert_eq!(held(0, "HOURS"), None);
        assert_eq!(window_minutes(Some(&serde_json::json!({}))), None);
    }

    /// The base URL follows the CLI's own env var, and the join never doubles
    /// the slash.
    #[test]
    fn the_endpoint_is_the_cli_s_own() {
        // The default, when nothing is set. (Read rather than asserted
        // against a second literal: two spellings of one URL is the bug.)
        assert!(usage_url().ends_with(USAGE_PATH));
        assert!(usage_url().starts_with(DEFAULT_BASE_URL) || std::env::var(BASE_URL_VAR).is_ok());
        assert_eq!(
            format!(
                "{}{USAGE_PATH}",
                "https://example.test/v1/".trim_end_matches('/')
            ),
            "https://example.test/v1/usages",
            "a trailing slash on the base would double"
        );
    }

    /// A machine with no Kimi on it says so, and says it as a STATUS rather
    /// than as a failed read.
    #[test]
    fn a_machine_without_kimi_is_signed_out_not_broken() {
        let home = tempfile::tempdir().expect("tempdir");
        let read = read_credentials(&credentials_file(home.path()), 0);
        assert_eq!(read, Credentials::Absent);
    }

    /// The expiry is delegated, and the kind says so — that is what keeps the
    /// last good percentage on screen instead of blanking it.
    ///
    /// `DelegatedRefreshRequired` is not decoration: the stale policy lets a
    /// FAILURE inherit the previous reading's figures, and the scheduler backs
    /// off rather than hammering an endpoint that cannot answer until a person
    /// runs `kimi`.
    #[test]
    fn an_expired_token_asks_the_cli_rather_than_refreshing_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let file = credentials_file(home.path());
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, credentials("opener", 10)).expect("write");
        assert_eq!(read_credentials(&file, 1_000), Credentials::Expired);
        // And the module names that kind nowhere else — one spelling.
        let shipped = include_str!("usage_kimi.rs");
        let (source, _) = shipped.split_once("#[cfg(test)]").unwrap_or((shipped, ""));
        // Named once in code — the doc comments above it say the name too,
        // so the count is of the CODE lines that reach for it.
        assert_eq!(
            source
                .lines()
                .filter(|line| line.contains("DelegatedRefreshRequired")
                    && !line.trim_start().starts_with("///"))
                .count(),
            1,
            "the delegated-refresh kind grew a second spelling"
        );
    }
}
