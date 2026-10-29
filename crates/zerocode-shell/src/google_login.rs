//! This machine's Google login — the store zo owns, and the window's own
//! road into it.
//!
//! One login, two readers. zo signs in with `zo login google` and keeps the
//! result under `google_code_assist_oauth` in
//! `${ZO_CONFIG_HOME:-~/.zo}/credentials.json`; the window's Antigravity
//! gauge ([`crate::usage_antigravity`]) reads that same entry for quota. Until
//! now the window could only READ it, so a person whose Google login had
//! never happened — or had come undone — was sent to a terminal to type a zo
//! command, while the Claude and Codex logins both had a card in 설정 >
//! AI 제공자 계정 ("설정에 oauth 설정해줘 이것도.. codex랑 claude처럼",
//! 2026-09-03).
//!
//! So this module owns BOTH halves of that one login and nothing else does:
//! the store's path, key and shape, the OAuth client, the consent round trip,
//! and the reading that turns what is on disk into what a card can say. The
//! gauge asks it for the path, the credentials, a token refresh and the Code
//! Assist project zo recorded for the login; the commands in `cmd/usage.rs`
//! ask it for the login flow. Neither owns a copy.
//!
//! The write side follows zo's own protocol rather than inventing one
//! (`zo-ide/crates/api/src/oauth_store.rs`): the advisory lock on
//! `credentials.json.lock`, a read-modify-write of the ROOT object so every
//! other key (`oauth`, `openai_oauth`, `openai_compat_api_keys`, `mcp_oauth`)
//! survives untouched, and a staged 0600 file renamed into place under a 0700
//! parent. Both products take the same lock, so a login started here cannot
//! land on top of a refresh zo started a millisecond earlier.
//!
//! No token is ever logged or put in an error sentence. What comes back out
//! of this module for a person to read is an email address and a clock.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::durable_file;
use crate::hooks;
use crate::usage_http;

/// The one top-level key this login lives under, in zo's spelling.
const OAUTH_KEY: &str = "google_code_assist_oauth";

/// Antigravity IDE's installed-app OAuth client, the same pair zo ships
/// (`zo-ide/crates/api/src/providers/gemini_code_assist.rs`). Installed-app
/// credentials are not confidential — Google documents the secret as embedded
/// in and extractable from the desktop client (RFC 8252 §8.5) — and using
/// Antigravity's own client is what lets this sign-in need no per-user setup.
///
/// Named here ONCE. The gauge's refresh and this module's exchange both read
/// these, and a second copy would be a login that works and a refresh that
/// does not.
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// The scope Antigravity's agent road asks for beyond the five zo requests.
///
/// Measured 2026-09-03: it is registered on this client and consented to. It
/// is also the one scope a login made by zo does NOT carry, which is why the
/// card can offer 「다시 로그인」 to a login that otherwise looks perfectly
/// healthy — the quota gauge only needs `cloud-platform`, so nothing else can
/// tell the two apart.
const AGENT_SCOPE: &str = "https://www.googleapis.com/auth/aicode";

/// What consent is asked for: zo's `GEMINI_CODE_ASSIST_SCOPES` and the agent
/// road's own. A login this window makes is a superset of one zo makes, so zo
/// keeps working against it unchanged.
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
    AGENT_SCOPE,
];

/// The loopback seat this client is registered for — FIXED, unlike the
/// ephemeral port an ordinary desktop client may use. Antigravity's redirect
/// URI is `http://localhost:51121/oauth-callback`, so the listener has to bind
/// that exact port or Google refuses the redirect
/// (`ANTIGRAVITY_CALLBACK_PORT`, zo's own constant).
const CALLBACK_PORT: u16 = 51121;
const CALLBACK_PATH: &str = "/oauth-callback";

/// How long a person gets to finish at Google before the seat is given up.
const CONSENT_CEILING: Duration = Duration::from_secs(300);
/// How long a bind waits for a seat an abandoned attempt is still letting go
/// of — a reload mid-sign-in leaves one listening for up to one poll.
const SEAT_TRIES: u32 = 10;
const SEAT_WAIT: Duration = Duration::from_millis(50);
/// How often the waiting road looks at the socket and at the cancel flag.
const CONSENT_POLL: Duration = Duration::from_millis(100);
/// A connection that sends this much without ending its first line is not
/// bringing the redirect — zo's own bound (`OAUTH_CALLBACK_LINE_LIMIT`).
const REQUEST_LINE_LIMIT: usize = 8 * 1024;
/// Connections held at once while none has brought the redirect; one more
/// lets the oldest go — zo's own bound (`OAUTH_CALLBACK_HELD_LIMIT`).
const HELD_LIMIT: usize = 32;
/// How long the browser is given to take the page that ends the sign-in.
const RESPONSE_WAIT: Duration = Duration::from_secs(2);

/// A refresh is asked for this many seconds before expiry, so a token that
/// dies mid-request is renewed before the request rather than after it.
const REFRESH_SLOP_SECONDS: u64 = 60;

/// How long the credential lock is waited for — zo's own budget
/// (fifty tries, fifty milliseconds apart), so neither side gives up first.
const LOCK_TRIES: u32 = 50;
const LOCK_WAIT: Duration = Duration::from_millis(50);

/// The token set as zo writes it: four camelCase fields and nothing else.
///
/// Written in zo's shape on purpose. This entry is read by zo's own
/// `StoredOAuthCredentials` and rewritten by it on every refresh, so a field
/// of ours would be silently dropped the first time zo renews the token — a
/// place to cache something is not what this file is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Credentials {
    pub(crate) access_token: String,
    #[serde(default)]
    pub(crate) refresh_token: Option<String>,
    /// Unix SECONDS, which is what zo stores — the rest of this window counts
    /// in milliseconds, so every crossing here is explicit.
    #[serde(default)]
    pub(crate) expires_at: Option<u64>,
    #[serde(default)]
    pub(crate) scopes: Vec<String>,
}

impl Credentials {
    /// Whether the consent behind this login covers the agent road.
    ///
    /// An empty list is not a verdict: an entry written before scopes were
    /// recorded knows nothing about its own consent, and alarming on "unknown"
    /// would send everybody through a sign-in they may not need.
    fn covers_agent_road(&self) -> bool {
        self.scopes.is_empty() || self.scopes.iter().any(|scope| scope == AGENT_SCOPE)
    }

    fn expired_at(&self, now_seconds: u64) -> bool {
        self.expires_at
            .is_some_and(|expires| expires <= now_seconds)
    }
}

/* ---- where it lives ----------------------------------------------------- */

/// zo's credentials file, or `None` on a machine with no home at all.
pub(crate) fn credentials_path() -> Option<PathBuf> {
    credentials_path_from(
        std::env::var_os("ZO_CONFIG_HOME").as_deref().map(Path::new),
        dirs::home_dir().as_deref(),
    )
}

pub(crate) fn credentials_path_from(
    config_home: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    config_home
        .map(Path::to_path_buf)
        .or_else(|| home.map(|home| home.join(".zo")))
        .map(|root| root.join("credentials.json"))
}

/// Whether this machine has the login on file at all — the question a
/// default asks (the status bar's, `settings_runtime`), which needs no token
/// and no clock. A file that cannot be read is no login.
pub(crate) fn on_file(path: Option<&Path>) -> bool {
    path.is_some_and(|path| matches!(read(path), Ok(Some(_))))
}

/// [`on_file`] at this machine's own store.
pub(crate) fn on_this_machine() -> bool {
    on_file(credentials_path().as_deref())
}

/// The login on file, `None` when the file or the key is absent.
pub(crate) fn read(path: &Path) -> Result<Option<Credentials>, String> {
    match read_root(path)?.get(OAUTH_KEY) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| error.to_string()),
    }
}

/// The whole root object, so a write can put one key back and leave the rest.
fn read_root(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(error.to_string()),
    };
    if raw.trim().is_empty() {
        return Ok(Default::default());
    }
    match serde_json::from_str(&raw).map_err(|error| error.to_string())? {
        serde_json::Value::Object(root) => Ok(root),
        _ => Err("zo 자격 증명 파일이 JSON 객체가 아닙니다".to_string()),
    }
}

/// Put this login on file, leaving every other credential zo keeps alone.
pub(crate) fn save(path: &Path, held: &Credentials) -> Result<(), String> {
    let value = serde_json::to_value(held).map_err(|error| error.to_string())?;
    update_root(path, |root| {
        root.insert(OAUTH_KEY.to_string(), value);
    })
}

/// Take this login off file, leaving every other credential zo keeps alone.
pub(crate) fn clear(path: &Path) -> Result<(), String> {
    update_root(path, |root| {
        root.remove(OAUTH_KEY);
    })
}

fn update_root(
    path: &Path,
    change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("zo 자격 증명 경로에 상위 폴더가 없습니다")?;
    // 0700, created if missing — the same posture zo's
    // `ensure_credentials_parent` takes, through this window's own primitive.
    durable_file::ensure_private_directory(parent).map_err(|error| error.to_string())?;
    // Held until this function returns, which is the whole read-modify-write.
    let _lock = hold_lock(path)?;
    let mut root = read_root(path)?;
    change(&mut root);
    let mut rendered = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    rendered.push('\n');
    // Staged at 0600, fsynced, renamed, parent synced — so a reader never
    // sees half a credential file and never sees one anybody else can read.
    durable_file::replace_bytes(path, rendered.as_bytes())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// zo's `CredentialFileLock`, from this side of the fence.
///
/// The lock file stays in place permanently and the OS lock rides the open
/// descriptor: an unlink-and-recreate cycle would race, and a crash would
/// leave an existence lock nobody can clear. The guard ends the lock when it
/// drops, so a copy of the descriptor in a forked launch cannot keep it.
fn hold_lock(path: &Path) -> Result<zerocode_core::held_file_lock::HeldFileLock, String> {
    let lock_path = path.with_extension("json.lock");
    let file = durable_file::private_lock_file(&lock_path).map_err(|error| error.to_string())?;
    for attempt in 0..LOCK_TRIES {
        match file.try_lock() {
            Ok(()) => return Ok(zerocode_core::held_file_lock::HeldFileLock::locked(file)),
            Err(error) => {
                let error = std::io::Error::from(error);
                if error.kind() != std::io::ErrorKind::WouldBlock {
                    return Err(error.to_string());
                }
                if attempt + 1 < LOCK_TRIES {
                    std::thread::sleep(LOCK_WAIT);
                }
            }
        }
    }
    Err("zo 자격 증명 파일이 다른 프로세스에 잠겨 있습니다".to_string())
}

/* ---- the Code Assist project zo resolved for it ------------------------- */
//
// zo resolves the Code Assist project a login's requests are billed to once,
// and keeps the answer beside the login so the next process need not ask
// again (`oauth_store.rs`, `google_code_assist_project`). The gauge reads that
// record when Google's own answer names no project (t-72144). Read only: zo
// writes the record, replaces it, and clears it with the login.

/// The key zo files the project record under — zo's
/// `GOOGLE_CODE_ASSIST_PROJECT_KEY`. One record at a time: resolving for
/// another login or configuration replaces it.
const PROJECT_KEY: &str = "google_code_assist_project";

/// The record's field naming the configured project it was resolved under —
/// zo's `CONFIGURED_PROJECT_FIELD`. A record without it predates the field,
/// and zo trusts none of those.
const CONFIGURED_PROJECT_FIELD: &str = "configured_project";

/// Hex digits of the refresh token's SHA-256 that name a grant — zo's
/// `GRANT_FINGERPRINT_LEN`. Google refresh tokens do not rotate, so this
/// names one login across processes and holds nothing a reader could use.
const GRANT_FINGERPRINT_LEN: usize = 16;

/// The variables zo reads its configured project from, in zo's order
/// (`google_cloud_project`, `providers/gemini_code_assist.rs`).
const CONFIGURED_PROJECT_VARIABLES: [&str; 2] = ["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_PROJECT_ID"];

/// The name zo files a login's project under.
pub(crate) fn grant_fingerprint(refresh_token: &str) -> String {
    crate::artifact_runtime::sha256_hex(refresh_token.as_bytes())[..GRANT_FINGERPRINT_LEN]
        .to_string()
}

/// The Code Assist project this process's configuration names.
///
/// The window's own environment, which is launchd's when it was opened from
/// the Dock: a project named only in a shell profile is not seen here, and a
/// record zo made under it is then another configuration's.
pub(crate) fn configured_project() -> Option<String> {
    configured_project_from(|name| std::env::var(name).ok())
}

/// The configured project as zo reads it: the first of its variables that is
/// SET, trimmed, a blank one being none without asking the next. A bare
/// project number zo refuses outright, so it is no project here either.
///
/// Read through `lookup`, so the tests never touch the process environment.
pub(crate) fn configured_project_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    CONFIGURED_PROJECT_VARIABLES
        .into_iter()
        .find_map(lookup)
        .map(|value| value.trim().to_string())
        .filter(|project| {
            !project.is_empty() && !project.chars().all(|glyph| glyph.is_ascii_digit())
        })
}

/// The Code Assist project zo resolved for the login `held` while the
/// configuration named `configured`, read the way zo reads its record before
/// trusting it: the record is this login's (its grant) under this
/// configuration (its configured project), and names a project. No record,
/// another login's or another configuration's, a record of no project, a
/// login with no refresh token to name it by, or a file that does not read —
/// each is no project.
pub(crate) fn remembered_project(
    path: &Path,
    held: &Credentials,
    configured: Option<&str>,
) -> Option<String> {
    let grant = grant_fingerprint(held.refresh_token.as_deref()?);
    let root = read_root(path).ok()?;
    let record = root.get(PROJECT_KEY)?;
    if record.get("grant").and_then(serde_json::Value::as_str) != Some(grant.as_str()) {
        return None;
    }
    let recorded = match record.get(CONFIGURED_PROJECT_FIELD)? {
        serde_json::Value::Null => None,
        serde_json::Value::String(project) => Some(project.as_str()),
        _ => return None,
    };
    if recorded != configured {
        return None;
    }
    record
        .get("project")
        .and_then(serde_json::Value::as_str)
        .filter(|project| !project.is_empty())
        .map(str::to_string)
}

/* ---- what a card says about it ------------------------------------------ */

/// This machine's Google login as the settings card reads it.
///
/// Pure over its inputs — the file's word and the cached name — so the two
/// judgements a person acts on (is it renewable, does it cover the agent
/// road) are decided in one place and can be tested without a login.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Standing {
    pub(crate) signed_in: bool,
    /// Read once at sign-in and cached in this window's settings; never
    /// fetched on a paint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) email: Option<String>,
    /// Unix seconds, as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at: Option<u64>,
    pub(crate) expired: bool,
    /// Whether the access token can be renewed without a person. A login with
    /// no refresh token is one browser round trip from being dead.
    pub(crate) renewable: bool,
    pub(crate) scoped_for_agents: bool,
    /// The file both products share, for the card's hint line.
    pub(crate) store: String,
    /// Why the store could not be read, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

fn signed_out(store: String, error: Option<String>) -> Standing {
    Standing {
        signed_in: false,
        email: None,
        expires_at: None,
        expired: false,
        renewable: false,
        scoped_for_agents: true,
        store,
        error,
    }
}

/// What is on file, said as a card can say it.
pub(crate) fn standing(path: Option<&Path>, email: Option<String>, now_ms: i64) -> Standing {
    let Some(path) = path else {
        return signed_out(String::new(), Some("홈 폴더를 찾지 못했습니다".to_string()));
    };
    let store = path.to_string_lossy().into_owned();
    let held = match read(path) {
        Ok(Some(held)) => held,
        Ok(None) => return signed_out(store, None),
        Err(error) => return signed_out(store, Some(error)),
    };
    let now_seconds = u64::try_from(now_ms.max(0)).unwrap_or_default() / 1_000;
    Standing {
        signed_in: true,
        email: email.filter(|name| !name.trim().is_empty()),
        expires_at: held.expires_at,
        expired: held.expired_at(now_seconds),
        renewable: held
            .refresh_token
            .as_deref()
            .is_some_and(|token| !token.is_empty()),
        scoped_for_agents: held.covers_agent_road(),
        store,
        error: None,
    }
}

/* ---- the tokens --------------------------------------------------------- */

/// `application/x-www-form-urlencoded`, by hand — this build of reqwest
/// carries no form encoder, and the bodies on this road are four fields each.
/// Unreserved bytes ride bare; everything else is `%XX`. A refresh token is
/// URL-safe in practice, but "in practice" is not a contract.
fn form_encoded(fields: &[(&str, &str)]) -> String {
    fn escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for byte in text.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    out.push(byte as char);
                }
                other => out.push_str(&format!("%{other:02X}")),
            }
        }
        out
    }
    fields
        .iter()
        .map(|(name, value)| format!("{}={}", escape(name), escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A token response, read into the stored shape.
///
/// Google omits `refresh_token` on a repeat consent, so the one already on
/// file is carried forward — zo does the same (`into_token_set`), and without
/// it a re-login would turn a renewable login into a dead end. `scope` is
/// likewise trusted over what was ASKED for: consent is the server's answer,
/// not the client's request.
fn stored_from_token_response(
    body: &serde_json::Value,
    carried_refresh: Option<String>,
    now_ms: i64,
) -> Result<Credentials, String> {
    let access_token = body
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or("Google 토큰 응답에 access_token이 없습니다")?
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .or(carried_refresh);
    let now_seconds = u64::try_from(now_ms.max(0)).unwrap_or_default() / 1_000;
    let expires_at = body
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .map(|seconds| now_seconds.saturating_add(seconds));
    let scopes = body
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || SCOPES.iter().map(|scope| (*scope).to_string()).collect(),
            |scope| scope.split_whitespace().map(str::to_string).collect(),
        );
    Ok(Credentials {
        access_token,
        refresh_token,
        expires_at,
        scopes,
    })
}

/// Swap a refresh token for a fresh access token.
///
/// Any failure at all is "no refresh" to the caller: the road it came back on
/// already named the kind, and the token either got renewed or it did not.
pub(crate) fn refreshed_access_token(refresh_token: &str, now_ms: i64) -> Option<String> {
    let form = form_encoded(&[
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ]);
    let body = usage_http::post_form(TOKEN_URL, form, now_ms).ok()?;
    Some(
        body.get("access_token")?
            .as_str()
            .filter(|token| !token.is_empty())?
            .to_string(),
    )
}

/// Whether the stored access token has to be renewed before it is used.
///
/// The margin is what makes a token that would die mid-request get renewed
/// BEFORE the request rather than after it. Named here because the gauge
/// decides the same thing behind its own test seam
/// ([`crate::usage_antigravity`]) — one margin, two refreshers.
pub(crate) fn needs_refresh(held: &Credentials, now_ms: i64) -> bool {
    let now_seconds = u64::try_from(now_ms.max(0)).unwrap_or_default() / 1_000;
    held.access_token.is_empty()
        || held
            .expires_at
            .is_some_and(|expires| expires <= now_seconds.saturating_add(REFRESH_SLOP_SECONDS))
}

/// A bearer good right now, refreshing first when the stored one is spent.
/// The card's own road; the gauge refreshes through its wire.
///
/// Named for what it IS rather than for the field it comes out of: a gate
/// over the shipped backend refuses any credential field name there, because
/// the only thing the account roads may read out of a credentials file is who
/// logged in (`an_account_switch_clears_what_would_override_it_and_read...`).
pub(crate) fn usable_token(held: &Credentials, now_ms: i64) -> Option<String> {
    if !needs_refresh(held, now_ms) {
        return Some(held.access_token.clone());
    }
    refreshed_access_token(held.refresh_token.as_deref()?, now_ms)
}

/// Who this login belongs to, asked ONCE — at sign-in — and cached by the
/// caller. A name on a card is not worth a network round trip per paint.
pub(crate) fn email_of(access_token: &str, now_ms: i64) -> Option<String> {
    let bearer = format!("Bearer {access_token}");
    let body = usage_http::get_json(USERINFO_URL, &[("Authorization", &bearer)], now_ms).ok()?;
    body.get("email")
        .and_then(serde_json::Value::as_str)
        .filter(|email| !email.is_empty())
        .map(str::to_string)
}

/* ---- the consent round trip --------------------------------------------- */

/// PKCE's challenge: base64url(SHA-256(verifier)), no padding (RFC 7636).
fn code_challenge_s256(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// One attempt's secrets and the seat it will be answered at.
///
/// Split from the socket on purpose: the consent URL and the redirect reading
/// are the two pieces a gate can hold still, and binding a FIXED port to ask
/// them a question would make two tests — or a test and a real zo sign-in —
/// fight over one seat on the machine.
struct Attempt {
    verifier: String,
    expected_state: String,
    redirect_uri: String,
}

impl Attempt {
    fn new() -> Result<Self, String> {
        // The same fallible OS CSPRNG the hook bridge's token comes from, and
        // for the same reason: a verifier or a state from a clock is
        // guessable. 24 bytes as hex is 48 unreserved characters, inside RFC
        // 7636's 43-128 window.
        Ok(Self {
            verifier: hooks::random_token().ok_or("이 기계의 난수원을 읽지 못했습니다")?,
            expected_state: hooks::random_token().ok_or("이 기계의 난수원을 읽지 못했습니다")?,
            // `localhost`, not `127.0.0.1`: this client is registered for that
            // exact spelling and Google compares the string.
            redirect_uri: format!("http://localhost:{CALLBACK_PORT}{CALLBACK_PATH}"),
        })
    }

    /// Where the person consents.
    fn consent_url(&self) -> String {
        let query = form_encoded(&[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", &self.redirect_uri),
            ("scope", &SCOPES.join(" ")),
            ("state", &self.expected_state),
            ("code_challenge", &code_challenge_s256(&self.verifier)),
            ("code_challenge_method", "S256"),
            // Offline access is the refresh token, and `consent` is what makes
            // Google hand one over on a REPEAT sign-in — without it a
            // re-login can quietly produce a login that cannot be renewed.
            ("access_type", "offline"),
            ("prompt", "consent"),
        ]);
        format!("{AUTHORIZE_URL}?{query}")
    }

    /// The code out of one redirect target, refusing everything that is not
    /// this attempt's answer.
    ///
    /// `state` is compared because that is the whole of CSRF protection on a
    /// loopback redirect: without it any local page could drive a sign-in into
    /// this window's waiting seat.
    fn code_of(&self, target: &str) -> Result<String, String> {
        let parsed = url::Url::parse(&format!("http://localhost{target}"))
            .map_err(|_| "로그인 리다이렉트를 읽지 못했습니다".to_string())?;
        if parsed.path() != CALLBACK_PATH {
            return Err("로그인 리다이렉트 경로가 다릅니다".to_string());
        }
        let mut code = None;
        let mut state = None;
        let mut refused = None;
        for (key, value) in parsed.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "error" => refused = Some(value.into_owned()),
                _ => {}
            }
        }
        if let Some(refused) = refused {
            return Err(format!("Google이 로그인을 거절했습니다 ({refused})"));
        }
        if state.as_deref() != Some(self.expected_state.as_str()) {
            return Err("로그인 응답의 state가 맞지 않습니다".to_string());
        }
        code.filter(|code| !code.is_empty())
            .ok_or_else(|| "로그인 응답에 코드가 없습니다".to_string())
    }
}

/// One sign-in, from the bound seat to the saved token.
///
/// The seat is bound BEFORE the consent URL is handed out: a redirect that
/// arrives at a closed port is a person staring at a browser error with no
/// way to know the window was never listening.
pub(crate) struct Flow {
    attempt: Attempt,
    listener: TcpListener,
}

/// Bind the fixed callback seat with `SO_REUSEADDR`.
///
/// Not `TcpListener::bind`, and the reason is measured rather than
/// defensive: the option has to be set BEFORE the bind, and without it the
/// accepted connection from the last sign-in holds this exact local port in
/// `TIME_WAIT` for tens of seconds (verified on macOS 26 — a rebind two
/// hundred milliseconds later fails `EADDRINUSE`). The port is fixed by the
/// OAuth client's registration, so there is no other seat to fall back to:
/// 「다시 로그인」 pressed right after a sign-in would simply refuse.
///
/// A live listener still conflicts, which `SO_REUSEADDR` does not change —
/// so the caller also gets a short retry for the one case that produces one:
/// a window reloaded mid-sign-in leaves an abandoned wait holding the seat
/// until its next poll.
fn bind_callback_seat() -> std::io::Result<TcpListener> {
    bind_seat(CALLBACK_PORT)
}

/// The bind itself, with the port named — so a gate can measure the
/// `TIME_WAIT` property on an ephemeral port and never fight the real seat.
fn bind_seat(port: u16) -> std::io::Result<TcpListener> {
    let seat = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_reuse_address(true)?;
    socket.bind(&seat.into())?;
    socket.listen(8)?;
    Ok(socket.into())
}

impl Flow {
    /// Take the seat and mint this attempt's secrets.
    pub(crate) fn open() -> Result<Self, String> {
        let attempt = Attempt::new()?;
        let mut refused = None;
        let mut listener = None;
        for tried in 0..SEAT_TRIES {
            match bind_callback_seat() {
                Ok(bound) => {
                    listener = Some(bound);
                    break;
                }
                Err(error) => {
                    refused = Some(error);
                    if tried + 1 < SEAT_TRIES {
                        std::thread::sleep(SEAT_WAIT);
                    }
                }
            }
        }
        let listener = listener.ok_or_else(|| {
            let error = refused.map_or_else(String::new, |error| format!(" ({error})"));
            format!(
                "로그인 대기 포트 {CALLBACK_PORT}을(를) 열지 못했습니다 — \
                 zo 로그인이 진행 중일 수 있습니다{error}"
            )
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        Ok(Self { attempt, listener })
    }

    /// Where the person consents. Opened in the window's own browser tab by
    /// the caller, so the sign-in happens on this surface.
    pub(crate) fn consent_url(&self) -> String {
        self.attempt.consent_url()
    }

    /// Wait for the redirect, then trade the code for tokens.
    ///
    /// `Ok(None)` is a cancellation — the person pressed 취소, which is not a
    /// failure and must not be dressed as one. The ceiling is five minutes:
    /// past that the seat is worth more than the attempt.
    pub(crate) fn wait(
        self,
        cancelled: &AtomicBool,
        now_ms: i64,
    ) -> Result<Option<Credentials>, String> {
        let Some(code) = self.await_code(cancelled)? else {
            return Ok(None);
        };
        let carried = credentials_path()
            .and_then(|path| read(&path).ok().flatten())
            .and_then(|held| held.refresh_token);
        let form = form_encoded(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &self.attempt.redirect_uri),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("code_verifier", &self.attempt.verifier),
        ]);
        let body = usage_http::post_form(TOKEN_URL, form, now_ms)
            .map_err(|failure| format!("Google 토큰 교환에 실패했습니다 ({})", failure.message))?;
        let held = stored_from_token_response(&body, carried, now_ms)?;
        if held.refresh_token.is_none() {
            return Err(
                "Google이 refresh_token을 주지 않았습니다 — 다시 로그인해 주세요".to_string(),
            );
        }
        Ok(Some(held))
    }

    /// The redirect's authorization code, or `None` when the person cancelled.
    ///
    /// zo's design for its own callback (`mcp_oauth::wait_for_callback_on`,
    /// t-78016): a browser opens connections it sends nothing on yet (a
    /// preconnect) and may send the redirect on one of them later, so every
    /// accepted connection is held and read without waiting, and the first
    /// to bring a whole request line is the redirect. One pass every
    /// [`CONSENT_POLL`] looks at the cancel flag and the ceiling too: a
    /// silent connection delays neither. Before, each accepted connection
    /// was read blocking for up to two seconds — a cancel waited that long,
    /// and a preconnect that later carried the redirect was let go.
    fn await_code(&self, cancelled: &AtomicBool) -> Result<Option<String>, String> {
        let deadline = Instant::now() + CONSENT_CEILING;
        let mut held: Vec<(std::net::TcpStream, Vec<u8>)> = Vec::new();
        let (mut stream, line) = 'redirect: loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            loop {
                match self.listener.accept() {
                    // macOS hands an accepted socket the listener's
                    // non-blocking mode and Linux does not: set it, so no
                    // read waits.
                    Ok((stream, _)) => {
                        if stream.set_nonblocking(true).is_ok() {
                            if held.len() == HELD_LIMIT {
                                drop(held.remove(0));
                            }
                            held.push((stream, Vec::new()));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error.to_string()),
                }
            }
            let mut index = 0;
            while index < held.len() {
                let (stream, received) = &mut held[index];
                match read_request_line(stream, received) {
                    RequestLine::Whole(line) => break 'redirect (held.remove(index).0, line),
                    RequestLine::Partial => index += 1,
                    RequestLine::Gone => drop(held.remove(index)),
                }
            }
            if Instant::now() >= deadline {
                return Err("Google 로그인이 시간 안에 끝나지 않았습니다".to_string());
            }
            std::thread::sleep(CONSENT_POLL);
        };
        let target = line.split_whitespace().nth(1).unwrap_or_default();
        let answer = self.attempt.code_of(target);
        // The browser is answered either way — a hung tab reads as a hung
        // sign-in — and the page never carries the code it just delivered.
        let page = if answer.is_ok() {
            "로그인이 끝났습니다. 이 탭은 닫아도 됩니다."
        } else {
            "로그인을 끝내지 못했습니다. ZeroCode 설정으로 돌아가 주세요."
        };
        let body = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>ZeroCode</title>\
             <p style=\"font:14px system-ui;padding:2rem\">{page}</p>"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // A browser that never takes the page holds the sign-in no longer
        // than this.
        if stream.set_nonblocking(false).is_ok()
            && stream.set_write_timeout(Some(RESPONSE_WAIT)).is_ok()
        {
            let _ = stream.write_all(response.as_bytes());
        }
        answer.map(Some)
    }
}

/// What a held callback connection has brought so far — zo's own reading
/// (`mcp_oauth::RequestLine`).
enum RequestLine {
    /// Its first line is whole.
    Whole(String),
    /// Not yet.
    Partial,
    /// It closed, broke, or ran past [`REQUEST_LINE_LIMIT`].
    Gone,
}

/// Read what `stream` has brought without waiting, and say whether its first
/// line is whole. The request line is all this needs; a body is never read.
fn read_request_line(stream: &mut std::net::TcpStream, received: &mut Vec<u8>) -> RequestLine {
    let mut chunk = [0_u8; 1024];
    loop {
        if let Some(end) = received.iter().position(|&byte| byte == b'\n') {
            let line = String::from_utf8_lossy(&received[..end]);
            return RequestLine::Whole(line.trim_end_matches('\r').to_string());
        }
        if received.len() > REQUEST_LINE_LIMIT {
            return RequestLine::Gone;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return RequestLine::Gone,
            Ok(read) => received.extend_from_slice(&chunk[..read]),
            Err(error) => match error.kind() {
                std::io::ErrorKind::WouldBlock => return RequestLine::Partial,
                std::io::ErrorKind::Interrupted => {}
                _ => return RequestLine::Gone,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> Credentials {
        Credentials {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: Some(1_800_000_000),
            scopes: SCOPES.iter().map(|scope| (*scope).to_string()).collect(),
        }
    }

    #[test]
    fn credentials_use_the_zo_override_then_the_dot_zo_default() {
        assert_eq!(
            credentials_path_from(Some(Path::new("/config")), Some(Path::new("/home/me"))),
            Some(PathBuf::from("/config/credentials.json"))
        );
        assert_eq!(
            credentials_path_from(None, Some(Path::new("/home/me"))),
            Some(PathBuf::from("/home/me/.zo/credentials.json"))
        );
        assert_eq!(credentials_path_from(None, None), None);
    }

    /// A write puts one key back and leaves every other credential zo keeps
    /// exactly as it was — including the collection maps, which are other
    /// products' whole feature sets.
    #[test]
    fn a_write_keeps_every_other_credential_zo_holds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            r#"{
  "oauth": {"accessToken": "claude"},
  "openai_oauth": {"accessToken": "codex"},
  "openai_compat_api_keys": {"one": "key"},
  "mcp_oauth": {"server": {"accessToken": "mcp"}}
}"#,
        )
        .expect("seed");

        save(&path, &credentials()).expect("save");
        let root = read_root(&path).expect("root");
        for key in [
            "oauth",
            "openai_oauth",
            "openai_compat_api_keys",
            "mcp_oauth",
        ] {
            assert!(root.contains_key(key), "`{key}` was lost by a Google write");
        }
        assert_eq!(read(&path).expect("read"), Some(credentials()));
        // zo's own spelling, or zo reads back a login it cannot parse.
        assert!(
            root[OAUTH_KEY].get("accessToken").is_some()
                && root[OAUTH_KEY].get("refreshToken").is_some()
                && root[OAUTH_KEY].get("expiresAt").is_some(),
            "the stored entry left zo's camelCase shape: {}",
            root[OAUTH_KEY]
        );

        clear(&path).expect("clear");
        let root = read_root(&path).expect("root after clear");
        assert!(!root.contains_key(OAUTH_KEY));
        for key in [
            "oauth",
            "openai_oauth",
            "openai_compat_api_keys",
            "mcp_oauth",
        ] {
            assert!(
                root.contains_key(key),
                "`{key}` was lost by a Google logout"
            );
        }
        assert_eq!(read(&path).expect("read after clear"), None);
    }

    /// The file a login lands in is owner-only, and so is the folder it is in
    /// — this is somebody's Google refresh token.
    #[cfg(unix)]
    #[test]
    fn a_saved_login_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("zo");
        let path = root.join("credentials.json");
        save(&path, &credentials()).expect("save");
        assert_eq!(
            std::fs::metadata(&path).expect("file").permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&root).expect("dir").permissions().mode() & 0o777,
            0o700
        );
        // And the lock beside it, which zo opens too.
        assert_eq!(
            std::fs::metadata(root.join("credentials.json.lock"))
                .expect("lock")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    /// A write waits for zo's lock rather than trampling a refresh in flight.
    #[test]
    fn a_write_waits_for_the_lock_zo_takes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, "{}").expect("seed");
        let held = hold_lock(&path).expect("first hold");
        let started = Instant::now();
        let refused = save(&path, &credentials());
        assert!(
            refused.is_err(),
            "a write went through while the credential lock was held"
        );
        assert!(
            started.elapsed() >= LOCK_WAIT,
            "the write gave up without waiting at all"
        );
        drop(held);
        save(&path, &credentials()).expect("save once the lock is free");
    }

    /// A launch on another thread that forks before it execs holds a copy of
    /// every descriptor until the exec, and a flock belongs to the open file
    /// description, so a lock its holder only closed stays held by that copy
    /// (t-73907, t-74976). The duplicate here is that copy.
    #[cfg(unix)]
    #[test]
    fn a_copy_of_the_credential_lock_left_in_a_launch_does_not_hold_the_next_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        let held = hold_lock(&path).expect("first hold");
        let inherited = held.try_clone().expect("a copy of the descriptor");
        drop(held);
        let next = crate::durable_file::private_lock_file(&path.with_extension("json.lock"))
            .expect("the lock file");
        let taken = next.try_lock();
        drop(inherited);
        assert!(
            taken.is_ok(),
            "a lock its holder let go is still held by a copy"
        );
    }

    /// The store's word turned into the two judgements a person acts on.
    #[test]
    fn a_standing_names_what_is_wrong_with_the_login() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        let now_ms = 1_800_000_000_000;

        let none = standing(Some(&path), None, now_ms);
        assert!(!none.signed_in && none.error.is_none());
        assert_eq!(none.store, path.to_string_lossy());

        save(&path, &credentials()).expect("save");
        let held = standing(Some(&path), Some("me@example.com".to_string()), now_ms);
        assert!(held.signed_in && held.renewable && held.scoped_for_agents);
        assert_eq!(held.email.as_deref(), Some("me@example.com"));
        // 1_800_000_000 seconds is exactly now, so it is spent.
        assert!(held.expired);

        // A login zo made: five scopes, no agent road.
        let mut narrow = credentials();
        narrow.scopes.retain(|scope| scope != AGENT_SCOPE);
        narrow.expires_at = Some(1_900_000_000);
        save(&path, &narrow).expect("save narrow");
        let narrow = standing(Some(&path), None, now_ms);
        assert!(narrow.signed_in && !narrow.scoped_for_agents && !narrow.expired);
        assert_eq!(
            narrow.email, None,
            "a login with no cached name claimed one"
        );

        // A login that cannot be renewed is one round trip from dead.
        let mut stranded = credentials();
        stranded.refresh_token = None;
        save(&path, &stranded).expect("save stranded");
        assert!(!standing(Some(&path), None, now_ms).renewable);

        // An entry from before scopes were recorded knows nothing about its
        // own consent, and "unknown" must not read as "missing".
        let mut silent = credentials();
        silent.scopes.clear();
        save(&path, &silent).expect("save silent");
        assert!(standing(Some(&path), None, now_ms).scoped_for_agents);

        // A file that is not JSON says so instead of reading as signed out.
        std::fs::write(&path, "not json").expect("break it");
        let broken = standing(Some(&path), None, now_ms);
        assert!(!broken.signed_in && broken.error.is_some());
    }

    /// The status bar's question — is there a Google login on this machine at
    /// all (t-70039) — answered off the file alone: no home, no file, a file
    /// holding only other products' logins, and a file that is not JSON are
    /// all no login; the entry is one, whatever its clock.
    #[test]
    fn a_login_is_on_file_only_when_its_entry_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        assert!(!on_file(None), "a machine with no home read as signed in");
        assert!(!on_file(Some(&path)), "a missing file read as signed in");

        std::fs::write(&path, r#"{"oauth": {"accessToken": "claude"}}"#).expect("seed");
        assert!(
            !on_file(Some(&path)),
            "another product's login read as the Google one"
        );

        let mut spent = credentials();
        spent.expires_at = Some(1);
        save(&path, &spent).expect("save");
        assert!(on_file(Some(&path)), "a Google login on file did not count");

        std::fs::write(&path, "not json").expect("break it");
        assert!(!on_file(Some(&path)), "a broken file read as signed in");
    }

    /// zo names a grant by the first sixteen hex digits of its refresh token's
    /// SHA-256 (`oauth_store::grant_fingerprint`), so a record it wrote is
    /// found under exactly that name — pinned here by the digest of a
    /// synthetic token, not by calling the same function twice.
    #[test]
    fn a_grant_is_named_the_way_zo_names_it() {
        assert_eq!(grant_fingerprint("rt"), "cdffd5dd8ca8126c");
    }

    /// zo reads its configured project from `GOOGLE_CLOUD_PROJECT`, else
    /// `GOOGLE_CLOUD_PROJECT_ID` — the first one SET, trimmed, a blank one
    /// being none (`google_cloud_project`). A bare project number is refused.
    #[test]
    fn the_configured_project_is_zos_variable_in_zos_order() {
        let from = |set: &[(&str, &str)]| {
            configured_project_from(|name| {
                set.iter()
                    .find(|(variable, _)| *variable == name)
                    .map(|&(_, value)| value.to_string())
            })
        };
        assert_eq!(from(&[]), None);
        assert_eq!(
            from(&[("GOOGLE_CLOUD_PROJECT", " my-project ")]).as_deref(),
            Some("my-project")
        );
        assert_eq!(
            from(&[("GOOGLE_CLOUD_PROJECT_ID", "id-project")]).as_deref(),
            Some("id-project")
        );
        assert_eq!(
            from(&[
                ("GOOGLE_CLOUD_PROJECT", "first"),
                ("GOOGLE_CLOUD_PROJECT_ID", "second"),
            ])
            .as_deref(),
            Some("first")
        );
        assert_eq!(
            from(&[
                ("GOOGLE_CLOUD_PROJECT", "  "),
                ("GOOGLE_CLOUD_PROJECT_ID", "second"),
            ]),
            None,
            "a blank first variable handed over to the second, which zo does not"
        );
        assert_eq!(
            from(&[("GOOGLE_CLOUD_PROJECT", "123456789012")]),
            None,
            "a project number was taken for a project id"
        );
    }

    /// zo remembers the project it resolved for a login under the login's
    /// grant and the project the configuration named then
    /// (`google_code_assist_project`). The record is THIS login's under THIS
    /// configuration only — the same match zo makes before it trusts it.
    #[test]
    fn zos_project_record_is_read_for_this_login_and_configuration_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("credentials.json");
        let held = credentials();
        let grant = grant_fingerprint("rt");
        let with_record = |record: serde_json::Value| {
            std::fs::write(
                &path,
                serde_json::json!({ "google_code_assist_project": record }).to_string(),
            )
            .expect("credentials");
        };

        assert_eq!(remembered_project(&path, &held, None), None, "no file");
        with_record(
            serde_json::json!({ "grant": grant, "configured_project": null, "project": "zo-project" }),
        );
        assert_eq!(
            remembered_project(&path, &held, None).as_deref(),
            Some("zo-project")
        );
        assert_eq!(
            remembered_project(&path, &held, Some("configured")),
            None,
            "a record made under no configured project answered for one"
        );

        with_record(serde_json::json!({
            "grant": grant, "configured_project": "configured", "project": "configured-zo-project"
        }));
        assert_eq!(
            remembered_project(&path, &held, Some("configured")).as_deref(),
            Some("configured-zo-project")
        );
        assert_eq!(remembered_project(&path, &held, None), None);

        for (record, why) in [
            (
                serde_json::json!({ "grant": "0123456789abcdef", "configured_project": null, "project": "theirs" }),
                "another login's record",
            ),
            (
                serde_json::json!({ "grant": grant, "project": "before-configurations" }),
                "a record that names no configuration",
            ),
            (
                serde_json::json!({ "grant": grant, "configured_project": null, "project": null }),
                "a record of no project",
            ),
            (
                serde_json::json!({ "grant": grant, "configured_project": null, "project": "" }),
                "a record of an empty project",
            ),
        ] {
            with_record(record);
            assert_eq!(
                remembered_project(&path, &held, None),
                None,
                "{why} was read"
            );
        }

        // A login with no refresh token has no grant: zo keeps its project for
        // one process only.
        with_record(
            serde_json::json!({ "grant": grant, "configured_project": null, "project": "zo-project" }),
        );
        let mut bearer_only = credentials();
        bearer_only.refresh_token = None;
        assert_eq!(remembered_project(&path, &bearer_only, None), None);

        std::fs::write(&path, "not json").expect("break it");
        assert_eq!(remembered_project(&path, &held, None), None);
    }

    /// The hand-rolled form body escapes what a token may legally carry.
    #[test]
    fn the_form_body_escapes_reserved_bytes() {
        assert_eq!(
            form_encoded(&[("grant_type", "refresh_token"), ("t", "a/b+c=&d 한")]),
            "grant_type=refresh_token&t=a%2Fb%2Bc%3D%26d%20%ED%95%9C"
        );
    }

    /// The consent URL carries PKCE, the fixed loopback seat, the agent scope,
    /// and the two parameters that make a refresh token arrive.
    #[test]
    fn the_consent_url_asks_for_offline_agent_scoped_consent() {
        let attempt = Attempt::new().expect("attempt");
        let url = attempt.consent_url();
        assert!(url.starts_with(&format!("{AUTHORIZE_URL}?")));
        for spelled in [
            "response_type=code",
            "code_challenge_method=S256",
            "access_type=offline",
            "prompt=consent",
            "redirect_uri=http%3A%2F%2Flocalhost%3A51121%2Foauth-callback",
            "auth%2Faicode",
        ] {
            assert!(
                url.contains(spelled),
                "the consent URL lost `{spelled}`:\n{url}"
            );
        }
        assert!(
            url.contains(&format!(
                "code_challenge={}",
                code_challenge_s256(&attempt.verifier)
            )),
            "the challenge is not this attempt's verifier:\n{url}"
        );
        assert!(
            !url.contains(&attempt.verifier),
            "the verifier itself went to the browser, which defeats PKCE"
        );
        // RFC 7636's window, and the challenge is base64url with no padding.
        assert!((43..=128).contains(&attempt.verifier.len()));
        assert!(
            !code_challenge_s256(&attempt.verifier).contains(['=', '+', '/']),
            "the challenge is not base64url"
        );
        // Two attempts never share a verifier or a state.
        let other = Attempt::new().expect("second attempt");
        assert_ne!(attempt.verifier, other.verifier);
        assert_ne!(attempt.expected_state, other.expected_state);
    }

    /// The seat can be retaken while the LAST sign-in's connection is still
    /// lingering.
    ///
    /// That is what `SO_REUSEADDR` before the bind buys, and it is what
    /// 「다시 로그인」 pressed right after a sign-in depends on: the redirect
    /// port is fixed by the OAuth client's registration, so a refused bind has
    /// nowhere else to go. Measured on macOS 26 — without the option, a rebind
    /// two hundred milliseconds after the connection closed answers
    /// `EADDRINUSE`. On a port of its own, so no test ever contends for the
    /// real callback seat or for a zo sign-in in flight — and not one the
    /// kernel hands out on its own (t-74682): the port is released before it is
    /// bound again, and another test's `bind(0)` can take it in that gap. The
    /// port is a quiet one, under the dynamic range, and its lease is kept to
    /// the end of the test so no other test is given it meanwhile (t-74864).
    #[test]
    fn the_callback_seat_can_be_retaken_while_the_last_one_lingers() {
        let (lease, first) = crate::test_host::quiet_ports::QuietPort::claim(bind_seat);
        let port = lease.port();
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).expect("a browser");
        let (accepted, _) = first.accept().expect("the redirect");
        // Closed from THIS side first, so it is our end of that connection
        // that lingers on the port a second sign-in needs.
        drop(accepted);
        drop(first);
        drop(client);
        if let Err(error) = bind_seat(port) {
            panic!("the seat could not be retaken while the last one lingered: {error}");
        }
    }

    /// A connection that says nothing does not end the sign-in.
    ///
    /// A browser opens sockets it never sends on (a preconnect), and the
    /// redirect arrives on another; on macOS the accepted socket also inherits
    /// the waiting seat's non-blocking mode, so a read made before the
    /// redirect's bytes land answered `WouldBlock` and the sign-in failed with
    /// it (l81g, 2026-10-27). Here the silent connection closes its side first,
    /// so nothing waits on a clock: the first read of it is an end, not a request.
    #[test]
    fn a_connection_that_says_nothing_is_let_go_and_the_redirect_after_it_signs_in() {
        let (lease, listener) = crate::test_host::quiet_ports::QuietPort::claim(bind_seat);
        let port = lease.port();
        listener
            .set_nonblocking(true)
            .expect("the seat waits without blocking");
        let flow = Flow {
            attempt: Attempt::new().expect("attempt"),
            listener,
        };
        let state = flow.attempt.expected_state.clone();
        let silent = std::net::TcpStream::connect(("127.0.0.1", port)).expect("a preconnect");
        silent
            .shutdown(std::net::Shutdown::Write)
            .expect("the preconnect says nothing");
        let browser = std::thread::spawn(move || {
            let mut redirect =
                std::net::TcpStream::connect(("127.0.0.1", port)).expect("the redirect");
            // Only a hang guard: the answer comes as soon as the seat takes it.
            redirect
                .set_read_timeout(Some(Duration::from_secs(60)))
                .expect("a bounded read");
            write!(
                redirect,
                "GET /oauth-callback?code=c-1&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            )
            .expect("the redirect is sent");
            let mut page = String::new();
            redirect.read_to_string(&mut page).expect("the page");
            page
        });
        let cancelled = AtomicBool::new(false);
        let code = flow.await_code(&cancelled);
        // The seat closes before the browser is waited for: a sign-in that
        // ended without taking the redirect leaves it unanswered in the
        // backlog, and closing the seat ends its read instead of hanging here.
        drop(flow);
        drop(silent);
        assert_eq!(code, Ok(Some("c-1".to_string())));
        let page = browser.join().expect("the browser");
        assert!(
            page.starts_with("HTTP/1.1 200 OK") && !page.contains("c-1"),
            "{page}"
        );
    }

    /// A seat for one test: the flow on a quiet port, its listener not
    /// blocking, as `begin` lays it.
    fn quiet_flow() -> (crate::test_host::quiet_ports::QuietPort, Flow) {
        let (lease, listener) = crate::test_host::quiet_ports::QuietPort::claim(bind_seat);
        listener
            .set_nonblocking(true)
            .expect("the seat waits without blocking");
        let flow = Flow {
            attempt: Attempt::new().expect("attempt"),
            listener,
        };
        (lease, flow)
    }

    /// t-78016, zo's design for its own callback: a browser's preconnect can
    /// stay silent past any wait and then carry the redirect itself. The
    /// connection is held, not let go, and the sign-in ends on it. Before,
    /// a silent connection was read for two seconds and dropped, and the
    /// redirect it carried later never landed.
    #[test]
    fn a_preconnect_that_carries_the_redirect_later_signs_in() {
        let (lease, flow) = quiet_flow();
        let port = lease.port();
        let state = flow.attempt.expected_state.clone();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let browser = std::thread::spawn(move || {
            let mut preconnect =
                std::net::TcpStream::connect(("127.0.0.1", port)).expect("a preconnect");
            // Past the two seconds a silent connection used to be given.
            std::thread::sleep(Duration::from_millis(2_500));
            let _ = write!(
                preconnect,
                "GET /oauth-callback?code=c-1&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            let _ = preconnect.set_read_timeout(Some(Duration::from_secs(60)));
            let mut page = String::new();
            let _ = preconnect.read_to_string(&mut page);
            page
        });
        // Only a hang guard: a seat that let the preconnect go waits for a
        // redirect that never comes.
        let _guard = {
            let cancelled = std::sync::Arc::clone(&cancelled);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(15));
                cancelled.store(true, Ordering::Relaxed);
            })
        };
        let code = flow.await_code(&cancelled);
        drop(flow);
        assert_eq!(code, Ok(Some("c-1".to_string())));
        let page = browser.join().expect("the browser");
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
    }

    /// t-78016: a cancel is taken at the next look, whatever connection is
    /// open and silent. Before, a silent connection held the seat in a
    /// blocking read for up to two seconds, and the cancel waited for it.
    #[test]
    fn a_cancel_is_taken_at_once_while_a_connection_stays_silent() {
        let (lease, flow) = quiet_flow();
        let silent =
            std::net::TcpStream::connect(("127.0.0.1", lease.port())).expect("a preconnect");
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let pressed = {
            let cancelled = std::sync::Arc::clone(&cancelled);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                cancelled.store(true, Ordering::Relaxed);
                Instant::now()
            })
        };
        let code = flow.await_code(&cancelled);
        let ended = Instant::now();
        let pressed = pressed.join().expect("the press");
        drop(silent);
        assert_eq!(code, Ok(None));
        assert!(
            ended.saturating_duration_since(pressed) < Duration::from_secs(1),
            "the cancel waited {:?} on a silent connection",
            ended.saturating_duration_since(pressed)
        );
    }

    /// A redirect is accepted only when it is THIS attempt's answer.
    #[test]
    fn a_redirect_is_refused_unless_it_carries_this_attempts_state() {
        let attempt = Attempt::new().expect("attempt");
        let state = attempt.expected_state.clone();
        assert_eq!(
            attempt.code_of(&format!("/oauth-callback?code=abc&state={state}")),
            Ok("abc".to_string())
        );
        for refused in [
            "/oauth-callback?code=abc&state=somebody-else".to_string(),
            format!("/oauth-callback?state={state}"),
            format!("/oauth-callback?code=&state={state}"),
            format!("/elsewhere?code=abc&state={state}"),
            format!("/oauth-callback?error=access_denied&state={state}"),
        ] {
            assert!(
                attempt.code_of(&refused).is_err(),
                "`{refused}` was taken as a good sign-in"
            );
        }
        // The refusal names Google's word without carrying anything secret.
        let said = attempt
            .code_of(&format!(
                "/oauth-callback?error=access_denied&state={state}"
            ))
            .expect_err("a refusal");
        assert!(said.contains("access_denied") && !said.contains(&state));
        // A code that arrives percent-encoded is decoded, not passed through.
        assert_eq!(
            attempt.code_of(&format!("/oauth-callback?code=a%2Fb&state={state}")),
            Ok("a/b".to_string())
        );
    }

    /// A token response becomes the stored shape, carrying forward the
    /// refresh token Google omits on a repeat consent.
    #[test]
    fn a_token_response_carries_the_refresh_token_google_omits() {
        let now_ms = 1_800_000_000_000;
        let full = stored_from_token_response(
            &serde_json::json!({
                "access_token": "fresh",
                "refresh_token": "renewed",
                "expires_in": 3599,
                "scope": "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/aicode",
            }),
            Some("old".to_string()),
            now_ms,
        )
        .expect("full response");
        assert_eq!(full.access_token, "fresh");
        assert_eq!(full.refresh_token.as_deref(), Some("renewed"));
        assert_eq!(full.expires_at, Some(1_800_003_599));
        assert!(full.covers_agent_road());

        // No `refresh_token` and no `scope`: the old refresh token stays and
        // the scopes fall back to what was asked for.
        let repeat = stored_from_token_response(
            &serde_json::json!({ "access_token": "fresh", "expires_in": 60 }),
            Some("old".to_string()),
            now_ms,
        )
        .expect("repeat response");
        assert_eq!(repeat.refresh_token.as_deref(), Some("old"));
        assert_eq!(repeat.scopes.len(), SCOPES.len());

        // The server's answer wins over the request: consent that came back
        // narrower is recorded narrower, so the card can say so.
        let narrowed = stored_from_token_response(
            &serde_json::json!({
                "access_token": "fresh",
                "scope": "https://www.googleapis.com/auth/cloud-platform",
            }),
            None,
            now_ms,
        )
        .expect("narrowed response");
        assert!(!narrowed.covers_agent_road());
        assert_eq!(narrowed.expires_at, None);

        assert!(
            stored_from_token_response(&serde_json::json!({}), None, now_ms).is_err(),
            "a response with no access token was accepted"
        );
    }

    /// A token still in date is used as it is; a spent one is refreshed, and
    /// a spent one with nothing to refresh with is nothing.
    #[test]
    fn a_spent_token_needs_a_refresh_and_a_fresh_one_does_not() {
        let now_ms = 1_800_000_000_000;
        let mut held = credentials();
        held.expires_at = Some(1_800_000_000 + REFRESH_SLOP_SECONDS + 1);
        assert_eq!(usable_token(&held, now_ms).as_deref(), Some("at"));
        // Inside the margin counts as spent — a token that dies mid-request is
        // renewed before the request, not after it.
        held.expires_at = Some(1_800_000_000 + REFRESH_SLOP_SECONDS);
        held.refresh_token = None;
        assert_eq!(usable_token(&held, now_ms), None);
    }
}
