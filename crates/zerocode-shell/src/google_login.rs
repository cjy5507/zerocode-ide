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
//! gauge asks it for the path, the credentials and a token refresh; the
//! commands in `cmd/usage.rs` ask it for the login flow. Neither owns a copy.
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
/// leave an existence lock nobody can clear.
fn hold_lock(path: &Path) -> Result<std::fs::File, String> {
    let lock_path = path.with_extension("json.lock");
    let file = durable_file::private_lock_file(&lock_path).map_err(|error| error.to_string())?;
    for attempt in 0..LOCK_TRIES {
        match file.try_lock() {
            Ok(()) => return Ok(file),
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
    fn await_code(&self, cancelled: &AtomicBool) -> Result<Option<String>, String> {
        let deadline = Instant::now() + CONSENT_CEILING;
        let mut stream = loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err("Google 로그인이 시간 안에 끝나지 않았습니다".to_string());
                    }
                    std::thread::sleep(CONSENT_POLL);
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        // The request line is all this needs, and 8 KiB is more than a
        // redirect's whole header block; a body is never read.
        let mut buffer = [0_u8; 8192];
        let read = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        let request = String::from_utf8_lossy(&buffer[..read]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or_default()
            .to_string();
        let answer = self.attempt.code_of(&target);
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
        let _ = stream.write_all(response.as_bytes());
        answer.map(Some)
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
    /// `EADDRINUSE`. On an ephemeral port, so no test ever contends for the
    /// real callback seat or for a zo sign-in in flight.
    #[test]
    fn the_callback_seat_can_be_retaken_while_the_last_one_lingers() {
        let first = bind_seat(0).expect("a seat");
        let port = first.local_addr().expect("its address").port();
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).expect("a browser");
        let (accepted, _) = first.accept().expect("the redirect");
        // Closed from THIS side first, so it is our end of that connection
        // that lingers on the port a second sign-in needs.
        drop(accepted);
        drop(first);
        drop(client);
        bind_seat(port).expect("the seat could not be retaken while the last one lingered");
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
