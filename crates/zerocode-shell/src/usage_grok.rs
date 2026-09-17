//! Grok's plan usage, read off the session file its own CLI maintains.
//!
//! The same read-only shape as [`crate::usage_kimi`] and for the same reason —
//! *"Orca never runs grok login; it only reads the session file the CLI
//! updates"* (`grok-fetcher.ts:240`) — but with two things Kimi does not have.
//!
//! The first is `auth.json` holding SEVERAL issuers at once. Grok's own OAuth
//! issuer is `https://auth.x.ai`, and a stale alternate issuer can sit ahead of
//! it in the file, so the entry is chosen by issuer rather than by position
//! (`grok-auth.ts:67,81-83`). An alternate issuer is a compatibility fallback
//! only when the preferred one is absent entirely — present-but-expired still
//! wins, because the expired preferred session is the one the CLI will refresh.
//!
//! The second is that the account may be billed two different ways. A credits
//! plan reports `creditUsagePercent` against a weekly period; a unified-billing
//! account reports only a monthly budget, and its credits view omits the
//! percent — so a second read of the format-less billing view is the only way
//! to see that budget (`:281-284`).

use std::path::{Path, PathBuf};

use crate::usage::{ProviderUsage, UsageWindow};
use crate::usage_http::{self, Failure};
use zerocode_core::civil::epoch_ms_of_iso;
use zerocode_core::usage_limit::FailureKind;

/// The CLI's own proxy, and the env var it honours (`grok-fetcher.ts:15-21`).
const PROXY_BASE_VAR: &str = "GROK_CLI_CHAT_PROXY_BASE_URL";
const DEFAULT_PROXY_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
/// The credits view, and the format-less one that carries a monthly budget.
const BILLING_CREDITS_PATH: &str = "/billing?format=credits";
const BILLING_DEFAULT_PATH: &str = "/billing";

/// `GROK_HOME ?? ~/.grok`, the CLI's own resolution
/// (`grok-session-paths.ts:40-46`).
const HOME_VAR: &str = "GROK_HOME";
const HOME_DIR: &str = ".grok";
const AUTH_FILE: &str = "auth.json";

/// The header xAI checks; without it the request is rejected
/// (`grok-fetcher.ts:27,140-148`).
const CLI_AUTH_HEADER: &str = "xai-grok-cli";

/// Grok's own OAuth issuer. Entries are keyed by issuer, and one file may hold
/// several (`grok-auth.ts:67`).
const PREFERRED_ISSUER: &str = "https://auth.x.ai";

/// How long before expiry a token counts as gone (`grok-auth.ts:133`).
const TOKEN_SKEW_MS: i64 = 5 * 60 * 1000;

/// Grok reports a week and a month, never a session.
const WEEKLY_WINDOW_MINUTES: u32 = 10_080;
const MONTHLY_WINDOW_MINUTES: u32 = 43_200;

/// One issuer's stored session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Session {
    pub(crate) access_token: String,
    pub(crate) user_id: Option<String>,
    pub(crate) email: Option<String>,
    /// Absent is not expired: `auth.json` may carry no stamp at all, and a bad
    /// token still surfaces as an HTTP 401 from billing (`grok-auth.ts:136-139`).
    pub(crate) expires_at_ms: Option<i64>,
}

/// What `auth.json` said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Auth {
    /// No file — or a file with no usable entry left in it, which is what
    /// `grok logout` leaves behind. Signed out is a state, not a failure
    /// (`grok-auth.ts:121-123`).
    Absent,
    Unreadable,
    Held(Session),
}

pub(crate) fn grok_home() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os(HOME_VAR) {
        let path = PathBuf::from(named);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    Some(dirs::home_dir()?.join(HOME_DIR))
}

pub(crate) fn auth_file(home: &Path) -> PathBuf {
    home.join(AUTH_FILE)
}

/// Whether the issuer key is Grok's own — bare, or with the `::` suffix the
/// CLI appends to distinguish sessions under one issuer (`grok-auth.ts:81-83`).
fn is_preferred(issuer: &str) -> bool {
    issuer == PREFERRED_ISSUER || issuer.starts_with(&format!("{PREFERRED_ISSUER}::"))
}

fn session_of(entry: &serde_json::Value) -> Option<Session> {
    let token = entry.get("key")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    let text = |name: &str| {
        entry
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    Some(Session {
        access_token: token.to_string(),
        user_id: text("user_id"),
        email: text("email"),
        expires_at_ms: entry
            .get("expires_at")
            .and_then(serde_json::Value::as_str)
            .and_then(epoch_ms_of_iso),
    })
}

/// A token still worth spending a request on.
pub(crate) fn is_fresh(session: &Session, now_ms: i64) -> bool {
    session
        .expires_at_ms
        .is_none_or(|at| at - now_ms > TOKEN_SKEW_MS)
}

/// Choose the session this road should use.
///
/// The order is the whole point (`grok-auth.ts:94-119`): a FRESH preferred
/// entry wins outright; an expired preferred entry beats any alternate issuer,
/// because that is the one the CLI refreshes on its next run; an alternate
/// issuer is used only when no preferred key is in the file at all.
pub(crate) fn read_auth(file: &Path, now_ms: i64) -> Auth {
    let Ok(raw) = std::fs::read_to_string(file) else {
        return Auth::Absent;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Auth::Unreadable;
    };
    let Some(entries) = parsed.as_object() else {
        return Auth::Unreadable;
    };
    let mut preferred_key_seen = false;
    let mut expired_preferred: Option<Session> = None;
    let mut fallback: Option<Session> = None;
    for (issuer, entry) in entries {
        let preferred = is_preferred(issuer);
        preferred_key_seen |= preferred;
        let Some(session) = session_of(entry) else {
            continue;
        };
        if preferred {
            if is_fresh(&session, now_ms) {
                return Auth::Held(session);
            }
            expired_preferred.get_or_insert(session);
        } else if fallback.is_none() {
            fallback = Some(session);
        }
    }
    let chosen = expired_preferred.or(if preferred_key_seen { None } else { fallback });
    chosen.map_or(Auth::Absent, Auth::Held)
}

fn proxy_base() -> String {
    std::env::var(PROXY_BASE_VAR)
        .ok()
        .map(|held| held.trim().trim_end_matches('/').to_string())
        .filter(|held| !held.is_empty())
        .unwrap_or_else(|| DEFAULT_PROXY_BASE.to_string())
}

pub(crate) fn credits_url() -> String {
    format!("{}{BILLING_CREDITS_PATH}", proxy_base())
}

pub(crate) fn default_billing_url() -> String {
    format!("{}{BILLING_DEFAULT_PATH}", proxy_base())
}

/// A money figure, which the API sends as `{ "val": … }` and may spell either
/// as a number or as a string (`parseMoneyVal`, `grok-fetcher.ts:119-123`).
fn money(raw: Option<&serde_json::Value>) -> Option<f64> {
    let held = raw?.get("val")?;
    let number = match held {
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok()?,
        other => other.as_f64()?,
    };
    number.is_finite().then_some(number)
}

/// The billing block, which arrives either nested under `config` or flat
/// (`resolveBillingConfig`, `grok-fetcher.ts:154-163`).
///
/// A flat body counts only when it actually carries `creditUsagePercent`: a
/// 200 with neither shape means the plan has no weekly credits, and that is a
/// state to report rather than a body to mine for figures.
pub(crate) fn billing_config(body: &serde_json::Value) -> Option<&serde_json::Value> {
    if let Some(nested) = body.get("config").filter(|held| held.is_object()) {
        return Some(nested);
    }
    body.get("creditUsagePercent")
        .and_then(serde_json::Value::as_f64)
        .map(|_| body)
}

/// When the period ends, in both the forms this window keeps.
fn period_end(config: &serde_json::Value) -> Option<&str> {
    config
        .get("currentPeriod")
        .and_then(|period| period.get("end"))
        .and_then(serde_json::Value::as_str)
        // Orca falls back to the account's own billing period end (`:177`).
        .or_else(|| {
            config
                .get("billingPeriodEnd")
                .and_then(serde_json::Value::as_str)
        })
}

fn dressed(used_percent: f64, window_minutes: u32, config: &serde_json::Value) -> UsageWindow {
    let end = period_end(config);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=100 before the cast"
    )]
    UsageWindow {
        used_percent: used_percent.round().clamp(0.0, 100.0) as u8,
        window_minutes,
        resets_at: end.and_then(epoch_ms_of_iso),
        reset_description: end.map(str::to_string),
    }
}

/// Whether the period the body describes is the confirmed weekly one
/// (`hasConfirmedWeeklyPeriod`, `grok-fetcher.ts:93-99`).
///
/// This is what lets a missing `creditUsagePercent` mean ZERO rather than
/// unknown: an account whose current period is the weekly billing period and
/// which has spent nothing reports no percent at all.
fn confirmed_weekly(config: &serde_json::Value) -> bool {
    let period = config.get("currentPeriod");
    let same = |left: Option<&serde_json::Value>, right: Option<&serde_json::Value>| match (
        left.and_then(serde_json::Value::as_str),
        right.and_then(serde_json::Value::as_str),
    ) {
        (Some(left), Some(right)) => epoch_ms_of_iso(left) == epoch_ms_of_iso(right),
        _ => false,
    };
    period
        .and_then(|held| held.get("type"))
        .and_then(serde_json::Value::as_str)
        == Some("USAGE_PERIOD_TYPE_WEEKLY")
        && same(
            period.and_then(|held| held.get("start")),
            config.get("billingPeriodStart"),
        )
        && same(
            period.and_then(|held| held.get("end")),
            config.get("billingPeriodEnd"),
        )
}

/// The weekly credits window (`mapWeeklyCredits`, `grok-fetcher.ts:101-121`).
pub(crate) fn weekly_window(config: &serde_json::Value) -> Option<UsageWindow> {
    let percent = match config
        .get("creditUsagePercent")
        .and_then(serde_json::Value::as_f64)
    {
        Some(percent) if percent.is_finite() => percent,
        Some(_) => return None,
        None if confirmed_weekly(config) => 0.0,
        None => return None,
    };
    Some(dressed(percent, WEEKLY_WINDOW_MINUTES, config))
}

/// The monthly budget window (`mapMonthlyUsage`, `grok-fetcher.ts:125-143`).
pub(crate) fn monthly_window(config: &serde_json::Value) -> Option<UsageWindow> {
    let limit = money(config.get("monthlyLimit"))?;
    let used = money(config.get("used"))?;
    if limit <= 0.0 {
        return None;
    }
    Some(dressed(
        (used / limit) * 100.0,
        MONTHLY_WINDOW_MINUTES,
        config,
    ))
}

/// Who the reading belongs to — the email if there is one, else the user id
/// (`billingUsageResult`, `grok-fetcher.ts:167-170`).
fn whose(session: &Session) -> Option<String> {
    session
        .email
        .as_deref()
        .map(str::trim)
        .filter(|held| !held.is_empty())
        .or(session.user_id.as_deref())
        .map(str::to_string)
}

/// One reading of Grok's plan usage. Writes nothing, anywhere.
pub fn scan(now_ms: i64) -> ProviderUsage {
    let answer = |status: &str, error: Option<String>, kind: Option<FailureKind>| ProviderUsage {
        provider: "grok".to_string(),
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
    let Some(home) = grok_home() else {
        return answer(
            "error",
            Some("홈 디렉터리를 찾지 못했습니다".to_string()),
            None,
        );
    };
    let held = match read_auth(&auth_file(&home), now_ms) {
        Auth::Absent => {
            return answer(
                crate::usage::SIGNED_OUT_STATUS,
                Some("Grok에 로그인되어 있지 않습니다 — grok login을 실행하세요".to_string()),
                None,
            );
        }
        Auth::Unreadable => {
            // The path is deliberately not in the sentence: it can carry a
            // local username or a custom GROK_HOME (`grok-auth.ts:41-44`).
            return answer(
                "error",
                Some("Grok 인증 파일을 읽지 못했습니다".to_string()),
                Some(FailureKind::MissingCredentials),
            );
        }
        Auth::Held(session) => session,
    };
    if !is_fresh(&held, now_ms) {
        // Reaching here always means a stored, refreshable session — a real
        // sign-out answered `Absent` above. So the repair is running `grok`,
        // not `grok login` (`grok-fetcher.ts:253-258`, issue #8497).
        return answer(
            "error",
            Some(
                "Grok 로그인이 만료되었습니다 — 이 컴퓨터에서 grok을 한 번 실행하세요".to_string(),
            ),
            Some(FailureKind::DelegatedRefreshRequired),
        );
    }
    let account = whose(&held);
    let bearer = format!("Bearer {}", held.access_token);
    let mut headers: Vec<(&str, &str)> = vec![
        ("Authorization", &bearer),
        ("X-XAI-Token-Auth", CLI_AUTH_HEADER),
        ("Accept", "application/json"),
    ];
    if let Some(user) = held.user_id.as_deref() {
        headers.push(("x-userid", user));
    }
    let body = match usage_http::get_json(&credits_url(), &headers, now_ms) {
        Ok(body) => body,
        Err(Failure {
            recovery,
            retry_at_ms,
            message,
            ..
        }) => {
            return ProviderUsage {
                retry_at_ms,
                account,
                ..answer("error", Some(message), Some(recovery.kind))
            };
        }
    };
    let Some(config) = billing_config(&body) else {
        // A 200 with no config means the plan has no weekly credits. Hiding
        // the bar is right; an error would paint a permanent alert on a
        // signed-in account that simply has no quota to show (`:271-275`).
        return ProviderUsage {
            account,
            ..answer(
                "unavailable",
                Some("Grok 응답에 요금제 정보가 없습니다".to_string()),
                None,
            )
        };
    };
    if let Some(weekly) = weekly_window(config) {
        return ProviderUsage {
            weekly: Some(weekly),
            account,
            ..answer("ok", None, None)
        };
    }
    // Unified billing: the credits view omits the percent, and the budget is
    // only in the format-less view. A FAILURE here is an error rather than
    // `unavailable`, so the stale policy keeps the last good monthly figure
    // (`:226-229` says so in as many words).
    let monthly = match usage_http::get_json(&default_billing_url(), &headers, now_ms) {
        Ok(body) => billing_config(&body)
            .or(Some(&body))
            .and_then(monthly_window),
        Err(Failure {
            recovery,
            retry_at_ms,
            message,
            ..
        }) => {
            return ProviderUsage {
                retry_at_ms,
                account,
                ..answer("error", Some(message), Some(recovery.kind))
            };
        }
    };
    match monthly {
        // Its own seat, not the weekly one. The duration alone would still
        // read "30d" in the bar, but a month filed as a week is a month that
        // any later reader — a panel row, a tooltip, a sum — believes is a
        // week.
        Some(window) => ProviderUsage {
            monthly: Some(window),
            account,
            ..answer("ok", None, None)
        },
        None => ProviderUsage {
            account,
            ..answer(
                "unavailable",
                Some("Grok 응답에 사용량이 없습니다".to_string()),
                None,
            )
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(token: &str, expires: Option<&str>) -> serde_json::Value {
        let mut held = serde_json::json!({ "key": token, "user_id": "u-1" });
        if let Some(expires) = expires {
            held["expires_at"] = serde_json::Value::String(expires.to_string());
        }
        held
    }

    fn write(file: &Path, body: &serde_json::Value) {
        std::fs::write(file, body.to_string()).expect("write");
    }

    /// The issuer decides, not the position — and an expired session of the
    /// right issuer still beats a fresh one of the wrong issuer.
    #[test]
    fn the_session_is_chosen_by_issuer_and_then_by_freshness() {
        let home = tempfile::tempdir().expect("tempdir");
        let file = auth_file(home.path());
        let now = epoch_ms_of_iso("2026-08-19T00:00:00Z").expect("now");
        let later = "2026-08-20T00:00:00Z";
        let past = "2026-08-18T00:00:00Z";

        assert_eq!(read_auth(&file, now), Auth::Absent, "no file is signed out");

        // A stale alternate issuer sits FIRST in the file and must not win.
        write(
            &file,
            &serde_json::json!({
                "https://legacy.example": entry("old", Some(later)),
                PREFERRED_ISSUER: entry("right", Some(later)),
            }),
        );
        let Auth::Held(chosen) = read_auth(&file, now) else {
            panic!("no session chosen");
        };
        assert_eq!(chosen.access_token, "right");

        // The suffixed form of the preferred issuer is preferred too.
        write(
            &file,
            &serde_json::json!({
                "https://legacy.example": entry("old", Some(later)),
                format!("{PREFERRED_ISSUER}::acct-2"): entry("suffixed", Some(later)),
            }),
        );
        let Auth::Held(chosen) = read_auth(&file, now) else {
            panic!("no session chosen");
        };
        assert_eq!(chosen.access_token, "suffixed");

        // Expired-preferred beats fresh-alternate: the CLI refreshes THAT one.
        write(
            &file,
            &serde_json::json!({
                "https://legacy.example": entry("old", Some(later)),
                PREFERRED_ISSUER: entry("expired", Some(past)),
            }),
        );
        let Auth::Held(chosen) = read_auth(&file, now) else {
            panic!("no session chosen");
        };
        assert_eq!(
            chosen.access_token, "expired",
            "an alternate issuer took over a refreshable session"
        );

        // With no preferred key at all, the alternate is the compatibility path.
        write(
            &file,
            &serde_json::json!({ "https://legacy.example": entry("old", Some(later)) }),
        );
        let Auth::Held(chosen) = read_auth(&file, now) else {
            panic!("no session chosen");
        };
        assert_eq!(chosen.access_token, "old");

        // A token-less file is `grok logout`, which is signed out — not broken.
        write(
            &file,
            &serde_json::json!({ PREFERRED_ISSUER: { "user_id": "u-1" } }),
        );
        assert_eq!(read_auth(&file, now), Auth::Absent);
        // And a file that will not parse says so.
        std::fs::write(&file, "{not json").expect("write");
        assert_eq!(read_auth(&file, now), Auth::Unreadable);

        // The file is never rewritten — the whole read-only contract.
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "{not json",
            "the auth file was rewritten"
        );
    }

    /// The skew, and the stamp that is allowed to be missing.
    #[test]
    fn a_stamp_five_minutes_out_is_already_gone_and_no_stamp_is_not() {
        let now = 1_000_000_000;
        let held = |expires: Option<i64>| Session {
            access_token: "t".to_string(),
            user_id: None,
            email: None,
            expires_at_ms: expires,
        };
        assert!(is_fresh(&held(Some(now + TOKEN_SKEW_MS + 1)), now));
        assert!(!is_fresh(&held(Some(now + TOKEN_SKEW_MS)), now));
        assert!(!is_fresh(&held(Some(now - 1)), now));
        // No stamp is not expired: a bad token still answers 401 from billing.
        assert!(is_fresh(&held(None), now));
    }

    /// Both billing shapes, and the missing percent that means zero.
    #[test]
    fn the_two_billing_shapes_map_to_the_two_windows() {
        // Nested under `config`, with a credits percent.
        let nested = serde_json::json!({
            "config": {
                "creditUsagePercent": 42.4,
                "currentPeriod": { "end": "2026-08-25T00:00:00Z" },
            },
        });
        let config = billing_config(&nested).expect("config");
        let weekly = weekly_window(config).expect("weekly");
        assert_eq!(weekly.used_percent, 42);
        assert_eq!(weekly.window_minutes, 10_080);
        assert_eq!(weekly.resets_at, epoch_ms_of_iso("2026-08-25T00:00:00Z"));

        // Flat, and only when it carries the percent.
        let flat = serde_json::json!({ "creditUsagePercent": 10 });
        assert!(billing_config(&flat).is_some());
        assert!(
            billing_config(&serde_json::json!({ "subscriptionTier": "pro" })).is_none(),
            "a body with no credit usage was mined for figures anyway"
        );

        // No percent, but the current period IS the weekly billing period —
        // that account has spent nothing, which is zero rather than unknown.
        let untouched = serde_json::json!({
            "currentPeriod": {
                "type": "USAGE_PERIOD_TYPE_WEEKLY",
                "start": "2026-08-18T00:00:00Z",
                "end": "2026-08-25T00:00:00Z",
            },
            "billingPeriodStart": "2026-08-18T00:00:00Z",
            "billingPeriodEnd": "2026-08-25T00:00:00Z",
        });
        assert_eq!(weekly_window(&untouched).expect("zero").used_percent, 0);
        // A period that does not match is not confirmed, so no window at all.
        let mismatched = serde_json::json!({
            "currentPeriod": {
                "type": "USAGE_PERIOD_TYPE_WEEKLY",
                "start": "2026-08-18T00:00:00Z",
                "end": "2026-08-25T00:00:00Z",
            },
            "billingPeriodStart": "2026-08-11T00:00:00Z",
            "billingPeriodEnd": "2026-08-18T00:00:00Z",
        });
        assert!(weekly_window(&mismatched).is_none());

        // The monthly budget, whose figures come as strings or numbers.
        let monthly = serde_json::json!({
            "monthlyLimit": { "val": "200" },
            "used": { "val": 50 },
            "billingPeriodEnd": "2026-09-01T00:00:00Z",
        });
        let window = monthly_window(&monthly).expect("monthly");
        assert_eq!(window.used_percent, 25);
        assert_eq!(window.window_minutes, 43_200);
        assert_eq!(window.resets_at, epoch_ms_of_iso("2026-09-01T00:00:00Z"));
        // A limit of zero is not a budget.
        assert!(
            monthly_window(&serde_json::json!({
                "monthlyLimit": { "val": 0 }, "used": { "val": 1 }
            }))
            .is_none()
        );
        assert!(monthly_window(&serde_json::json!({ "used": { "val": 1 } })).is_none());
    }

    /// The endpoints are the CLI's, and the base's trailing slash folds away.
    #[test]
    fn the_two_billing_views_hang_off_one_base() {
        assert!(credits_url().ends_with(BILLING_CREDITS_PATH));
        assert!(default_billing_url().ends_with(BILLING_DEFAULT_PATH));
        assert_eq!(
            credits_url().len() - BILLING_CREDITS_PATH.len(),
            default_billing_url().len() - BILLING_DEFAULT_PATH.len(),
            "the two views drifted onto different bases"
        );
    }

    /// The reading says whose it is — email first, id second.
    #[test]
    fn the_reading_carries_the_account_it_was_read_as() {
        let with_email = Session {
            access_token: "t".to_string(),
            user_id: Some("u-1".to_string()),
            email: Some("  someone@example.test ".to_string()),
            expires_at_ms: None,
        };
        assert_eq!(whose(&with_email).as_deref(), Some("someone@example.test"));
        let blank_email = Session {
            email: Some("   ".to_string()),
            ..with_email.clone()
        };
        assert_eq!(blank_email.email.as_deref(), Some("   "));
        assert_eq!(whose(&blank_email).as_deref(), Some("u-1"));
        let neither = Session {
            user_id: None,
            email: None,
            ..with_email
        };
        assert_eq!(whose(&neither), None);
    }
}
