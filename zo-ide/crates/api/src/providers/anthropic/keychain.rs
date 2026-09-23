//! Claude Code keychain session credentials — read, evaluate, refresh, write back.
//!
//! Mirrors the Claude Code CLI's own OAuth mechanism. The macOS keychain item
//! `Claude Code-credentials` holds `{"claudeAiOauth": {accessToken, refreshToken,
//! expiresAt (Unix ms), scopes, …}}`. When the access token expires, Claude Code
//! refreshes it against the shared token endpoint (`client_id` `9d1c250a…`) and
//! writes the new token set back to the keychain. Zo previously stopped at
//! "expired → fall back", which stranded every session on a scope-less fallback
//! token whenever the desktop app wasn't around to refresh — the recurring
//! "keychain token expired / lacks user:inference" warnings. This module
//! completes the parity: expired + refresh token present → refresh → write back
//! → use. The write-back keeps the keychain the single source of truth shared
//! with Claude Code (required if the server rotates refresh tokens: without it,
//! consuming the keychain's refresh token would strand Claude Code itself).
//!
//! Living in the `api` crate (not the CLI) so the sub-agent provider path uses
//! the *same* resolution chain as the interactive client instead of skipping
//! the keychain.
//!
//! ## Managed account directories (`CLAUDE_CONFIG_DIR`)
//!
//! Claude Code keeps one keychain item for the DEFAULT home, but a launch that
//! names `CLAUDE_CONFIG_DIR` (how zerocode-IDE runs one account per pane) keeps
//! that account's OAuth bundle in `$CLAUDE_CONFIG_DIR/.credentials.json` — the
//! same `{"claudeAiOauth": …}` blob, on disk. When that file exists it is the
//! blob source and the refresh write-back target instead of the keychain, so an
//! account chosen in the IDE is the account zo speaks as. Everything else —
//! evaluation, refresh, the process cache, the zo-store mirror — is shared.

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use core_types::{OAuthConfig, OAuthRefreshRequest};
use serde_json::Value;

use super::{AnthropicClient, AuthSource, OAuthTokenSet, read_base_url};
use crate::credential::CredentialMiss;
use crate::error::ApiError;
use crate::providers::refresh_gate;

/// Why a Claude Code login that is there could not be used, in the words a
/// model list shows (`zo models`) — each names the way back in.
const EXPIRED_REFRESH_REFUSED: &str = "the Claude Code sign-in expired and its refresh token was refused \
     (superseded or revoked) — sign in again with `claude` or `zo login claude`";
const EXPIRED_REFRESH_COOLING: &str =
    "the Claude Code sign-in expired and could not be refreshed a moment ago; the next connection tries again";
const EXPIRED_NO_REFRESH_TOKEN: &str = "the Claude Code sign-in expired and holds no refresh token — \
     sign in again with `claude` or `zo login claude`";
const MISSING_INFERENCE_SCOPE: &str =
    "the Claude Code sign-in lacks the user:inference scope — sign in again with `claude`";
const KEYCHAIN_UNANSWERED: &str =
    "the macOS keychain did not answer for the Claude Code sign-in (locked, or access refused)";
const LOGIN_NOT_A_DOCUMENT: &str =
    "the Claude Code sign-in on this machine is not a readable login — sign in again with `claude`";

/// Keychain service name Claude Code stores its OAuth bundle under.
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Hex digits of the config directory's SHA-256 that the CLI appends to the
/// service name for a managed directory's login (`Claude Code-credentials-<8>`,
/// Claude Code 2.1.261+; the window seeds the same name).
const SCOPED_SERVICE_HASH_LEN: usize = 8;

/// The keychain service a `CLAUDE_CONFIG_DIR` login is filed under: the
/// unscoped name plus the first eight hex digits of the directory path's
/// SHA-256 — the rule the CLI reads by and the window seeds by, so all three
/// look in one place.
fn scoped_keychain_service(config_dir: &std::ffi::OsStr) -> String {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(config_dir.to_string_lossy().as_bytes()));
    format!("{KEYCHAIN_SERVICE}-{}", &digest[..SCOPED_SERVICE_HASH_LEN])
}

/// `claudeAiOauth.expiresAt` of a blob, the stamp every refresh advances.
fn oauth_expires_at(blob: &Value) -> Option<u64> {
    blob.get("claudeAiOauth")
        .and_then(|oauth| oauth.get("expiresAt"))
        .and_then(Value::as_u64)
}

/// Which copy of a managed directory's login to believe when both the
/// `.credentials.json` beside it and the CLI's scoped keychain item answer:
/// the one refreshed more recently, by `expiresAt`. A tie — or a blob with no
/// stamp on either side — goes to the keychain, the store the CLI writes to
/// first since 2.1.261; the file is what it (and zo) write back second. Before
/// this fold zo read the file alone, so a rotation the CLI had already written
/// to the keychain left zo refreshing a superseded grant (`invalid_grant`,
/// 2026-09-10).
fn freshest_blob(file: Option<Value>, scoped: Option<Value>) -> Option<Value> {
    match (file, scoped) {
        (None, None) => None,
        (Some(file), None) => Some(file),
        (None, Some(scoped)) => Some(scoped),
        (Some(file), Some(scoped)) => {
            let file_at = oauth_expires_at(&file);
            let scoped_at = oauth_expires_at(&scoped);
            if file_at > scoped_at { Some(file) } else { Some(scoped) }
        }
    }
}

/// Parse what `security find-generic-password -w` printed. A value that is not
/// a document — the 128-byte cut-off a prompt-fed write left behind — is no
/// login at all: the resolution falls through to the next rung rather than
/// failing, and the explanation says the item is there and unreadable
/// ([`BlobRead::Unusable`]) rather than that nothing is kept.
fn parse_keychain_blob(raw: &str) -> Option<Value> {
    serde_json::from_str(raw.trim()).ok()
}

/// Treat a token expiring within this window as already expired and refresh it
/// proactively, instead of letting the request race the boundary and 401.
/// Milliseconds because the keychain's `expiresAt` is Unix ms; mirrors the
/// 60-second `OAUTH_EXPIRY_BUFFER_SECS` used for zo-saved tokens.
const KEYCHAIN_EXPIRY_BUFFER_MS: u64 = 60_000;

/// Kill switch: set `ZO_DISABLE_KEYCHAIN=1` to skip the Claude Code keychain
/// entirely (also keeps unit tests hermetic on developer machines where the
/// real keychain item exists).
const DISABLE_KEYCHAIN_ENV: &str = "ZO_DISABLE_KEYCHAIN";

/// The official Claude Code subscription OAuth application. `platform.claude.com`
/// is the developer/console flow, which mints tokens the server refuses to grant
/// `user:inference` on — every `/v1/messages` then 403s `OAuth token does not
/// meet scope requirement`. The subscription flow authorizes on `claude.ai` and
/// exchanges/refreshes on `console.anthropic.com`; both share this client id,
/// which is also the client id the keychain's refresh token was minted for.
#[must_use]
pub fn claude_code_oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: String::from("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
        authorize_url: String::from("https://claude.ai/oauth/authorize"),
        token_url: String::from("https://console.anthropic.com/v1/oauth/token"),
        callback_port: None,
        manual_redirect_url: None,
        scopes: vec![
            String::from("user:profile"),
            String::from("user:inference"),
            String::from("user:sessions:claude_code"),
            String::from("org:create_api_key"),
            String::from("user:mcp_servers"),
            String::from("user:file_upload"),
        ],
        client_secret: None,
    }
}

/// A usable Claude Code session from its managed file or keychain: the bearer
/// plus its expiry so the caller can schedule a proactive re-read before the
/// next lapse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainSession {
    pub access_token: String,
    /// Unix milliseconds, when the blob records one.
    pub expires_at_ms: Option<u64>,
    /// Subscription the grant was minted for (`"max"`, `"team"`, …), verbatim
    /// from the blob's `subscriptionType`.
    ///
    /// Anthropic's usage endpoint answers about the quota, never about the
    /// person — no email, no plan — so the only place a session can be
    /// *labelled* from is the credential that opened it. Read here because this
    /// is the one function that already holds the blob; a second reader would
    /// mean a second `security(1)` fork per lookup.
    pub plan: Option<String>,
}

/// Identity of the IDE-managed Claude credential file at one resolution.
/// Comparing this value costs one metadata lookup and no credential read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCredentialsStamp {
    path: std::path::PathBuf,
    modified: std::time::SystemTime,
    len: u64,
}

/// `subscriptionType` from a `claudeAiOauth` object, ignoring the empty string
/// a signed-out blob can leave behind.
fn blob_plan(oauth: &Value) -> Option<String> {
    oauth
        .get("subscriptionType")
        .and_then(Value::as_str)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string)
}

/// Process-wide memo of the last real keychain read.
///
/// Every credential lookup previously forked `security(1)` — at startup, on
/// every model swap's binding rebuild, once per turn while auth sat in
/// fallback, and from the model picker on the TUI thread. Each fork is a
/// synchronous child-process round-trip that can stall for seconds when
/// `securityd` is slow (first access from a freshly deployed binary re-runs
/// ACL evaluation; a pending keychain authorization prompt blocks
/// indefinitely) — the main-thread `posix_spawn`/`poll` stacks the freeze
/// watchdog kept capturing. The memo serves repeat lookups in-process:
/// - a usable session is reused until its recorded expiry enters the
///   proactive refresh buffer (sessions without a recorded expiry are re-read
///   on a fixed cadence);
/// - a miss (absent blob, missing scope, failed refresh) is negative-cached
///   briefly so a machine without Claude Code credentials doesn't re-fork per
///   turn;
/// - [`invalidate_claude_code_keychain_cache`] forces the next lookup through
///   to the keychain (401 recovery must never be served a cached bearer).
struct KeychainCacheEntry {
    /// The session, or why there was none — a miss keeps its reason, so a
    /// cached answer says "expired and refused" as the read did, not just
    /// "nothing".
    session: Result<KeychainSession, CredentialMiss>,
    read_at: Instant,
}

static KEYCHAIN_SESSION_CACHE: Mutex<Option<KeychainCacheEntry>> = Mutex::new(None);
/// Single-flight for the `security` fork + optional network refresh: parallel
/// resolvers (turn boundary, model picker, sub-agent spawn) coalesce on one
/// read instead of forking a `security` process each.
static KEYCHAIN_READ_FLIGHT: Mutex<()> = Mutex::new(());

/// How long a *miss* is trusted before the keychain is consulted again. The
/// fallback auth path re-probes the keychain every turn to recover without a
/// restart; this bounds that recovery latency while capping the fork rate.
const KEYCHAIN_NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(60);
/// Re-read cadence for a usable session whose blob records no `expiresAt`:
/// trust it, but notice an external rotation within this window.
const KEYCHAIN_NO_EXPIRY_RECHECK: Duration = Duration::from_secs(15 * 60);

/// Shape of a cache entry as the freshness rule sees it.
enum CachedSessionShape {
    /// The keychain had no usable session at the last read.
    Miss,
    /// A usable session whose blob records `expiresAt` (Unix ms).
    ExpiringAt(u64),
    /// A usable session without a recorded expiry.
    NoExpiry,
}

/// What a cache lookup produced: a fresh answer (which may itself be a cached
/// miss), or nothing servable — the keychain must actually be read.
#[derive(Debug, PartialEq, Eq)]
enum KeychainCacheLookup {
    Fresh(Result<KeychainSession, CredentialMiss>),
    Stale,
}

/// Pure freshness rule for the cache entry — split out for unit tests.
fn keychain_cache_entry_fresh(shape: &CachedSessionShape, age: Duration, now_ms: u64) -> bool {
    match shape {
        CachedSessionShape::Miss => age < KEYCHAIN_NEGATIVE_CACHE_TTL,
        // Usable session with a recorded expiry: serve it until the proactive
        // buffer would refresh it anyway, so refresh timing is unchanged.
        CachedSessionShape::ExpiringAt(expires_at_ms) => {
            now_ms.saturating_add(KEYCHAIN_EXPIRY_BUFFER_MS) <= *expires_at_ms
        }
        CachedSessionShape::NoExpiry => age < KEYCHAIN_NO_EXPIRY_RECHECK,
    }
}

/// Drop the memo so the next lookup reads the keychain again.
pub fn invalidate_claude_code_keychain_cache() {
    *KEYCHAIN_SESSION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

fn cached_keychain_session() -> KeychainCacheLookup {
    let guard = KEYCHAIN_SESSION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = guard.as_ref() else {
        return KeychainCacheLookup::Stale;
    };
    let shape = match &entry.session {
        Err(_) => CachedSessionShape::Miss,
        Ok(session) => session
            .expires_at_ms
            .map_or(CachedSessionShape::NoExpiry, CachedSessionShape::ExpiringAt),
    };
    if keychain_cache_entry_fresh(&shape, entry.read_at.elapsed(), now_unix_millis()) {
        KeychainCacheLookup::Fresh(entry.session.clone())
    } else {
        KeychainCacheLookup::Stale
    }
}

fn store_keychain_session(session: &Result<KeychainSession, CredentialMiss>) {
    *KEYCHAIN_SESSION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(KeychainCacheEntry {
        session: session.clone(),
        read_at: Instant::now(),
    });
}

/// Result of inspecting a Claude Code keychain credential blob.
#[derive(Debug, PartialEq, Eq)]
enum KeychainOutcome {
    /// Usable, unexpired session token carrying `user:inference`.
    Usable(String),
    /// Token past (or within the buffer of) its `expiresAt`.
    Expired,
    /// Token present and unexpired but its `scopes` list omits
    /// `user:inference`, so `/v1/messages` would 403 — unusable for inference.
    MissingInferenceScope,
    /// No usable `claudeAiOauth.accessToken` in the blob.
    Absent,
}

/// Pure evaluation of a parsed keychain JSON blob against the current time
/// (Unix milliseconds). Split out from the `security` shell-out so the expiry
/// and scope rules are unit-testable without touching the real keychain.
fn evaluate_keychain_credentials(creds: &Value, now_ms: u64) -> KeychainOutcome {
    let Some(oauth) = creds.get("claudeAiOauth") else {
        return KeychainOutcome::Absent;
    };
    let Some(token) = oauth.get("accessToken").and_then(Value::as_str) else {
        return KeychainOutcome::Absent;
    };
    if token.is_empty() {
        return KeychainOutcome::Absent;
    }
    if let Some(expires_at) = oauth.get("expiresAt").and_then(Value::as_u64) {
        if now_ms.saturating_add(KEYCHAIN_EXPIRY_BUFFER_MS) > expires_at {
            return KeychainOutcome::Expired;
        }
    }
    // When a `scopes` list is present, require `user:inference`; an absent list
    // is treated permissively (older blobs predate the field).
    if let Some(scopes) = oauth.get("scopes").and_then(Value::as_array) {
        let has_inference = scopes
            .iter()
            .filter_map(Value::as_str)
            .any(|scope| scope == "user:inference");
        if !has_inference {
            return KeychainOutcome::MissingInferenceScope;
        }
    }
    KeychainOutcome::Usable(token.to_string())
}

/// Read the Claude Code session, preferring the IDE-managed credentials file
/// and otherwise refreshing the keychain session when expired — the same
/// lifecycle Claude Code itself runs. Returns `None` when the selected source
/// has no usable bundle and refresh is impossible/failed.
#[must_use]
pub fn read_claude_code_keychain_session() -> Option<KeychainSession> {
    read_claude_code_keychain_session_explained().ok()
}

/// [`read_claude_code_keychain_session`], and when there is no session, why:
/// [`CredentialMiss::Absent`] when this source holds no login at all,
/// [`CredentialMiss::Unusable`] when it holds one that could not be used — a
/// session that expired and would not refresh, a login without the inference
/// scope, a keychain that would not answer.
pub fn read_claude_code_keychain_session_explained() -> Result<KeychainSession, CredentialMiss> {
    // `ZO_DISABLE_KEYCHAIN` disables the operating-system keychain, not an
    // explicitly handed-off credentials file. Managed files also bypass the
    // keychain memo: the runtime owns the cheaper `(mtime, len)` cache and only
    // calls this reader after that stamp changes.
    let managed_config = managed_config_dir_selected();
    if managed_config {
        // A managed account is never mirrored into zo's own store, whether its
        // login came from the file or from the CLI's scoped keychain item.
        return read_claude_code_keychain_session_uncached(true);
    }
    if std::env::var_os(DISABLE_KEYCHAIN_ENV).is_some() {
        return Err(CredentialMiss::Absent);
    }
    if let KeychainCacheLookup::Fresh(cached) = cached_keychain_session() {
        return cached;
    }
    let _flight = KEYCHAIN_READ_FLIGHT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A resolver that lost the flight race finds the winner's result here
    // instead of forking a second `security` process.
    if let KeychainCacheLookup::Fresh(cached) = cached_keychain_session() {
        return cached;
    }
    let session = read_claude_code_keychain_session_uncached(false);
    store_keychain_session(&session);
    session
}

fn read_claude_code_keychain_session_uncached(
    managed_file: bool,
) -> Result<KeychainSession, CredentialMiss> {
    let blob = match read_keychain_blob_answer() {
        BlobRead::Found(blob) => blob,
        BlobRead::Absent => return Err(CredentialMiss::Absent),
        BlobRead::Unusable(why) => return Err(CredentialMiss::Unusable(why.to_string())),
    };
    let now_ms = now_unix_millis();
    match evaluate_keychain_credentials(&blob, now_ms) {
        KeychainOutcome::Usable(access_token) => {
            eprintln!("\x1b[2mUsing Claude Code session credentials.\x1b[0m");
            if !managed_file {
                mirror_keychain_into_zo_store(&blob);
            }
            let expires_at_ms = blob
                .get("claudeAiOauth")
                .and_then(|oauth| oauth.get("expiresAt"))
                .and_then(Value::as_u64);
            Ok(KeychainSession {
                access_token,
                expires_at_ms,
                plan: blob.get("claudeAiOauth").and_then(blob_plan),
            })
        }
        KeychainOutcome::Expired => {
            let refreshed = refresh_expired_keychain_blob(&blob);
            match &refreshed {
                Ok(_) => {
                    eprintln!("\x1b[2mRefreshed Claude Code session credentials.\x1b[0m");
                }
                Err(_) => eprintln!(
                    "\x1b[33mClaude Code keychain token expired and could not be refreshed, falling back to Zo auth.\x1b[0m"
                ),
            }
            refreshed
        }
        KeychainOutcome::MissingInferenceScope => {
            eprintln!(
                "\x1b[33mClaude Code keychain token lacks user:inference scope, falling back to Zo auth.\x1b[0m"
            );
            Err(CredentialMiss::Unusable(MISSING_INFERENCE_SCOPE.to_string()))
        }
        KeychainOutcome::Absent => Err(CredentialMiss::Absent),
    }
}

/// Whether a Claude Code login is kept where this process would read one —
/// kept, not necessarily usable. Never reads a secret and never refreshes:
/// the managed folder's file or the CLI's scoped item for a managed launch,
/// the machine's item for a bare one. An answer the memo already holds is
/// reused; otherwise one `security` attribute lookup (no `-w`, so no access
/// prompt) settles it.
#[must_use]
pub fn claude_code_login_configured() -> bool {
    let keychain_allowed = std::env::var_os(DISABLE_KEYCHAIN_ENV).is_none();
    if let Some(dir) = crate::managed_account::claude_config_dir().filter(|dir| !dir.is_empty()) {
        return credentials_file_override().is_some()
            || (keychain_allowed && keychain_item_kept(&scoped_keychain_service(&dir)));
    }
    if !keychain_allowed {
        return false;
    }
    match cached_keychain_session() {
        KeychainCacheLookup::Fresh(Ok(_) | Err(CredentialMiss::Unusable(_))) => true,
        KeychainCacheLookup::Fresh(Err(CredentialMiss::Absent)) => false,
        KeychainCacheLookup::Stale => keychain_item_kept(KEYCHAIN_SERVICE),
    }
}

/// Whether the keychain keeps an item under `service`: its attributes only,
/// which no access list guards. A keychain that does not answer may well
/// keep one, and saying so costs one more question at the next connection;
/// a machine without `security` keeps none.
fn keychain_item_kept(service: &str) -> bool {
    match Command::new("security")
        .args(["find-generic-password", "-s", service])
        .output()
    {
        Ok(output) => output.status.code() != Some(KEYCHAIN_ITEM_NOT_FOUND),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Token-only convenience over [`read_claude_code_keychain_session`].
#[must_use]
pub fn read_claude_code_keychain_token() -> Option<String> {
    read_claude_code_keychain_session().map(|session| session.access_token)
}

/// Keep zo's own credential entry in step with the keychain whenever a usable
/// session is read.
///
/// Both stores hold the *same* rotating subscription grant, and only one copy of
/// it can be live: the token endpoint replaces a refresh token the moment it is
/// spent and forgets the predecessor. Zo's copy was written only when zo itself
/// refreshed, so every refresh Claude Code performed left the mirror one branch
/// behind — and a superseded branch is worse than no fallback at all, because it
/// still looks like a credential and can only ever answer `invalid_grant`.
///
/// Measured on a real machine: the keychain and the mirror carried different
/// refresh tokens, and startup credential resolution died on the mirror's dead
/// branch while a valid keychain session sat one rung above it.
///
/// The `oauth` entry is the keychain identity's mirror, by the same policy the
/// refresh path already applied when it overwrote that entry after every
/// keychain refresh; this only makes the mirror track the branch it is supposed
/// to be mirroring instead of drifting until the next refresh happened to run.
/// Nothing is written unless the mirror is actually stale, so a steady state
/// costs one small file read.
fn mirror_keychain_into_zo_store(blob: &Value) {
    let Some(oauth) = blob.get("claudeAiOauth") else {
        return;
    };
    let field = |name: &str| {
        oauth
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let Some(access_token) = field("accessToken") else {
        return;
    };
    let refresh_token = field("refreshToken");
    let mirrored = crate::oauth_store::load_oauth_credentials().ok().flatten();
    if mirrored.as_ref().is_some_and(|saved| {
        saved.access_token == access_token && saved.refresh_token == refresh_token
    }) {
        return;
    }
    // Unix seconds on the way out; the blob records milliseconds.
    let expires_at = oauth
        .get("expiresAt")
        .and_then(Value::as_u64)
        .map(|ms| ms / 1000);
    let scopes = oauth
        .get("scopes")
        .and_then(Value::as_array)
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let _ = crate::oauth_store::save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token,
        refresh_token,
        expires_at,
        scopes,
    });
}

/// Re-read the credential store after a failed refresh. `Some` only when the
/// store now carries a *different* refresh token than the one we just spent
/// and that copy is usable (not expired) — i.e. someone else rotated the grant
/// and wrote it back. Nothing is written here.
fn reread_store_if_rotated(spent_refresh_token: &str) -> Option<KeychainSession> {
    let blob = read_keychain_blob()?;
    let oauth = blob.get("claudeAiOauth")?;
    let current = oauth.get("refreshToken").and_then(Value::as_str)?;
    if current.is_empty() || current == spent_refresh_token {
        return None;
    }
    match evaluate_keychain_credentials(&blob, now_unix_millis()) {
        KeychainOutcome::Usable(access_token) => {
            mirror_keychain_into_zo_store(&blob);
            Some(KeychainSession {
                access_token,
                expires_at_ms: oauth.get("expiresAt").and_then(Value::as_u64),
                plan: blob_plan(oauth),
            })
        }
        _ => None,
    }
}

/// Does the store now carry `refresh_token`? The write-back's read-after-write.
fn store_holds_refresh_token(refresh_token: &str) -> bool {
    read_keychain_blob()
        .as_ref()
        .and_then(|blob| blob.get("claudeAiOauth"))
        .and_then(|oauth| oauth.get("refreshToken"))
        .and_then(Value::as_str)
        .is_some_and(|held| held == refresh_token)
}

/// Refresh an expired keychain blob via its `refreshToken`, persist the result
/// (keychain write-back + zo credential mirror), and return the fresh
/// session. Unusable — with the reason — when the blob has no refresh token, a
/// recent attempt already failed (cool-down), or the token endpoint rejects
/// the refresh.
fn refresh_expired_keychain_blob(blob: &Value) -> Result<KeychainSession, CredentialMiss> {
    let unusable = |why: &str| CredentialMiss::Unusable(why.to_string());
    let oauth = blob
        .get("claudeAiOauth")
        .ok_or_else(|| unusable(EXPIRED_NO_REFRESH_TOKEN))?;
    let refresh_token = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| unusable(EXPIRED_NO_REFRESH_TOKEN))?
        .to_string();

    // Keyed on the token, not on the clock: a branch the endpoint has already
    // rejected is retired outright (nothing but a new sign-in revives it), while
    // a transient failure only cools down. The old process-wide timestamp could
    // not tell those apart and blocked a healthy branch for a minute either way.
    match refresh_gate::refresh_blocked(&refresh_token) {
        Some(refresh_gate::RefreshBlock::Retired) => return Err(unusable(EXPIRED_REFRESH_REFUSED)),
        Some(refresh_gate::RefreshBlock::CoolingDown) => return Err(unusable(EXPIRED_REFRESH_COOLING)),
        None => {}
    }

    // Re-request the original grant's scopes; an empty/absent list falls back
    // to the standard subscription scopes (the server still bounds the result
    // by the original grant).
    let scopes: Vec<String> = oauth
        .get("scopes")
        .and_then(Value::as_array)
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let config = claude_code_oauth_config();
    let request = OAuthRefreshRequest::from_config(
        &config,
        refresh_token.clone(),
        (!scopes.is_empty()).then_some(scopes),
    );

    let refreshed = match refresh_token_set_on_own_thread(&config, &request) {
        Ok(refreshed) => {
            refresh_gate::record_success(&refresh_token);
            refreshed
        }
        Err(error) => {
            let retired = refresh_gate::record_failure(&refresh_token, &error);
            // Another process (Claude Code in a sibling pane, the IDE, an
            // earlier zo) may have rotated this grant a moment ago and written
            // the new branch back. Before declaring the grant dead, look once
            // more at the store: a newer refresh token there means we lost a
            // race, not the login.
            if let Some(fresh) = reread_store_if_rotated(&refresh_token) {
                eprintln!(
                    "\x1b[2mClaude Code credentials were refreshed by another process; using the newer copy.\x1b[0m"
                );
                return Ok(fresh);
            }
            eprintln!("\x1b[33mClaude Code OAuth refresh failed: {error}\x1b[0m");
            if retired {
                // The grant itself was rejected, so neither zo nor Claude Code
                // can recover without a sign-in. Say so — the bare 400 body sent
                // people looking for a network problem.
                eprintln!(
                    "\x1b[33m  This refresh token has been superseded or revoked. \
                     Sign in again (`claude` or `zo login claude`).\x1b[0m"
                );
                return Err(unusable(EXPIRED_REFRESH_REFUSED));
            }
            return Err(CredentialMiss::Unusable(format!(
                "the Claude Code sign-in expired and refreshing it failed ({}); the next connection tries again",
                super::single_line_reason(&error)
            )));
        }
    };

    // The endpoint may rotate the refresh token; keep the old one only when no
    // replacement arrives.
    let resolved_refresh_token = refreshed
        .refresh_token
        .clone()
        .unwrap_or_else(|| refresh_token.clone());
    let rotated = resolved_refresh_token != refresh_token;

    let updated_blob = updated_keychain_blob(blob, &refreshed, &resolved_refresh_token);
    // Read-after-write: a refused or half-applied write leaves Claude Code
    // holding the refresh token we just spent. Believe the store, not the
    // return code.
    let wrote_back = write_credentials_blob(&updated_blob) && store_holds_refresh_token(&resolved_refresh_token);
    if !wrote_back && rotated {
        // Claude Code still holds the now-invalidated refresh token; it will
        // ask the user to log in again next time it runs. Zo stays healthy
        // via the credential mirror below.
        eprintln!(
            "\x1b[33mwarning: refreshed Claude Code OAuth token could not be written back to the keychain; Claude Code may require a re-login.\x1b[0m"
        );
    }

    // Mirror into zo's own credential store so the fresh token set survives
    // a refused keychain write (and upgrades any stale scope-less `zo login`
    // token in passing — the mirror carries `user:inference`).
    let _ = crate::oauth_store::save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token: refreshed.access_token.clone(),
        refresh_token: Some(resolved_refresh_token),
        expires_at: refreshed.expires_at,
        scopes: refreshed.scopes.clone(),
    });

    Ok(KeychainSession {
        access_token: refreshed.access_token,
        expires_at_ms: refreshed.expires_at.map(|secs| secs.saturating_mul(1000)),
        // The refresh answers with tokens only; the plan is a property of the
        // grant, carried on the blob that is being refreshed.
        plan: blob_plan(oauth),
    })
}

/// Hard bounds on the refresh round-trip. Credential resolution can run on a
/// startup/turn-boundary path; a blackholed network (offline, sandboxed test
/// runner) must bound the wait instead of hanging that path forever — the
/// shared HTTP pool deliberately carries no overall timeout for streaming, so
/// the refresh uses its own client.
const REFRESH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the token-endpoint refresh on a dedicated OS thread with its own
/// single-threaded runtime. The read path is synchronous but gets called from
/// every flavor of context — plain startup code, `spawn_blocking` workers, and
/// agent threads already inside `Handle::block_on` — and a nested
/// `Runtime::new().block_on` panics in the last case. A scoped thread has no
/// ambient tokio context, so this is safe everywhere; the cost (one short-lived
/// thread per ~8-hourly refresh) is negligible.
pub(super) fn refresh_token_set_on_own_thread(
    config: &OAuthConfig,
    request: &OAuthRefreshRequest,
) -> Result<OAuthTokenSet, ApiError> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(ApiError::from)?;
                let http = reqwest::Client::builder()
                    .connect_timeout(REFRESH_CONNECT_TIMEOUT)
                    .timeout(REFRESH_TOTAL_TIMEOUT)
                    .build()
                    .map_err(ApiError::from)?;
                let client = AnthropicClient::from_auth(AuthSource::None)
                    .with_base_url(read_base_url())
                    .with_http_client(http);
                runtime.block_on(client.refresh_oauth_token(config, request))
            })
            .join()
            .map_err(|_| ApiError::Auth("keychain OAuth refresh thread panicked".to_string()))?
    })
}

/// Pure blob update: replace the OAuth fields the refresh changed, preserve
/// everything else (`subscriptionType`, unknown future fields) so the write-back
/// never strips data Claude Code relies on. `expiresAt` is converted from the
/// token set's Unix seconds to the blob's Unix milliseconds.
fn updated_keychain_blob(blob: &Value, refreshed: &OAuthTokenSet, refresh_token: &str) -> Value {
    let mut updated = blob.clone();
    if let Some(oauth) = updated
        .get_mut("claudeAiOauth")
        .and_then(Value::as_object_mut)
    {
        oauth.insert(
            "accessToken".to_string(),
            Value::String(refreshed.access_token.clone()),
        );
        oauth.insert(
            "refreshToken".to_string(),
            Value::String(refresh_token.to_string()),
        );
        if let Some(expires_at) = refreshed.expires_at {
            oauth.insert(
                "expiresAt".to_string(),
                Value::from(expires_at.saturating_mul(1000)),
            );
        }
        if !refreshed.scopes.is_empty() {
            oauth.insert("scopes".to_string(), Value::from(refreshed.scopes.clone()));
        }
    }
    updated
}

fn read_keychain_blob() -> Option<Value> {
    match read_keychain_blob_answer() {
        BlobRead::Found(blob) => Some(blob),
        BlobRead::Absent | BlobRead::Unusable(_) => None,
    }
}

/// What the store that holds the Claude Code login said.
enum BlobRead {
    Found(Value),
    /// No login is kept there.
    Absent,
    /// A login is kept there and could not be read — why.
    Unusable(&'static str),
}

fn read_keychain_blob_answer() -> BlobRead {
    // A managed account directory keeps its login in two places — the
    // `.credentials.json` beside it and the CLI's scoped keychain item — and
    // the fresher one is the login. Never the UNSCOPED item: that copy belongs
    // to another lineage of the same grant (the bare-terminal `claude`), and
    // refreshing it from inside an IDE pane is exactly the rotation race that
    // logs the other side out (measured 2026-08-27, zo-ide.log pid 78000). A
    // directory with neither is "not signed in".
    if let Some(dir) = crate::managed_account::claude_config_dir().filter(|dir| !dir.is_empty()) {
        let file_path = credentials_file_override();
        let file = file_path.as_deref().and_then(read_credentials_file);
        let scoped = (std::env::var_os(DISABLE_KEYCHAIN_ENV).is_none())
            .then(|| read_keychain_service_secret(&scoped_keychain_service(&dir)));
        let unanswered = matches!(scoped, Some(KeychainAnswer::Unanswered));
        let scoped_raw = scoped.and_then(KeychainAnswer::found);
        let scoped_blob = scoped_raw.as_deref().and_then(parse_keychain_blob);
        return match freshest_blob(file, scoped_blob) {
            Some(blob) => BlobRead::Found(blob),
            // Something is kept for this account and none of it reads as a
            // login: a file that is there, an item that is not a document, or
            // a keychain that did not answer.
            None if file_path.is_some() || scoped_raw.is_some() => BlobRead::Unusable(LOGIN_NOT_A_DOCUMENT),
            None if unanswered => BlobRead::Unusable(KEYCHAIN_UNANSWERED),
            None => BlobRead::Absent,
        };
    }
    if !keychain_read_allowed(None, false) {
        return BlobRead::Absent;
    }
    match read_keychain_service_secret(KEYCHAIN_SERVICE) {
        KeychainAnswer::Found(raw) => {
            parse_keychain_blob(&raw).map_or(BlobRead::Unusable(LOGIN_NOT_A_DOCUMENT), BlobRead::Found)
        }
        KeychainAnswer::Absent => BlobRead::Absent,
        KeychainAnswer::Unanswered => BlobRead::Unusable(KEYCHAIN_UNANSWERED),
    }
}

/// One keychain item's secret as `security find-generic-password -w` prints
/// it, trimmed; absent when the item is missing, refused or empty.
/// `security`'s exit status for an item this machine does not keep
/// (`errSecItemNotFound`). Every other failing status is the tool refusing or
/// failing, not the machine answering.
const KEYCHAIN_ITEM_NOT_FOUND: i32 = 44;

/// What the keychain said about one service.
///
/// The distinction is the whole point: [`Self::Absent`] is an answer — this
/// machine keeps no such item, and asking again this second will not change
/// that — while [`Self::Unanswered`] is the tool never running, an interaction
/// this session may not have, or a lock another process holds. A caller that
/// remembers the second as the first goes without a key it does have.
pub(crate) enum KeychainAnswer {
    Found(String),
    Absent,
    Unanswered,
}

impl KeychainAnswer {
    /// The secret, when there was one — for callers that do not retry.
    pub(crate) fn found(self) -> Option<String> {
        match self {
            Self::Found(secret) => Some(secret),
            Self::Absent | Self::Unanswered => None,
        }
    }
}

fn read_keychain_service_secret(service: &str) -> KeychainAnswer {
    let output = match Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
    {
        Ok(output) => output,
        // No `security` at all is a machine that keeps no keychain (Linux,
        // Windows): settled, not a read that failed.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return KeychainAnswer::Absent,
        Err(_) => return KeychainAnswer::Unanswered,
    };
    if !output.status.success() {
        return if output.status.code() == Some(KEYCHAIN_ITEM_NOT_FOUND) {
            KeychainAnswer::Absent
        } else {
            KeychainAnswer::Unanswered
        };
    }
    // An item that holds nothing readable is an item all the same: reading it
    // again answers the same, so it is absent rather than unanswered.
    let Ok(raw) = String::from_utf8(output.stdout) else {
        return KeychainAnswer::Absent;
    };
    let secret = raw.trim();
    if secret.is_empty() {
        KeychainAnswer::Absent
    } else {
        KeychainAnswer::Found(secret.to_string())
    }
}

/// A router key the window's router pane keeps under `service` — read only
/// on macOS, the one platform where the window keeps router keys, and never
/// under the keychain kill switch (`ZO_DISABLE_KEYCHAIN`).
pub(crate) fn read_router_key(service: &str) -> KeychainAnswer {
    // Neither is a read that failed: this platform keeps no router keys, and
    // the kill switch means this process may not look. Both are settled, so a
    // caller is right to remember them and stop asking.
    if !cfg!(target_os = "macos") || std::env::var_os(DISABLE_KEYCHAIN_ENV).is_some() {
        return KeychainAnswer::Absent;
    }
    read_keychain_service_secret(service)
}

/// Claude Code's config-directory override, and the credentials file it keeps
/// there. `Some` only when the variable names a directory that actually holds
/// `.credentials.json`. A named directory with no login deliberately does not
/// fall through to the machine keychain: those are different account lineages.
const CLAUDE_CREDENTIALS_FILE: &str = ".credentials.json";

fn managed_config_dir_selected() -> bool {
    crate::managed_account::claude_config_dir().is_some()
}

/// 어느 폴더를 볼 것인가 — [`crate::managed_account`] 가 답한다. 판이 도는
/// 동안 IDE 가 계정을 바꾸면 `auth.reload` 가 그 자리에 새 경로를 앉히고, 이
/// 자리는 굳어 버린 자기 env 대신 그것을 본다.
fn credentials_file_override() -> Option<std::path::PathBuf> {
    let dir = crate::managed_account::claude_config_dir()?;
    if dir.is_empty() {
        return None;
    }
    let path = std::path::Path::new(&dir).join(CLAUDE_CREDENTIALS_FILE);
    path.is_file().then_some(path)
}

/// Current `(mtime, len)` identity of the IDE-managed credentials file.
#[must_use]
pub fn managed_claude_credentials_stamp() -> Option<ManagedCredentialsStamp> {
    let path = credentials_file_override()?;
    let metadata = std::fs::metadata(&path).ok()?;
    Some(ManagedCredentialsStamp {
        path,
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
}


/// Whether the keychain may be consulted at all.
///
/// `config_dir` is `CLAUDE_CONFIG_DIR` as launched; `file_present` says whether
/// that directory already answered with a credentials file (in which case this
/// question never arises). The keychain is only for launches that name no
/// config directory — the bare terminal.
fn keychain_read_allowed(config_dir: Option<&std::ffi::OsStr>, file_present: bool) -> bool {
    if file_present {
        return false;
    }
    match config_dir {
        None => true,
        Some(dir) => dir.is_empty(),
    }
}

fn read_credentials_file(path: &std::path::Path) -> Option<Value> {
    // This file contains refresh credentials. Tighten a current-user-owned
    // legacy file, then read through the same owner-only/no-follow gate used by
    // Zo's own credential store. A symlink, junction, hard link, foreign owner,
    // or unverifiable Windows DACL is not trusted.
    core_types::paths::restrict_permissions_owner_only(path).ok()?;
    let raw = core_types::paths::read_private_file(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Atomic, owner-only rewrite of the credentials file — the on-disk twin of
/// `write_keychain_blob`. Claude Code reads this file back, so it must never
/// observe a half-written one.
fn write_credentials_file(path: &std::path::Path, blob: &Value) -> bool {
    let Ok(payload) = serde_json::to_vec(blob) else {
        return false;
    };
    core_types::paths::write_private_file(
        path,
        &payload,
        &core_types::paths::ParentDirPolicy::LeaveParent,
    )
    .is_ok()
}

/// Write a refreshed blob back to wherever it came from: the managed account's
/// file when `CLAUDE_CONFIG_DIR` names one, else the keychain item.
fn write_credentials_blob(blob: &Value) -> bool {
    if let Some(dir) = crate::managed_account::claude_config_dir().filter(|dir| !dir.is_empty()) {
        // Both copies, so neither the CLI's next read (keychain first) nor
        // zo's (fresher of the two) revives the grant this refresh just spent.
        // The keychain write is skipped where the keychain is disabled
        // (hermetic runs); the file alone is then the whole story.
        let path = std::path::Path::new(&dir).join(CLAUDE_CREDENTIALS_FILE);
        let file_written = write_credentials_file(&path, blob);
        let keychain_written = std::env::var_os(DISABLE_KEYCHAIN_ENV).is_some()
            || keychain_user().is_some_and(|account| {
                write_keychain_service_blob(&account, &scoped_keychain_service(&dir), blob)
            });
        return file_written && keychain_written;
    }
    keychain_account().is_some_and(|account| write_keychain_blob(&account, blob))
}

/// Who a scoped item is filed under: `$USER`, the way the CLI and the window
/// file it (`keychain.ts:80-82`).
fn keychain_user() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|user| !user.is_empty())
}

/// The keychain account the credential item is stored under, needed to address
/// the write-back. Parsed from the item's attribute listing (`-w` prints only
/// the secret).
fn keychain_account() -> Option<String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_keychain_account(&String::from_utf8_lossy(&output.stdout))
}

/// Extract the account from `security find-generic-password` attribute output:
/// a line of the form `    "acct"<blob>="joe"`. Hex-encoded (non-UTF-8)
/// accounts are not handled — the caller skips the write-back rather than
/// guessing.
fn parse_keychain_account(attributes: &str) -> Option<String> {
    let line = attributes
        .lines()
        .find(|line| line.trim_start().starts_with("\"acct\""))?;
    let (_, value) = line.split_once("=\"")?;
    let account = value.strip_suffix('"')?;
    (!account.is_empty()).then(|| account.to_string())
}

/// Best-effort keychain write-back (`-U` updates the existing item in place).
/// The secret travels via argv, which is briefly visible to same-user processes
/// — the same trust boundary as the existing `-w` read (any same-user process
/// could read the item directly), so this adds no new exposure. Uses the same
/// `security` binary the read path uses, so an item ACL that admits the read
/// admits the write without a new GUI prompt.
fn write_keychain_blob(account: &str, blob: &Value) -> bool {
    write_keychain_service_blob(account, KEYCHAIN_SERVICE, blob)
}

/// `-U` updates an existing item in place (its access list included) and
/// creates one otherwise. The secret is the argument, as the CLI passes it:
/// the tool's password prompt keeps only the first 128 bytes of a value typed
/// at it (measured 2026-09-10), which is how the window's seeds broke.
fn write_keychain_service_blob(account: &str, service: &str, blob: &Value) -> bool {
    Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            account,
            "-s",
            service,
            "-w",
            &blob.to_string(),
        ])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod keychain_gate_tests {
    use super::keychain_read_allowed;
    use std::ffi::OsStr;

    #[test]
    fn keychain_is_for_bare_launches_only() {
        assert!(keychain_read_allowed(None, false));
        assert!(keychain_read_allowed(Some(OsStr::new("")), false));
        // A named config dir with no login in it is "signed out", not "use the keychain".
        assert!(!keychain_read_allowed(Some(OsStr::new("/tmp/acct")), false));
        // And once the file answered, the question never reaches the keychain.
        assert!(!keychain_read_allowed(None, true));
    }
}

#[cfg(test)]
mod scoped_keychain_tests {
    use super::{freshest_blob, oauth_expires_at, parse_keychain_blob, scoped_keychain_service};
    use serde_json::json;
    use std::ffi::OsStr;

    /// The CLI's rule and the window's rule, in one place: unscoped name, a
    /// dash, the first eight hex digits of the directory path's SHA-256.
    #[test]
    fn a_managed_directory_is_filed_under_its_hashed_name() {
        let service = scoped_keychain_service(OsStr::new("/Users/dev/Library/Application Support/dev.zerocode.app/.claude"));
        // The digest is derived from the path above, so it moves with it.
        assert_eq!(service, "Claude Code-credentials-f2bb2309");
        assert_eq!(service.len(), "Claude Code-credentials-".len() + 8);
        assert_ne!(service, scoped_keychain_service(OsStr::new("/tmp/other")));
    }

    /// Two copies of one login: the refresh that happened later wins, and the
    /// keychain — the store the CLI writes first — breaks a tie or a missing stamp.
    #[test]
    fn the_fresher_copy_wins_and_the_keychain_breaks_ties() {
        let file = json!({"claudeAiOauth": {"accessToken": "f", "refreshToken": "rf", "expiresAt": 1_789_026_889_032u64}});
        let scoped = json!({"claudeAiOauth": {"accessToken": "k", "refreshToken": "rk", "expiresAt": 1_789_059_636_237u64}});
        assert_eq!(oauth_expires_at(&scoped), Some(1_789_059_636_237));
        assert_eq!(freshest_blob(Some(file.clone()), Some(scoped.clone())), Some(scoped.clone()));
        let newer_file = json!({"claudeAiOauth": {"accessToken": "f2", "refreshToken": "rf2", "expiresAt": 1_789_070_000_000u64}});
        assert_eq!(freshest_blob(Some(newer_file.clone()), Some(scoped.clone())), Some(newer_file));
        assert_eq!(freshest_blob(Some(scoped.clone()), Some(scoped.clone())), Some(scoped.clone()));
        let unstamped = json!({"claudeAiOauth": {"accessToken": "u", "refreshToken": "ru"}});
        assert_eq!(freshest_blob(Some(unstamped.clone()), Some(scoped.clone())), Some(scoped.clone()));
        assert_eq!(freshest_blob(Some(file.clone()), None), Some(file));
        assert_eq!(freshest_blob(None, Some(scoped.clone())), Some(scoped));
        assert_eq!(freshest_blob(None, None), None);
    }

    /// The item this Mac held for the window's home on 2026-09-10: 128 bytes,
    /// cut inside the access token. Not a login, not an error — absent.
    #[test]
    fn a_cut_off_item_reads_as_absent() {
        let cut = format!(r#"{{"claudeAiOauth":{{"accessToken":"{}"#, "A".repeat(95));
        assert_eq!(cut.len(), 128);
        assert_eq!(parse_keychain_blob(&cut), None);
        assert!(parse_keychain_blob(r#" {"claudeAiOauth":{"accessToken":"a"}} "#).is_some());
    }
}

#[cfg(test)]
mod credentials_file_tests {
    use super::{read_credentials_file, write_credentials_file};
    use serde_json::json;

    #[test]
    fn credentials_file_round_trips_the_claude_code_blob() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        let blob = json!({"claudeAiOauth": {"accessToken": "a", "refreshToken": "r", "expiresAt": 1}});
        assert!(write_credentials_file(&path, &blob));
        assert_eq!(read_credentials_file(&path), Some(blob));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credentials must stay owner-only");
        }
    }

    #[test]
    fn unreadable_or_invalid_file_is_absent_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        assert_eq!(read_credentials_file(&path), None);
        std::fs::write(&path, b"not json").expect("write");
        assert_eq!(read_credentials_file(&path), None);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KEYCHAIN_EXPIRY_BUFFER_MS, KeychainOutcome, REFRESH_CONNECT_TIMEOUT, REFRESH_TOTAL_TIMEOUT,
        claude_code_oauth_config, evaluate_keychain_credentials, mirror_keychain_into_zo_store,
        parse_keychain_account, updated_keychain_blob,
    };
    use crate::providers::anthropic::OAuthTokenSet;
    use std::time::Duration;

    /// A `now` far enough from the fixture expiries that the proactive buffer
    /// does not flip outcomes unintentionally.
    const NOW_MS: u64 = 1_000_000_000;
    const FUTURE_MS: u64 = NOW_MS + KEYCHAIN_EXPIRY_BUFFER_MS + 1;

    #[test]
    fn oauth_refresh_timeout_is_bounded_control_plane_timeout() {
        // Generation streams deliberately have no total request timeout, but
        // OAuth refresh is a short control-plane POST and must stay bounded so
        // startup/turn-boundary credential resolution cannot hang indefinitely.
        assert_eq!(REFRESH_CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(REFRESH_TOTAL_TIMEOUT, Duration::from_secs(30));
        assert!(REFRESH_CONNECT_TIMEOUT < REFRESH_TOTAL_TIMEOUT);
    }

    #[test]
    fn keychain_usable_with_unexpired_inference_scope() {
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-abc",
                "expiresAt": FUTURE_MS,
                "scopes": ["user:profile", "user:inference"],
            }
        });
        assert_eq!(
            evaluate_keychain_credentials(&creds, NOW_MS),
            KeychainOutcome::Usable("sk-ant-oat01-abc".to_string())
        );
    }

    #[test]
    fn keychain_expired_past_expiry() {
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-abc",
                "expiresAt": NOW_MS - 1,
                "scopes": ["user:inference"],
            }
        });
        assert_eq!(
            evaluate_keychain_credentials(&creds, NOW_MS),
            KeychainOutcome::Expired
        );
    }

    #[test]
    fn keychain_expiring_within_buffer_counts_as_expired() {
        // Proactive refresh: a token lapsing in under the buffer must refresh
        // now instead of racing the boundary and 401ing mid-turn.
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-abc",
                "expiresAt": NOW_MS + KEYCHAIN_EXPIRY_BUFFER_MS - 1,
                "scopes": ["user:inference"],
            }
        });
        assert_eq!(
            evaluate_keychain_credentials(&creds, NOW_MS),
            KeychainOutcome::Expired
        );
    }

    #[test]
    fn keychain_rejected_without_inference_scope() {
        // The exact failure behind the 403: a token whose scopes omit
        // `user:inference` must not be handed to the inference path.
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-abc",
                "expiresAt": FUTURE_MS,
                "scopes": ["user:profile", "org:create_api_key"],
            }
        });
        assert_eq!(
            evaluate_keychain_credentials(&creds, NOW_MS),
            KeychainOutcome::MissingInferenceScope
        );
    }

    #[test]
    fn keychain_absent_without_token() {
        assert_eq!(
            evaluate_keychain_credentials(&serde_json::json!({}), NOW_MS),
            KeychainOutcome::Absent
        );
        let empty = serde_json::json!({ "claudeAiOauth": { "accessToken": "" } });
        assert_eq!(
            evaluate_keychain_credentials(&empty, NOW_MS),
            KeychainOutcome::Absent
        );
    }

    #[test]
    fn keychain_permissive_when_scopes_field_absent() {
        // Older keychain blobs predate the scopes field; don't lock those out.
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-abc",
                "expiresAt": FUTURE_MS,
            }
        });
        assert_eq!(
            evaluate_keychain_credentials(&creds, NOW_MS),
            KeychainOutcome::Usable("sk-ant-oat01-abc".to_string())
        );
    }

    #[test]
    fn keychain_without_expiry_field_is_not_expired() {
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-abc",
                "scopes": ["user:inference"],
            }
        });
        assert_eq!(
            evaluate_keychain_credentials(&creds, u64::MAX),
            KeychainOutcome::Usable("sk-ant-oat01-abc".to_string())
        );
    }

    #[test]
    fn updated_blob_replaces_oauth_fields_and_preserves_siblings() {
        let blob = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "old-access",
                "refreshToken": "old-refresh",
                "expiresAt": 1_111_u64,
                "scopes": ["user:inference"],
                "subscriptionType": "max",
            },
            "otherTopLevel": true,
        });
        let refreshed = OAuthTokenSet {
            access_token: "new-access".to_string(),
            refresh_token: Some("new-refresh".to_string()),
            expires_at: Some(2_000),
            scopes: vec!["user:inference".to_string(), "user:profile".to_string()],
        };
        let updated = updated_keychain_blob(&blob, &refreshed, "new-refresh");
        let oauth = updated.get("claudeAiOauth").expect("oauth object");
        assert_eq!(oauth["accessToken"], "new-access");
        assert_eq!(oauth["refreshToken"], "new-refresh");
        // Unix seconds from the token endpoint → Unix milliseconds in the blob.
        assert_eq!(oauth["expiresAt"], 2_000_000_u64);
        assert_eq!(
            oauth["scopes"],
            serde_json::json!(["user:inference", "user:profile"])
        );
        // Fields the refresh does not own survive untouched.
        assert_eq!(oauth["subscriptionType"], "max");
        assert_eq!(updated["otherTopLevel"], true);
    }

    #[test]
    fn updated_blob_keeps_old_expiry_and_scopes_when_response_omits_them() {
        let blob = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "old-access",
                "refreshToken": "old-refresh",
                "expiresAt": 1_111_u64,
                "scopes": ["user:inference"],
            }
        });
        let refreshed = OAuthTokenSet {
            access_token: "new-access".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: Vec::new(),
        };
        // No rotation: caller passes the old refresh token through.
        let updated = updated_keychain_blob(&blob, &refreshed, "old-refresh");
        let oauth = updated.get("claudeAiOauth").expect("oauth object");
        assert_eq!(oauth["accessToken"], "new-access");
        assert_eq!(oauth["refreshToken"], "old-refresh");
        assert_eq!(oauth["expiresAt"], 1_111_u64);
        assert_eq!(oauth["scopes"], serde_json::json!(["user:inference"]));
    }

    #[test]
    fn keychain_cache_freshness_rules() {
        use super::{
            CachedSessionShape, KEYCHAIN_NEGATIVE_CACHE_TTL, KEYCHAIN_NO_EXPIRY_RECHECK,
            keychain_cache_entry_fresh,
        };
        // Cached miss: trusted only inside the negative TTL, so a machine
        // without credentials stops forking `security` per turn but still
        // recovers within a minute of a login.
        assert!(keychain_cache_entry_fresh(
            &CachedSessionShape::Miss,
            Duration::ZERO,
            NOW_MS
        ));
        assert!(!keychain_cache_entry_fresh(
            &CachedSessionShape::Miss,
            KEYCHAIN_NEGATIVE_CACHE_TTL,
            NOW_MS
        ));
        // Session with a recorded expiry: served regardless of age until the
        // proactive buffer window — refresh timing is unchanged by the cache.
        assert!(keychain_cache_entry_fresh(
            &CachedSessionShape::ExpiringAt(FUTURE_MS),
            Duration::from_secs(86_400),
            NOW_MS
        ));
        assert!(!keychain_cache_entry_fresh(
            &CachedSessionShape::ExpiringAt(NOW_MS + KEYCHAIN_EXPIRY_BUFFER_MS - 1),
            Duration::ZERO,
            NOW_MS
        ));
        // Session without an expiry: re-read on the fixed cadence.
        assert!(keychain_cache_entry_fresh(
            &CachedSessionShape::NoExpiry,
            Duration::ZERO,
            NOW_MS
        ));
        assert!(!keychain_cache_entry_fresh(
            &CachedSessionShape::NoExpiry,
            KEYCHAIN_NO_EXPIRY_RECHECK,
            NOW_MS
        ));
    }

    #[test]
    fn keychain_cache_store_serve_invalidate_roundtrip() {
        let session = Ok(super::KeychainSession {
            access_token: "sk-ant-oat01-cache".to_string(),
            expires_at_ms: Some(u64::MAX),
            plan: Some("max".to_string()),
        });
        super::store_keychain_session(&session);
        assert_eq!(
            super::cached_keychain_session(),
            super::KeychainCacheLookup::Fresh(session)
        );
        // 401 recovery invalidates the memo: the next lookup must go through
        // to the keychain instead of re-serving the stale bearer.
        super::invalidate_claude_code_keychain_cache();
        assert_eq!(
            super::cached_keychain_session(),
            super::KeychainCacheLookup::Stale
        );
        // A miss keeps its reason while it is served from the memo: a cached
        // "expired and refused" must not come back as "nothing here".
        let refused = Err(crate::credential::CredentialMiss::Unusable(
            super::EXPIRED_REFRESH_REFUSED.to_string(),
        ));
        super::store_keychain_session(&refused);
        assert_eq!(
            super::cached_keychain_session(),
            super::KeychainCacheLookup::Fresh(refused)
        );
        super::invalidate_claude_code_keychain_cache();
    }

    #[test]
    fn parses_account_from_security_attribute_listing() {
        let attributes = concat!(
            "keychain: \"/Users/dev/Library/Keychains/login.keychain-db\"\n",
            "version: 512\n",
            "class: \"genp\"\n",
            "attributes:\n",
            "    0x00000007 <blob>=\"Claude Code-credentials\"\n",
            "    \"acct\"<blob>=\"joe\"\n",
            "    \"svce\"<blob>=\"Claude Code-credentials\"\n",
        );
        assert_eq!(parse_keychain_account(attributes), Some("joe".to_string()));
    }

    #[test]
    fn account_parse_rejects_missing_or_unquoted_forms() {
        assert_eq!(parse_keychain_account(""), None);
        // Hex-encoded (non-UTF-8) account: skip the write-back, don't guess.
        assert_eq!(
            parse_keychain_account("    \"acct\"<blob>=0x6A6F65\n"),
            None
        );
        assert_eq!(parse_keychain_account("    \"acct\"<blob>=\"\"\n"), None);
    }

    /// The mirror and the keychain hold the same rotating grant, so a mirror
    /// that is only written when *zo* refreshes falls behind every refresh
    /// Claude Code performs — and a superseded refresh token still looks like a
    /// credential while being able to answer nothing but `invalid_grant`. Reading
    /// a usable session has to bring the mirror along.
    #[test]
    fn reading_a_usable_session_brings_the_zo_mirror_along() {
        let _guard = crate::test_env_lock();
        let config_home = std::env::temp_dir().join(format!(
            "api-keychain-mirror-{}-{}",
            std::process::id(),
            super::now_unix_millis()
        ));
        std::fs::create_dir_all(&config_home).expect("temp config home");
        std::env::set_var("ZO_CONFIG_HOME", &config_home);

        // Stale mirror: the branch zo last saw, two rotations ago.
        crate::oauth_store::save_oauth_credentials(&core_types::OAuthTokenSet {
            access_token: "stale-access".to_string(),
            refresh_token: Some("superseded-branch".to_string()),
            expires_at: Some(1),
            scopes: vec!["user:inference".to_string()],
        })
        .expect("seed the stale mirror");

        let blob = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "live-access",
                "refreshToken": "live-branch",
                "expiresAt": 2_000_000_000_000_u64,
                "scopes": ["user:inference", "user:profile"],
                "subscriptionType": "team",
            }
        });
        mirror_keychain_into_zo_store(&blob);

        let mirrored = crate::oauth_store::load_oauth_credentials()
            .expect("read the mirror")
            .expect("the mirror exists");
        assert_eq!(mirrored.access_token, "live-access");
        assert_eq!(
            mirrored.refresh_token.as_deref(),
            Some("live-branch"),
            "the fallback rung must hold the live branch, not a spent one"
        );
        assert_eq!(
            mirrored.expires_at,
            Some(2_000_000_000),
            "the blob records milliseconds; the store records seconds"
        );
        assert!(mirrored.scopes.iter().any(|scope| scope == "user:inference"));

        // Already in step: nothing to do, and nothing written.
        let before = std::fs::metadata(config_home.join("credentials.json"))
            .and_then(|meta| meta.modified())
            .expect("mirror mtime");
        mirror_keychain_into_zo_store(&blob);
        let after = std::fs::metadata(config_home.join("credentials.json"))
            .and_then(|meta| meta.modified())
            .expect("mirror mtime");
        assert_eq!(
            before, after,
            "a steady state must not rewrite the credential file on every read"
        );

        crate::oauth_store::clear_oauth_credentials().expect("clear the mirror");
        std::env::remove_var("ZO_CONFIG_HOME");
        std::fs::remove_dir_all(&config_home).ok();
    }

    #[test]
    fn subscription_oauth_config_targets_the_claude_ai_flow() {
        // Regression guard for the 403 class of bugs: the config must stay on
        // the subscription flow (claude.ai authorize, console token endpoint)
        // and keep requesting `user:inference`.
        let config = claude_code_oauth_config();
        assert_eq!(config.client_id, "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
        assert!(config.authorize_url.starts_with("https://claude.ai/"));
        assert!(
            config
                .token_url
                .starts_with("https://console.anthropic.com/")
        );
        assert!(config.scopes.iter().any(|scope| scope == "user:inference"));
    }
}
