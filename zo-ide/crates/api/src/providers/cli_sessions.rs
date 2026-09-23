//! Logins other CLIs keep on disk — the Grok CLI's and the Kimi Code CLI's —
//! read to list the models a login may use and, for Grok, to speak as it
//! (t-6248, C5).
//!
//! Read only: never refreshed, never rewritten. Each CLI owns its token's
//! lifecycle — a Grok OIDC token its CLI renews, a fifteen-minute Kimi token
//! behind a rotating refresh token — and a refresh spent by anybody else logs
//! the CLI itself out. An expired session is said to be expired, with the way
//! back in, and waits for the person to run the CLI again.
//!
//! The places and the reading rules are the window's (`usage_grok`,
//! `usage_kimi` and the `cli_login` rows read the same files):
//! `zerocode_core::cli_login_files` spells them for both trees, and this crate
//! — which cannot depend on that one — spells them once more here, pinned to
//! it by a zo-ide test. `ZO_DISABLE_EXTERNAL_CREDENTIALS` hides both files, as
//! it hides every other store this machine keeps outside zo.

use std::fmt;
use std::path::PathBuf;

use serde_json::Value;

use super::openai_compat::{self, OpenAiCompatConfig};
use crate::credential::CredentialMiss;
use crate::managed_account::external_credentials_disabled;

/// The variable that moves the Grok CLI's home.
pub const GROK_HOME_ENV: &str = "GROK_HOME";
/// The Grok CLI's home below the person's own.
pub const GROK_HOME_DIR: &str = ".grok";
/// The Grok CLI's login file inside its home.
pub const GROK_AUTH_FILE: &str = "auth.json";
/// Grok's own OAuth issuer — bare, or `::`-suffixed per session.
pub const GROK_PREFERRED_ISSUER: &str = "https://auth.x.ai";
/// How long before its stamp a Grok token already counts as gone.
pub const GROK_TOKEN_SKEW_MS: i64 = 5 * 60 * 1000;
/// The variable that moves the Kimi Code CLI's home.
pub const KIMI_CODE_HOME_ENV: &str = "KIMI_CODE_HOME";
/// The Kimi Code CLI's home below the person's own.
pub const KIMI_CODE_HOME_DIR: &str = ".kimi-code";
/// The Kimi Code CLI's login file below its home.
pub const KIMI_CODE_CREDENTIALS_TAIL: [&str; 2] = ["credentials", "kimi-code.json"];
/// A Kimi token expiring inside this margin is already gone.
pub const KIMI_CODE_EXPIRY_SKEW_SECONDS: i64 = 5;
/// The variable the Kimi Code CLI honours for its API.
pub const KIMI_CODE_BASE_URL_ENV: &str = "KIMI_CODE_BASE_URL";
/// The API the Kimi Code CLI speaks to.
pub const KIMI_CODE_DEFAULT_BASE_URL: &str = "https://api.kimi.com/coding/v1";

/// A session another CLI keeps: the bearer it sends. `Debug` never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct CliSession {
    pub bearer: String,
}

impl fmt::Debug for CliSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CliSession { bearer: <redacted> }")
    }
}

/// A CLI's home: its variable when set and non-empty, else `dir` below the
/// person's home — the CLIs' own resolution, and the window's.
fn cli_home(var: &str, dir: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|named| !named.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(dir)))
}

/// `$GROK_HOME/auth.json`.
#[must_use]
pub fn grok_auth_file() -> Option<PathBuf> {
    cli_home(GROK_HOME_ENV, GROK_HOME_DIR).map(|home| home.join(GROK_AUTH_FILE))
}

/// `$KIMI_CODE_HOME/credentials/kimi-code.json`.
#[must_use]
pub fn kimi_code_credentials_file() -> Option<PathBuf> {
    cli_home(KIMI_CODE_HOME_ENV, KIMI_CODE_HOME_DIR).map(|home| {
        KIMI_CODE_CREDENTIALS_TAIL
            .iter()
            .fold(home, |at, part| at.join(part))
    })
}

/// Where the Kimi Code API lists the models its login may use — the catalog
/// the CLI treats as authoritative.
#[must_use]
pub fn kimi_code_models_url() -> String {
    let base = std::env::var(KIMI_CODE_BASE_URL_ENV)
        .ok()
        .map(|held| held.trim().to_string())
        .filter(|held| !held.is_empty())
        .unwrap_or_else(|| KIMI_CODE_DEFAULT_BASE_URL.to_string());
    format!("{}/models", base.trim_end_matches('/'))
}

/// A CLI's login file as text: absent when it is not there (signed out is an
/// answer, not a failure), unusable when it is there and cannot be read.
fn read_login_file(path: Option<PathBuf>, cli: &str) -> Result<String, CredentialMiss> {
    if external_credentials_disabled() {
        return Err(CredentialMiss::Absent);
    }
    let path = path.ok_or(CredentialMiss::Absent)?;
    std::fs::read_to_string(&path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => CredentialMiss::Absent,
        _ => CredentialMiss::Unusable(format!("the {cli} login file could not be read ({error})")),
    })
}

fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

/// The Grok CLI's session, chosen as the window chooses it.
pub fn grok_session() -> Result<CliSession, CredentialMiss> {
    let raw = read_login_file(grok_auth_file(), "Grok CLI")?;
    parse_grok_auth(&raw, now_unix_millis())
}

/// One issuer's entry: the token under `key`, and its ISO `expires_at`.
struct GrokEntry {
    key: String,
    expires_at: Option<String>,
    expires_at_ms: Option<i64>,
}

impl GrokEntry {
    fn read(entry: &Value) -> Option<Self> {
        let key = entry.get("key")?.as_str()?.trim();
        if key.is_empty() {
            return None;
        }
        let expires_at = entry
            .get("expires_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        let expires_at_ms = expires_at
            .as_deref()
            .and_then(core_types::date::unix_secs_from_rfc3339)
            .map(|secs| secs.saturating_mul(1000));
        Some(Self {
            key: key.to_string(),
            expires_at,
            expires_at_ms,
        })
    }

    /// No stamp is not expired: the file may carry none, and a bad token
    /// still answers for itself.
    fn fresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms
            .is_none_or(|at| at.saturating_sub(now_ms) > GROK_TOKEN_SKEW_MS)
    }
}

fn is_preferred_issuer(issuer: &str) -> bool {
    issuer == GROK_PREFERRED_ISSUER || issuer.starts_with(&format!("{GROK_PREFERRED_ISSUER}::"))
}

/// The session `auth.json` holds. The order is the window's (`usage_grok`):
/// a fresh entry of Grok's own issuer wins outright; an expired one of it
/// beats any other issuer — it is the one the CLI refreshes on its next run —
/// and another issuer answers only when Grok's own is not in the file at
/// all. An entry left without a token (what `grok logout` leaves) is no login.
pub(crate) fn parse_grok_auth(raw: &str, now_ms: i64) -> Result<CliSession, CredentialMiss> {
    let unreadable = || {
        CredentialMiss::Unusable(
            "the Grok CLI's login file is not a readable login — sign in again with `grok login`"
                .to_string(),
        )
    };
    let parsed: Value = serde_json::from_str(raw).map_err(|_| unreadable())?;
    let entries = parsed.as_object().ok_or_else(unreadable)?;
    let mut preferred_seen = false;
    let mut expired_preferred: Option<GrokEntry> = None;
    let mut other_issuer: Option<GrokEntry> = None;
    for (issuer, entry) in entries {
        let preferred = is_preferred_issuer(issuer);
        preferred_seen |= preferred;
        let Some(entry) = GrokEntry::read(entry) else {
            continue;
        };
        if preferred {
            if entry.fresh(now_ms) {
                return Ok(CliSession { bearer: entry.key });
            }
            expired_preferred.get_or_insert(entry);
        } else if other_issuer.is_none() {
            other_issuer = Some(entry);
        }
    }
    let chosen = expired_preferred.or(if preferred_seen { None } else { other_issuer });
    match chosen {
        None => Err(CredentialMiss::Absent),
        Some(entry) if entry.fresh(now_ms) => Ok(CliSession { bearer: entry.key }),
        Some(entry) => Err(CredentialMiss::Unusable(format!(
            "the Grok CLI sign-in expired{} — sign in again with `grok login`",
            entry
                .expires_at
                .map(|at| format!(" ({at})"))
                .unwrap_or_default()
        ))),
    }
}

/// The Kimi Code CLI's session.
pub fn kimi_code_session() -> Result<CliSession, CredentialMiss> {
    let raw = read_login_file(kimi_code_credentials_file(), "Kimi Code")?;
    let now_secs = now_unix_millis() / 1000;
    parse_kimi_code_credentials(&raw, now_secs)
}

/// The token `kimi-code.json` holds, judged only by its stamp — which is
/// required: a token without one cannot be told from one that expired an
/// hour ago (the window's rule, `usage_kimi`).
pub(crate) fn parse_kimi_code_credentials(
    raw: &str,
    now_secs: i64,
) -> Result<CliSession, CredentialMiss> {
    let unreadable = || {
        CredentialMiss::Unusable(
            "the Kimi Code login file is not a readable login — sign in again with `kimi login`"
                .to_string(),
        )
    };
    let parsed: Value = serde_json::from_str(raw).map_err(|_| unreadable())?;
    let token = parsed
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(unreadable)?;
    let expires_at = parsed
        .get("expires_at")
        .and_then(Value::as_i64)
        .ok_or_else(unreadable)?;
    if expires_at.saturating_sub(now_secs) <= KIMI_CODE_EXPIRY_SKEW_SECONDS {
        return Err(CredentialMiss::Unusable(
            "the Kimi Code sign-in expired — run `kimi` to renew it (zo never refreshes another CLI's login)"
                .to_string(),
        ));
    }
    Ok(CliSession {
        bearer: token.to_string(),
    })
}

/// Whether a Kimi Code login is kept here — fresh or expired.
#[must_use]
pub fn kimi_code_login_configured() -> bool {
    !matches!(kimi_code_session(), Err(CredentialMiss::Absent))
}

/// Where an xAI credential came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XaiCredentialSource {
    /// `XAI_API_KEY` — the environment ladder, then zo's saved key.
    ApiKey,
    /// The Grok CLI's own login.
    GrokCli,
}

/// An xAI credential and where it came from. `Debug` never shows the bearer.
#[derive(Clone, PartialEq, Eq)]
pub struct XaiCredential {
    pub bearer: String,
    pub source: XaiCredentialSource,
}

impl fmt::Debug for XaiCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XaiCredential")
            .field("bearer", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

/// The one xAI credential resolution — the provider client and the model
/// list both ask it: the API key (`XAI_API_KEY` down the environment ladder,
/// then zo's saved key), then the Grok CLI's session. The Grok token is sent
/// as it is: an expired one is refused here with the way back in, never
/// refreshed.
pub fn resolve_xai_credential() -> Result<XaiCredential, CredentialMiss> {
    let key_miss = match openai_compat::read_api_key(OpenAiCompatConfig::xai().api_key_env) {
        Ok(Some(key)) => {
            return Ok(XaiCredential {
                bearer: key,
                source: XaiCredentialSource::ApiKey,
            });
        }
        Ok(None) => CredentialMiss::Absent,
        Err(error) => CredentialMiss::Unusable(error.to_string()),
    };
    match grok_session() {
        Ok(session) => Ok(XaiCredential {
            bearer: session.bearer,
            source: XaiCredentialSource::GrokCli,
        }),
        Err(miss) => Err(key_miss.or(miss)),
    }
}

/// Whether an xAI credential is kept here — a key, or a Grok CLI login,
/// fresh or expired. A file read at most; nothing is refreshed.
#[must_use]
pub fn xai_credential_configured() -> bool {
    !matches!(resolve_xai_credential(), Err(CredentialMiss::Absent))
}

#[cfg(test)]
mod tests {
    use super::{parse_grok_auth, parse_kimi_code_credentials, CliSession, GROK_PREFERRED_ISSUER, GROK_TOKEN_SKEW_MS};
    use crate::credential::CredentialMiss;

    const NOW_MS: i64 = 1_790_121_000_000;

    fn entry(token: &str, expires: Option<&str>) -> serde_json::Value {
        let mut held = serde_json::json!({ "key": token, "user_id": "u-1" });
        if let Some(expires) = expires {
            held["expires_at"] = serde_json::Value::String(expires.to_string());
        }
        held
    }

    fn bearer(answer: Result<CliSession, CredentialMiss>) -> Option<String> {
        answer.ok().map(|session| session.bearer)
    }

    /// The window's order, case by case (`usage_grok`): Grok's own issuer,
    /// fresh, wins; expired, it still beats another issuer and is refused as
    /// expired; another issuer answers only when Grok's own key is absent;
    /// a logged-out file is no login; a broken file is a login that cannot
    /// be read.
    #[test]
    fn the_grok_session_is_chosen_as_the_window_chooses_it() {
        let later = "2026-09-23T09:00:00.000000Z";
        let past = "2026-09-22T00:00:00Z";
        let file = |value: serde_json::Value| value.to_string();

        let fresh = file(serde_json::json!({
            "https://other.example": entry("other", Some(later)),
            format!("{GROK_PREFERRED_ISSUER}::acct-2"): entry("suffixed", Some(later)),
        }));
        assert_eq!(bearer(parse_grok_auth(&fresh, NOW_MS)).as_deref(), Some("suffixed"));

        let expired = file(serde_json::json!({
            "https://other.example": entry("other", Some(later)),
            GROK_PREFERRED_ISSUER: entry("expired-token", Some(past)),
        }));
        match parse_grok_auth(&expired, NOW_MS) {
            Err(CredentialMiss::Unusable(why)) => {
                assert!(why.contains("expired") && why.contains("grok login"), "{why}");
                assert!(!why.contains("expired-token"), "no token in the words: {why}");
            }
            other => panic!("an expired Grok session is there and refused: {other:?}"),
        }

        let other_only = file(serde_json::json!({ "https://other.example": entry("other", None) }));
        assert_eq!(bearer(parse_grok_auth(&other_only, NOW_MS)).as_deref(), Some("other"));

        let logged_out = file(serde_json::json!({ GROK_PREFERRED_ISSUER: { "user_id": "u-1" } }));
        assert_eq!(parse_grok_auth(&logged_out, NOW_MS).err(), Some(CredentialMiss::Absent));

        assert!(matches!(parse_grok_auth("{not json", NOW_MS), Err(CredentialMiss::Unusable(_))));
        assert!(format!("{:?}", parse_grok_auth(&fresh, NOW_MS)).contains("<redacted>"));
    }

    #[test]
    fn a_grok_token_inside_the_skew_is_already_gone() {
        let at = |ms: i64| {
            let secs = ms / 1000;
            let days = secs.div_euclid(86_400);
            let rem = secs.rem_euclid(86_400);
            let (year, month, day) = core_types::date::civil_from_unix_days(days);
            format!(
                "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
                rem / 3600,
                (rem % 3600) / 60,
                rem % 60
            )
        };
        let raw = |ms: i64| serde_json::json!({ GROK_PREFERRED_ISSUER: entry("t", Some(&at(ms))) }).to_string();
        assert!(parse_grok_auth(&raw(NOW_MS + GROK_TOKEN_SKEW_MS + 1_000), NOW_MS).is_ok());
        assert!(parse_grok_auth(&raw(NOW_MS + GROK_TOKEN_SKEW_MS), NOW_MS).is_err());
    }

    /// Kimi's stamp is required and judged with a five-second skew; nothing
    /// is refreshed.
    #[test]
    fn the_kimi_code_session_is_judged_by_its_stamp_alone() {
        let now = 1_790_121_000;
        let raw = |token: &str, expires: Option<i64>| {
            let mut value = serde_json::json!({ "access_token": token, "refresh_token": "never-read-here" });
            if let Some(expires) = expires {
                value["expires_at"] = serde_json::json!(expires);
            }
            value.to_string()
        };
        assert_eq!(
            bearer(parse_kimi_code_credentials(&raw("kimi-token", Some(now + 900)), now)).as_deref(),
            Some("kimi-token")
        );
        match parse_kimi_code_credentials(&raw("kimi-token", Some(now + 5)), now) {
            Err(CredentialMiss::Unusable(why)) => assert!(why.contains("expired") && why.contains("kimi"), "{why}"),
            other => panic!("inside the skew is expired: {other:?}"),
        }
        assert!(matches!(
            parse_kimi_code_credentials(&raw("kimi-token", None), now),
            Err(CredentialMiss::Unusable(_))
        ));
        assert!(matches!(
            parse_kimi_code_credentials(&raw("", Some(now + 900)), now),
            Err(CredentialMiss::Unusable(_))
        ));
    }
}
