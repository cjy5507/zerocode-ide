//! Plan usage over the providers' own OAuth APIs — the road Orca drives
//! FIRST, with the hidden terminal as its fallback, not the other way
//! around (`claude-fetcher.ts:46,355-359`, `codex-fetcher.ts:544`).
//!
//! The PTY scan in [`crate::usage`] spawns a CLI, types at it, and reads a
//! screen for up to twenty-five seconds; these two functions ask an HTTP
//! endpoint that answers in one round trip with the same figures. Every
//! failure here answers `None` and the caller walks the terminal road it
//! always had — a slow answer beats no answer, and the terminal is also the
//! screen a person can open to check the figure themselves.
//!
//! Where the Claude login comes from is not this file's question: it is handed
//! the credentials document `accounts::usage_login` found, and that function
//! reads the keychain item scoped to the store the window's own reading
//! environment names — through `/usr/bin/security`, the read the scan already
//! makes — before the runtime home's file. One knowing divergence from Orca
//! stays recorded on the map: Orca reads the person's own unsuffixed item
//! (`Claude Code-credentials`) in-process, and reading another app's item from
//! an adhoc-signed binary raises the password prompt on every rebuild — the
//! exact incident the release procedure memo records. That item is never read
//! here; a system-default selection finds no login and walks the terminal.
//!
//! The file alone was not enough (t-6583): the CLI refreshes its token in the
//! keychain, so the file the window wrote at the last switch had expired and
//! every read of it came back 401.

use std::path::Path;

use crate::usage::{ResetCredit, ResetCredits};
use crate::usage_http::{self, Failure};
use zerocode_core::civil::epoch_ms_of_iso;

/// `https://api.anthropic.com/api/oauth/usage` and the three headers the
/// contract wants, verbatim from the measurement.
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_BETA_HEADER: &str = "oauth-2025-04-20";
const CLAUDE_USER_AGENT: &str = "claude-code/2.1.0";

/// Codex reuses its CLI's own backend endpoint — no app-server session, no
/// login shell per refresh (`codex-fetcher.ts:543`).
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USER_AGENT: &str = "codex-cli";

/// The two headers Codex's own CLI sends that this road was not sending
/// (`codex-fetcher.ts:370-371`).
///
/// They are not decoration. The backend varies what it returns by who is
/// asking, and the reset-credit metadata below is one of the things it hands
/// to a client that identifies itself and withholds from one that does not.
const CODEX_BETA_HEADER: &str = "codex-1";
const CODEX_ORIGINATOR: &str = "Codex Desktop";

/// Where the reset credits live when the usage payload omits them.
///
/// Orca's comment on this endpoint says why it exists at all: *"Codex 0.140's
/// app-server strips the reset-credit metadata this backend endpoint still
/// returns"* (`codex-fetcher.ts:393`). So the figures can be true and
/// incomplete at the same time, and the second ask is what completes them.
const CODEX_RESET_CREDITS_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";

/// Codex buckets drift by a minute across CLI versions; absorb exactly that
/// (`CODEX_WINDOW_DURATION_TOLERANCE_MINUTES`).
const CODEX_WINDOW_TOLERANCE_MINUTES: u32 = 1;

/// One window as the APIs speak it, before the caller dresses it.
#[derive(Debug, Clone, PartialEq)]
pub struct OauthWindow {
    pub used_percent: f32,
    pub window_minutes: u32,
    /// Epoch milliseconds, when the API named a reset moment.
    pub resets_at: Option<i64>,
}

/// What one OAuth read produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OauthUsage {
    pub session: Option<OauthWindow>,
    pub weekly: Option<OauthWindow>,
    pub fable_weekly: Option<OauthWindow>,
    /// The tier the provider named — Codex's `plan_type`. Claude's endpoint
    /// does not speak one, so this stays `None` on that road.
    pub plan_type: Option<String>,
    /// Codex's spendable reset credits, when the backend reported any.
    pub reset_credits: Option<ResetCredits>,
}

/// Claude's plan figures, asked with `login` — the credentials document the
/// caller found for the selected account (`None`: no login to ask with, which
/// is also what a system-default selection gets, so the person's own login is
/// never read to fill a status bar).
///
/// The failure carries a KIND, not just a sentence: whether the caller walks
/// the terminal road, how long the last good snapshot may keep standing, and
/// when to ask again all hang off telling a 429 from a dropped packet
/// ([`zerocode_core::usage_limit`]).
pub fn claude(login: Option<&str>, now_ms: i64) -> Result<OauthUsage, Failure> {
    let token = login
        .and_then(claude_access_token)
        .ok_or_else(Failure::no_credentials)?;
    let body = usage_http::get_json(
        CLAUDE_USAGE_URL,
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("anthropic-beta", CLAUDE_BETA_HEADER),
            ("User-Agent", CLAUDE_USER_AGENT),
        ],
        now_ms,
    )?;
    Ok(read_claude_usage(&body))
}

/// Codex's plan figures for the login under `codex_home`. The caller has
/// already checked `auth.json` exists — the signed-out answer is its own
/// status, not a failed read.
pub fn codex(codex_home: &Path, now_ms: i64) -> Result<OauthUsage, Failure> {
    let auth = codex_auth(&codex_home.join(zerocode_core::codex_account::AUTH_FILE))
        .ok_or_else(Failure::no_credentials)?;
    let headers = codex_headers(&auth);
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let body = usage_http::get_json(CODEX_USAGE_URL, &borrowed, now_ms)?;
    // A body with no `plan_type` is an account with no plan figures to give —
    // Orca's `usage-unavailable`, which is an answer and not a broken read.
    let mut read = read_codex_usage(&body).ok_or_else(Failure::no_plan)?;
    // The second ask happens only when the first answer was short, and its
    // failure is not the read's failure: the windows are already in hand and
    // a missing credit count is a missing garnish (`withBackendRateLimit\
    // ResetCredits`, `codex-fetcher.ts:406-423` — Orca swallows the throw for
    // the same reason).
    if !complete_enough(read.reset_credits.as_ref())
        && let Ok(body) = usage_http::get_json(CODEX_RESET_CREDITS_URL, &borrowed, now_ms)
        && let Some(credits) = read_reset_credits(&body)
    {
        read.reset_credits = Some(credits);
    }
    Ok(read)
}

/// What the CLI itself sends, so the backend answers this road as fully as it
/// answers that one (`getCodexBackendAuthHeaders`, `codex-fetcher.ts:367-376`).
fn codex_headers(auth: &CodexAuth) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("Authorization", format!("Bearer {}", auth.access_token)),
        ("User-Agent", CODEX_USER_AGENT.to_string()),
        ("OpenAI-Beta", CODEX_BETA_HEADER.to_string()),
        ("originator", CODEX_ORIGINATOR.to_string()),
    ];
    if let Some(account) = &auth.account_id {
        headers.push(("ChatGPT-Account-Id", account.clone()));
    }
    headers
}

/// Whether a credit report answers the question a person would ask of it.
///
/// Orca's test, exactly: none available, or a next expiry named
/// (`hasCompleteRateLimitResetCredits`, `codex-fetcher.ts:307-311`). "You have
/// two" without "until when" is the one shape worth a second round trip —
/// zero is complete because there is no deadline left to report.
fn complete_enough(credits: Option<&ResetCredits>) -> bool {
    credits.is_some_and(|held| held.available_count == 0 || held.next_expires_at.is_some())
}

/// The backend's credit block, in either of the two places it appears — beside
/// the windows in the usage payload, or alone in the reset-credits payload.
///
/// `available_count` may be absent, and then it is COUNTED from the list
/// rather than assumed: a report with credits listed and no total is still a
/// report. Absent from both is `None`, which is Orca's own answer and not the
/// same as zero — "the backend did not say" must not render as "you have
/// none" (`mapBackendRateLimitResetCredits`, `codex-fetcher.ts:279-305`).
fn read_reset_credits(body: &serde_json::Value) -> Option<ResetCredits> {
    let raw = body
        .get("rate_limit_reset_credits")
        .filter(|held| !held.is_null())
        .unwrap_or(body);
    let credits: Vec<ResetCredit> = raw
        .get("credits")
        .and_then(serde_json::Value::as_array)
        .map(|listed| {
            listed
                .iter()
                .map(|credit| ResetCredit {
                    status: credit
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .map_or_else(|| "unknown".to_string(), str::to_lowercase),
                    expires_at: credit.get("expires_at").and_then(reset_stamp_ms),
                    granted_at: credit.get("granted_at").and_then(reset_stamp_ms),
                })
                .collect()
        })
        .unwrap_or_default();
    let listed = raw.get("credits").is_some_and(|held| held.is_array());
    let available_count = whole_count(raw.get("available_count")).or_else(|| {
        listed.then(|| {
            u32::try_from(
                credits
                    .iter()
                    .filter(|credit| credit.status == "available")
                    .count(),
            )
            .unwrap_or(u32::MAX)
        })
    })?;
    Some(ResetCredits {
        available_count,
        total_earned_count: whole_count(raw.get("total_earned_count")),
        next_expires_at: credits
            .iter()
            .filter(|credit| credit.status == "available")
            .filter_map(|credit| credit.expires_at)
            .min(),
        credits,
    })
}

/// A count the backend reported, floored at zero and rounded down — a
/// negative or fractional credit count is not a number of credits
/// (`Math.max(0, Math.floor(...))`, `codex-fetcher.ts:298`).
fn whole_count(raw: Option<&serde_json::Value>) -> Option<u32> {
    let number = raw?.as_f64()?;
    if !number.is_finite() {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped into u32's range on the line above the cast"
    )]
    Some(number.floor().clamp(0.0, f64::from(u32::MAX)) as u32)
}

/// The access token out of a Claude credentials document —
/// `claudeAiOauth.accessToken` — and nothing else read or judged: the local
/// `expiresAt` is deliberately NOT consulted, and the server decides, as Orca
/// lets it (claude-fetcher.ts:105).
///
/// Which document is the caller's question (`accounts::usage_login`). What it
/// must NOT be is a guess at the person's own `~/.claude`: this scan runs on a
/// fifteen-minute timer, and a fallback there once took the token of whoever
/// was logged in on this machine — somebody else's login, for a figure on our
/// status bar. A missing login says "no account selected", which is true and
/// is what the person can act on.
fn claude_access_token(document: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(document).ok()?;
    let token = parsed.get("claudeAiOauth")?.get("accessToken")?.as_str()?;
    (!token.trim().is_empty()).then(|| token.to_string())
}

struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
}

/// `auth.json` — `tokens.access_token` opens the door, `tokens.account_id`
/// names which ChatGPT account the figures belong to when present.
fn codex_auth(file: &Path) -> Option<CodexAuth> {
    let raw = std::fs::read_to_string(file).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let tokens = parsed.get("tokens")?;
    let access_token = tokens.get("access_token")?.as_str()?;
    if access_token.trim().is_empty() {
        return None;
    }
    Some(CodexAuth {
        access_token: access_token.to_string(),
        account_id: tokens
            .get("account_id")
            .and_then(|held| held.as_str())
            .filter(|held| !held.trim().is_empty())
            .map(str::to_string),
    })
}

/// A usage read that produced no figures, and what it failed of.
///
/// The kind is the deciding half; the message is kept BESIDE it rather than
/// replaced by it, exactly as Orca keeps `error` and `failureKind` both
/// (`rate-limit-types.ts:86`). The sentence is diagnostic — the words a person
/// reads are the renderer's to make from the kind, which is how they get to be
/// translated (`usage-error-copy.ts:78-118`).
/// Claude's response: `five_hour` and `seven_day` windows, plus the scoped
/// `limits[]` list newer servers put the model ledger in.
fn read_claude_usage(body: &serde_json::Value) -> OauthUsage {
    OauthUsage {
        session: claude_window(body.get("five_hour"), 300),
        weekly: claude_window(body.get("seven_day"), 10080),
        fable_weekly: claude_scoped_weekly(body),
        // Claude's endpoint names neither: the plan is not in the payload, and
        // reset credits are a Codex idea.
        plan_type: None,
        reset_credits: None,
    }
}

/// One Claude window: `utilization` first, `used_percentage` as the older
/// spelling, clamped the way Orca clamps (`mapClaudeUsageWindow`).
fn claude_window(raw: Option<&serde_json::Value>, window_minutes: u32) -> Option<OauthWindow> {
    let raw = raw?;
    let percent = raw
        .get("utilization")
        .and_then(serde_json::Value::as_f64)
        .or_else(|| {
            raw.get("used_percentage")
                .and_then(serde_json::Value::as_f64)
        })?;
    Some(OauthWindow {
        #[allow(clippy::cast_possible_truncation)]
        used_percent: percent.clamp(0.0, 100.0) as f32,
        window_minutes,
        resets_at: raw.get("resets_at").and_then(reset_stamp_ms),
    })
}

/// The model-scoped weekly ledger. Newer servers carry it as a structured
/// scoped limit; older ones as any of three legacy field spellings — the
/// exact preference order of `mapFableWeeklyWindow` (claude-fetcher.ts:316),
/// including reading INACTIVE entries: `is_active` marks the binding limit,
/// not data validity (#8979).
fn claude_scoped_weekly(body: &serde_json::Value) -> Option<OauthWindow> {
    let scoped = body
        .get("limits")
        .and_then(serde_json::Value::as_array)
        .and_then(|limits| {
            limits.iter().find(|limit| {
                limit.get("kind").and_then(serde_json::Value::as_str) == Some("weekly_scoped")
                    && limit
                        .get("percent")
                        .and_then(serde_json::Value::as_f64)
                        .is_some()
                    && limit
                        .get("scope")
                        .and_then(|scope| scope.get("model"))
                        .and_then(|model| model.get("display_name"))
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|name| name.trim().eq_ignore_ascii_case("fable"))
            })
        });
    if let Some(limit) = scoped {
        #[allow(clippy::cast_possible_truncation)]
        return Some(OauthWindow {
            used_percent: limit
                .get("percent")
                .and_then(serde_json::Value::as_f64)?
                .clamp(0.0, 100.0) as f32,
            window_minutes: 10080,
            resets_at: limit.get("resets_at").and_then(reset_stamp_ms),
        });
    }
    for legacy in ["fable_weekly", "fable_seven_day", "seven_day_fable"] {
        if let Some(window) = claude_window(body.get(legacy), 10080) {
            return Some(window);
        }
    }
    None
}

/// Codex's response: two anonymous windows told apart by their duration,
/// with Orca's exact tolerance and its legacy order for durations nobody
/// recognises (`classifyCodexRateLimitWindows`). `plan_type` must be a
/// string or the whole payload is distrusted — Orca rejects it so its next
/// road runs, and here that road is the terminal.
fn read_codex_usage(body: &serde_json::Value) -> Option<OauthUsage> {
    let plan_type = body.get("plan_type")?.as_str()?.trim().to_string();
    let rate_limit = body.get("rate_limit");
    let primary = codex_window(rate_limit.and_then(|held| held.get("primary_window")));
    let secondary = codex_window(rate_limit.and_then(|held| held.get("secondary_window")));

    let kind_of = |window: &CodexWindow| -> Option<bool> {
        // `Some(true)` weekly, `Some(false)` session, `None` unrecognised.
        let minutes = window.window_minutes?;
        if minutes.abs_diff(crate::usage::SESSION_WINDOW_MINUTES) <= CODEX_WINDOW_TOLERANCE_MINUTES
        {
            return Some(false);
        }
        if minutes.abs_diff(crate::usage::WEEKLY_WINDOW_MINUTES) <= CODEX_WINDOW_TOLERANCE_MINUTES {
            return Some(true);
        }
        None
    };
    let mut session: Option<&CodexWindow> = None;
    let mut weekly: Option<&CodexWindow> = None;
    for window in [primary.as_ref(), secondary.as_ref()].into_iter().flatten() {
        match kind_of(window) {
            Some(false) if session.is_none() => session = Some(window),
            Some(true) if weekly.is_none() => weekly = Some(window),
            _ => {}
        }
    }
    // Unknown durations keep the legacy seats: primary is the session,
    // secondary the week.
    if session.is_none()
        && let Some(window) = primary.as_ref()
        && kind_of(window).is_none()
    {
        session = Some(window);
    }
    if weekly.is_none()
        && let Some(window) = secondary.as_ref()
        && kind_of(window).is_none()
    {
        weekly = Some(window);
    }

    let dressed = |window: Option<&CodexWindow>, fallback_minutes: u32| -> Option<OauthWindow> {
        let window = window?;
        Some(OauthWindow {
            used_percent: window.used_percent.clamp(0.0, 100.0),
            window_minutes: window.window_minutes.unwrap_or(fallback_minutes),
            resets_at: window.resets_at,
        })
    };
    Some(OauthUsage {
        session: dressed(session, crate::usage::SESSION_WINDOW_MINUTES),
        weekly: dressed(weekly, crate::usage::WEEKLY_WINDOW_MINUTES),
        fable_weekly: None,
        // A blank string is a field that was sent and says nothing; the window
        // would render "Codex · " and look broken.
        plan_type: (!plan_type.is_empty()).then_some(plan_type),
        reset_credits: read_reset_credits(body),
    })
}

struct CodexWindow {
    used_percent: f32,
    window_minutes: Option<u32>,
    resets_at: Option<i64>,
}

/// One backend window: `used_percent` is the entry ticket, the duration is
/// `ceil(limit_window_seconds / 60)` when it is a positive finite number
/// (`backendWindowToSnapshot`), and `reset_at` arrives in epoch seconds.
fn codex_window(raw: Option<&serde_json::Value>) -> Option<CodexWindow> {
    let raw = raw?;
    let used = raw
        .get("used_percent")
        .and_then(serde_json::Value::as_f64)?;
    let minutes = raw
        .get("limit_window_seconds")
        .and_then(serde_json::Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(|seconds| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                (seconds / 60.0).ceil() as u32
            }
        });
    #[allow(clippy::cast_possible_truncation)]
    Some(CodexWindow {
        used_percent: used as f32,
        window_minutes: minutes,
        resets_at: raw.get("reset_at").and_then(reset_stamp_ms),
    })
}

/// A reset stamp as epoch milliseconds, from either spelling the APIs use: a
/// number (epoch seconds — promoted to ms below the year-33658 line) or an
/// ISO-8601 string with an explicit zone. A form without a zone is left
/// unread — guessing a timezone puts the countdown hours off, and `None`
/// only costs the label.
fn reset_stamp_ms(raw: &serde_json::Value) -> Option<i64> {
    if let Some(number) = raw.as_f64() {
        if !number.is_finite() || number <= 0.0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let stamp = if number < 1e12 {
            (number * 1000.0) as i64
        } else {
            number as i64
        };
        return Some(stamp);
    }
    let text = raw.as_str()?.trim();
    // A stamp quoted as a string is still a stamp. The reset-credit payloads
    // send seconds, milliseconds, ISO text, and quoted numbers across
    // versions, and Orca reads all four (`parseCreditTimestamp`,
    // `codex-fetcher.ts:211-226`).
    if let Ok(number) = text.parse::<f64>() {
        return reset_stamp_ms(&serde_json::Value::from(number));
    }
    epoch_ms_of_iso(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_claude_response_reads_its_three_ledgers_apart() {
        let body: serde_json::Value = serde_json::json!({
            "five_hour": { "utilization": 26.0, "resets_at": "2026-08-18T03:00:00Z" },
            "seven_day": { "used_percentage": 12.0, "resets_at": 1787000000 },
            "limits": [
                { "kind": "weekly_scoped", "percent": 7.0, "is_active": false,
                  "scope": { "model": { "display_name": "Fable" } },
                  "resets_at": "2026-08-20T00:00:00+09:00" },
                { "kind": "weekly_scoped", "percent": 99.0,
                  "scope": { "model": { "display_name": "Other" } } }
            ]
        });
        let read = read_claude_usage(&body);
        let session = read.session.expect("five_hour");
        assert!((session.used_percent - 26.0).abs() < 0.01);
        assert_eq!(session.window_minutes, 300);
        assert_eq!(session.resets_at, epoch_ms_of_iso("2026-08-18T03:00:00Z"));
        let weekly = read.weekly.expect("seven_day");
        assert!((weekly.used_percent - 12.0).abs() < 0.01);
        assert_eq!(weekly.window_minutes, 10080);
        assert_eq!(weekly.resets_at, Some(1_787_000_000_000));
        // The scoped ledger wins over any legacy spelling, inactive or not —
        // is_active marks the binding limit, not data validity (#8979).
        let scoped = read.fable_weekly.expect("scoped weekly");
        assert!((scoped.used_percent - 7.0).abs() < 0.01);

        // Legacy spellings still read when no scoped entry names the model.
        let legacy: serde_json::Value =
            serde_json::json!({ "seven_day_fable": { "utilization": 3.0 } });
        assert!(read_claude_usage(&legacy).fable_weekly.is_some());

        // Overflow clamps; an empty body is three absences, never zeros.
        let over: serde_json::Value = serde_json::json!({ "five_hour": { "utilization": 130.0 } });
        assert!(
            (read_claude_usage(&over)
                .session
                .expect("clamped")
                .used_percent
                - 100.0)
                .abs()
                < 0.01
        );
        let empty = read_claude_usage(&serde_json::json!({}));
        assert!(empty.session.is_none() && empty.weekly.is_none() && empty.fable_weekly.is_none());
    }

    #[test]
    fn the_codex_windows_are_told_apart_by_their_durations() {
        // 300±1 is the session, 10080±1 the week — order does not decide.
        let body = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": { "used_percent": 40.0, "limit_window_seconds": 604_860,
                                     "reset_at": 1_787_000_000 },
                "secondary_window": { "used_percent": 12.0, "limit_window_seconds": 17_940 }
            }
        });
        let read = read_codex_usage(&body).expect("payload");
        let session = read.session.expect("session");
        assert!((session.used_percent - 12.0).abs() < 0.01);
        assert_eq!(session.window_minutes, 299);
        let weekly = read.weekly.expect("weekly");
        assert!((weekly.used_percent - 40.0).abs() < 0.01);
        assert_eq!(weekly.resets_at, Some(1_787_000_000_000));

        // Durations nobody recognises keep the legacy seats: primary is the
        // session, secondary the week.
        let odd = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": { "used_percent": 1.0, "limit_window_seconds": 60 },
                "secondary_window": { "used_percent": 2.0 }
            }
        });
        let read = read_codex_usage(&odd).expect("payload");
        assert!((read.session.expect("primary").used_percent - 1.0).abs() < 0.01);
        let weekly = read.weekly.expect("secondary");
        assert!((weekly.used_percent - 2.0).abs() < 0.01);
        // And the fallback minutes dress the unknowns.
        assert_eq!(weekly.window_minutes, 10080);

        // No plan_type string → the whole payload is distrusted, so the
        // terminal road runs (Orca rejects it for its own fallback).
        let planless = serde_json::json!({ "rate_limit": {
            "primary_window": { "used_percent": 12.0 } } });
        assert!(read_codex_usage(&planless).is_none());
    }

    #[test]
    fn the_codex_ask_carries_what_the_cli_carries() {
        let auth = CodexAuth {
            access_token: "opener".to_string(),
            account_id: Some("acct-9".to_string()),
        };
        let sent = codex_headers(&auth);
        let named = |name: &str| {
            sent.iter()
                .find(|(held, _)| *held == name)
                .map(|(_, value)| value.as_str())
        };
        // The two the backend uses to decide how much it will say.
        assert_eq!(named("OpenAI-Beta"), Some("codex-1"));
        assert_eq!(named("originator"), Some("Codex Desktop"));
        assert_eq!(named("User-Agent"), Some("codex-cli"));
        assert_eq!(named("Authorization"), Some("Bearer opener"));
        assert_eq!(named("ChatGPT-Account-Id"), Some("acct-9"));

        // No account on the token, no header claiming one.
        let anonymous = CodexAuth {
            access_token: "opener".to_string(),
            account_id: None,
        };
        assert!(
            !codex_headers(&anonymous)
                .iter()
                .any(|(name, _)| *name == "ChatGPT-Account-Id")
        );
    }

    #[test]
    fn the_credits_are_counted_when_the_backend_does_not_count_them() {
        // available_count absent → counted from the list, and only the
        // available ones. Statuses arrive in either case.
        let listed = serde_json::json!({
            "credits": [
                { "status": "AVAILABLE", "expires_at": 1_800_000_100, "granted_at": 1_700_000_000 },
                { "status": "available", "expires_at": "2027-06-01T00:00:00Z" },
                { "status": "spent", "expires_at": 1_700_000_000 },
                { "status": "expired" }
            ]
        });
        let read = read_reset_credits(&listed).expect("a report");
        assert_eq!(read.available_count, 2);
        assert_eq!(read.credits.len(), 4);
        assert_eq!(read.credits[0].status, "available", "status was not folded");
        assert_eq!(read.credits[0].granted_at, Some(1_700_000_000_000));
        // ISO text is a stamp too (oracle: 2027-06-01T00:00:00Z is epoch
        // second 1_811_808_000).
        assert_eq!(read.credits[1].expires_at, Some(1_811_808_000_000));
        // The soonest expiry AMONG THE AVAILABLE — the spent credit expires
        // sooner and is not a deadline anybody can act on.
        assert_eq!(read.next_expires_at, Some(1_800_000_100_000));

        // A stated count wins over the list, and is floored at whole credits.
        let stated = serde_json::json!({
            "available_count": 3.7, "total_earned_count": 9,
            "credits": [{ "status": "available" }]
        });
        let read = read_reset_credits(&stated).expect("a report");
        assert_eq!(read.available_count, 3);
        assert_eq!(read.total_earned_count, Some(9));
        // No expiry stamped anywhere → no deadline invented.
        assert!(read.next_expires_at.is_none());
        assert!(
            read_reset_credits(&serde_json::json!({ "available_count": -4 }))
                .is_some_and(|read| read.available_count == 0)
        );

        // Nothing said at all is NOT zero credits. A window that renders
        // "0 resets available" from silence is telling the person something
        // the backend never claimed.
        assert!(read_reset_credits(&serde_json::json!({})).is_none());
        assert!(
            read_reset_credits(&serde_json::json!({ "rate_limit_reset_credits": null })).is_none()
        );
    }

    #[test]
    fn a_credit_count_without_a_deadline_is_worth_asking_again_for() {
        let none_left = ResetCredits {
            available_count: 0,
            total_earned_count: Some(4),
            next_expires_at: None,
            credits: Vec::new(),
        };
        // Zero is complete: there is no deadline left to report.
        assert!(complete_enough(Some(&none_left)));

        let short = ResetCredits {
            available_count: 2,
            ..none_left.clone()
        };
        assert!(
            !complete_enough(Some(&short)),
            "a countdown-less two is short"
        );
        assert!(complete_enough(Some(&ResetCredits {
            next_expires_at: Some(1_800_000_000_000),
            ..short
        })));
        // And an absent report is short, which is what sends the second ask.
        assert!(!complete_enough(None));
    }

    #[test]
    fn the_plan_and_its_credits_ride_out_of_the_usage_payload() {
        let body = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": { "primary_window": { "used_percent": 5.0,
                                                 "limit_window_seconds": 18_000 } },
            "rate_limit_reset_credits": {
                "available_count": 1,
                "credits": [{ "status": "available", "expires_at": 1_800_000_000 }]
            }
        });
        let read = read_codex_usage(&body).expect("payload");
        assert_eq!(read.plan_type.as_deref(), Some("plus"));
        let credits = read.reset_credits.expect("credits");
        assert_eq!(credits.available_count, 1);
        assert_eq!(credits.next_expires_at, Some(1_800_000_000_000));

        // A plan sent blank is a plan the window must not print — "Codex · "
        // reads as a bug, and it would be one.
        let blank = serde_json::json!({ "plan_type": "  ", "rate_limit": {} });
        assert!(
            read_codex_usage(&blank)
                .expect("payload")
                .plan_type
                .is_none()
        );

        // No credit block at all: the windows still stand.
        let bare = serde_json::json!({ "plan_type": "pro", "rate_limit": {} });
        let read = read_codex_usage(&bare).expect("payload");
        assert_eq!(read.plan_type.as_deref(), Some("pro"));
        assert!(read.reset_credits.is_none());
    }

    #[test]
    fn the_reset_stamps_read_both_spellings_and_refuse_a_zoneless_guess() {
        // Epoch seconds are promoted to milliseconds; milliseconds pass.
        assert_eq!(
            reset_stamp_ms(&serde_json::json!(1_787_000_000)),
            Some(1_787_000_000_000)
        );
        assert_eq!(
            reset_stamp_ms(&serde_json::json!(1_787_000_000_000i64)),
            Some(1_787_000_000_000)
        );
        // The ISO reading itself is pinned in `zerocode_core::civil`; here only
        // that the JSON wrapper still reaches it.
        assert_eq!(
            reset_stamp_ms(&serde_json::json!("1970-01-01T00:00:01Z")),
            Some(1_000)
        );
        assert_eq!(
            reset_stamp_ms(&serde_json::json!("2026-08-18T00:00:00")),
            None
        );
        assert_eq!(reset_stamp_ms(&serde_json::json!(null)), None);
        assert_eq!(reset_stamp_ms(&serde_json::json!(-5)), None);
    }

    #[test]
    fn the_credential_files_open_only_for_a_real_token() {
        let dir = tempfile::tempdir().expect("sandbox");

        // A Claude login arrives as a document — out of the keychain or a
        // file, whichever `accounts::usage_login` found — and opens only for
        // a real token.
        assert_eq!(claude_access_token(""), None, "no document");
        assert_eq!(claude_access_token("{not json"), None, "corrupt document");
        assert_eq!(
            claude_access_token(r#"{ "claudeAiOauth": { "accessToken": "  " } }"#),
            None,
            "blank token"
        );
        assert_eq!(
            claude_access_token(r#"{ "claudeAiOauth": { "accessToken": "sk-ant-oat01-live" } }"#)
                .as_deref(),
            Some("sk-ant-oat01-live")
        );

        let auth_file = dir.path().join("auth.json");
        std::fs::write(&auth_file, r#"{ "tokens": { "access_token": "eya" } }"#).expect("write");
        let bare = codex_auth(&auth_file).expect("token");
        assert_eq!(bare.access_token, "eya");
        assert_eq!(bare.account_id, None);
        std::fs::write(
            &auth_file,
            r#"{ "tokens": { "access_token": "eyb", "account_id": "acc-1" } }"#,
        )
        .expect("write");
        let named = codex_auth(&auth_file).expect("token");
        assert_eq!(named.account_id.as_deref(), Some("acc-1"));
        std::fs::write(&auth_file, r#"{ "tokens": { "access_token": "" } }"#).expect("write");
        assert!(
            codex_auth(&auth_file).is_none(),
            "empty token opened a door"
        );
    }
}
