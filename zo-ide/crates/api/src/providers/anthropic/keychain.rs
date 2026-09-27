//! Claude Code session credentials — read where the CLI keeps them, renewed
//! by the CLI itself.
//!
//! Claude Code keeps its OAuth bundle — `{"claudeAiOauth": {accessToken,
//! refreshToken, expiresAt (Unix ms), scopes, …}}` — in a macOS keychain item,
//! and beside its settings in `.credentials.json` when the keychain will not
//! take it. Which item and which folder is the CLI's own rule
//! ([`ClaudeCodeStore`]).
//!
//! ## Zo reads that login and never renews it (t-11045)
//!
//! A refresh token can be spent once: the endpoint hands back a replacement
//! and forgets the old one. Zo used to renew an expired Claude Code login
//! itself and write the new pair back where it had read it — and a window pane
//! read the wrong place: the shared runtime home (`CLAUDE_CONFIG_DIR`), a COPY
//! the window writes of the chosen account at a switch, while the CLI in every
//! pane keeps and renews the account's own folder
//! (`CLAUDE_SECURESTORAGE_CONFIG_DIR`). When a pane's CLI renewed first, zo
//! spent a superseded refresh token and lost (`invalid_grant`, then "Claude
//! auth unavailable" — 89 times in one machine's log); when zo renewed first,
//! the new branch went to the copy and the CLI's own store kept the dead one.
//!
//! So zo reads the store the CLI keeps, and when that login has expired it
//! asks the store's own CLI to renew it ([`install_claude_login_renewer`]: a
//! headless run answering a slash command, no model asked) and reads again.
//! The CLI takes its own refresh lock and reads its store again under it, so
//! zo's ask is safe beside every pane of the same account. Zo writes nothing
//! to a Claude Code store and copies nothing out of one.
//!
//! Living in the `api` crate (not the CLI) so the sub-agent provider path uses
//! the *same* resolution chain as the interactive client.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use core_types::{OAuthConfig, OAuthRefreshRequest};
use serde_json::Value;

use super::{AnthropicClient, AuthSource, OAuthTokenSet, read_base_url};
use crate::credential::CredentialMiss;
use crate::error::ApiError;
use crate::providers::refresh_gate;

/// Why a Claude Code login that is there could not be used, in the words a
/// model list shows (`zo models`) — each names the way back in.
const EXPIRED_RENEWAL_REFUSED: &str = concat!(
    "the Claude Code sign-in expired and its own CLI could not renew it — ",
    claude_sign_in_again!()
);
const EXPIRED_RENEWAL_COOLING: &str =
    "the Claude Code sign-in expired and its CLI could not be asked a moment ago; the next connection asks again";
const EXPIRED_NO_RENEWER: &str = "the Claude Code sign-in expired — running `claude` renews it";
const EXPIRED_NO_REFRESH_TOKEN: &str = concat!(
    "the Claude Code sign-in expired and holds no refresh token — ",
    claude_sign_in_again!()
);
const MISSING_INFERENCE_SCOPE: &str = concat!(
    "the Claude Code sign-in lacks the user:inference scope — ",
    claude_sign_in_again!()
);
const KEYCHAIN_UNANSWERED: &str =
    "the macOS keychain did not answer for the Claude Code sign-in (locked, or access refused)";
const LOGIN_NOT_A_DOCUMENT: &str = concat!(
    "the Claude Code sign-in on this machine is not a readable login — ",
    claude_sign_in_again!()
);

/// Keychain service name Claude Code stores its OAuth bundle under.
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Hex digits of the folder's SHA-256 that the CLI appends to the service
/// name for a scoped login (`Claude Code-credentials-<8>`, Claude Code
/// 2.1.261+; the window seeds the same name).
const SCOPED_SERVICE_HASH_LEN: usize = 8;

/// The keychain service a scoped login is filed under: the unscoped name plus
/// the first eight hex digits of the folder path's SHA-256 — the rule the CLI
/// reads by and the window seeds by, so all three look in one place.
fn scoped_keychain_service(folder: &std::ffi::OsStr) -> String {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(folder.to_string_lossy().as_bytes()));
    format!("{KEYCHAIN_SERVICE}-{}", &digest[..SCOPED_SERVICE_HASH_LEN])
}

/// Where the Claude Code CLI keeps the login this process speaks as — the
/// CLI's own rule (Claude Code 2.1.283, `JL()` and `Bw()`), so zo reads the
/// store the CLI renews: the credential folder when
/// `CLAUDE_SECURESTORAGE_CONFIG_DIR` names one, else the config folder
/// `CLAUDE_CONFIG_DIR`, else the machine's own login. A credential folder set
/// but empty is the machine's own login too.
///
/// A window pane is launched with both folders — the shared runtime home as
/// its config, the chosen account's folder as its credentials — and reading
/// the first is how zo came to read the window's copy instead of the login
/// (t-11045).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeCodeStore {
    /// The folder the login is scoped to; `None` is the machine's own login.
    folder: Option<PathBuf>,
    /// What chose it, in the words a diagnosis shows: the variable that named
    /// the folder, or the keychain for the machine's own login.
    named_by: &'static str,
}

/// [`ClaudeCodeStore::named_by`] for the machine's own login.
const MACHINE_KEYCHAIN: &str = "keychain";

impl ClaudeCodeStore {
    fn current() -> Self {
        Self::from_folders(
            crate::managed_account::claude_secure_storage_dir(),
            crate::managed_account::claude_config_dir(),
        )
    }

    /// The rule with its two inputs handed in.
    fn from_folders(credentials: Option<OsString>, config: Option<OsString>) -> Self {
        use crate::managed_account::{CLAUDE_CONFIG_DIR_ENV, CLAUDE_SECURE_STORAGE_DIR_ENV};
        // The credential variable decides whenever it is set — empty, it names
        // the machine's own login; only its absence hands over to the config
        // folder.
        let named = credentials
            .map(|folder| (folder, CLAUDE_SECURE_STORAGE_DIR_ENV))
            .or_else(|| config.map(|folder| (folder, CLAUDE_CONFIG_DIR_ENV)));
        match named.filter(|(folder, _)| !folder.is_empty()) {
            Some((folder, named_by)) => Self {
                folder: Some(PathBuf::from(folder)),
                named_by,
            },
            None => Self {
                folder: None,
                named_by: MACHINE_KEYCHAIN,
            },
        }
    }

    fn service(&self) -> String {
        self.folder.as_ref().map_or_else(
            || KEYCHAIN_SERVICE.to_string(),
            |folder| scoped_keychain_service(folder.as_os_str()),
        )
    }

    /// The `.credentials.json` a scoped login keeps beside it, when there is
    /// one. The machine's own login is read from its keychain item alone, as
    /// it always was.
    fn credentials_file(&self) -> Option<PathBuf> {
        let path = self.folder.as_ref()?.join(CLAUDE_CREDENTIALS_FILE);
        path.is_file().then_some(path)
    }
}

/// `claudeAiOauth.expiresAt` of a blob, the stamp every refresh advances.
fn oauth_expires_at(blob: &Value) -> Option<u64> {
    blob.get("claudeAiOauth")
        .and_then(|oauth| oauth.get("expiresAt"))
        .and_then(Value::as_u64)
}

/// Which copy of a scoped login to believe when both the `.credentials.json`
/// beside it and the CLI's keychain item answer: the one refreshed more
/// recently, by `expiresAt`. A tie — or a blob with no stamp on either side —
/// goes to the keychain, the store the CLI writes to first.
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

/// Treat a token expiring within this window as already expired and ask for a
/// renewal now, instead of letting the request race the boundary and 401.
/// Milliseconds because the keychain's `expiresAt` is Unix ms; mirrors the
/// 60-second `OAUTH_EXPIRY_BUFFER_SECS` used for zo-saved tokens. The CLI
/// renews five minutes ahead of expiry, so an ask inside this window is always
/// one it acts on.
const KEYCHAIN_EXPIRY_BUFFER_MS: u64 = 60_000;

/// Kill switch: set `ZO_DISABLE_KEYCHAIN=1` to skip the Claude Code keychain
/// entirely (also keeps unit tests hermetic on developer machines where the
/// real keychain item exists).
const DISABLE_KEYCHAIN_ENV: &str = "ZO_DISABLE_KEYCHAIN";

/// The account the CLI files its item under: `$USER` as the CLI reads it
/// (`wk()`), or the CLI's own name for one `$USER` cannot spell. `None` when
/// `$USER` is unset — the CLI then asks the password database, which this
/// crate cannot do without unsafe code, and the read goes out without an
/// account as it always did.
///
/// Named on every read because one service can hold two items: the chosen
/// account's folder on the machine this was written for held its live login
/// under the person's account and a stale copy under another (t-11045), and
/// `security` without `-a` answers with whichever it finds first.
fn cli_keychain_account() -> Option<String> {
    cli_keychain_account_from(std::env::var("USER").ok())
}

/// [`cli_keychain_account`] with `$USER` handed in.
fn cli_keychain_account_from(user: Option<String>) -> Option<String> {
    let user = user.filter(|user| !user.is_empty())?;
    let spelled = user
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    Some(if spelled {
        user
    } else {
        CLI_FALLBACK_KEYCHAIN_ACCOUNT.to_string()
    })
}

/// The CLI's account name for a `$USER` outside `[a-zA-Z0-9._-]` (`wk()`).
const CLI_FALLBACK_KEYCHAIN_ACCOUNT: &str = "claude-code-user";

/// The official Claude Code subscription OAuth application. `platform.claude.com`
/// is the developer/console flow, which mints tokens the server refuses to grant
/// `user:inference` on — every `/v1/messages` then 403s `OAuth token does not
/// meet scope requirement`. The subscription flow authorizes on `claude.ai` and
/// exchanges/refreshes on `console.anthropic.com`; both share this client id.
/// Zo's own saved login refreshes against it; a Claude Code login never is
/// (its own CLI renews it).
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

/// A usable Claude Code session from its store: the bearer plus its expiry so
/// the caller can schedule a proactive re-read before the next lapse.
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

/// Identity of the credentials file beside a scoped login at one resolution.
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

/// Process-wide memo of the last real keychain read of the machine's own
/// login.
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
///   proactive buffer (sessions without a recorded expiry are re-read on a
///   fixed cadence);
/// - a miss (absent blob, missing scope, a renewal that did not bring the
///   login back) is negative-cached briefly so a machine without Claude Code
///   credentials doesn't re-fork per turn;
/// - [`invalidate_claude_code_keychain_cache`] forces the next lookup through
///   to the keychain (401 recovery must never be served a cached bearer).
struct KeychainCacheEntry {
    /// The session, or why there was none — a miss keeps its reason, so a
    /// cached answer says "expired and not renewed" as the read did, not just
    /// "nothing".
    session: Result<KeychainSession, CredentialMiss>,
    read_at: Instant,
}

static KEYCHAIN_SESSION_CACHE: Mutex<Option<KeychainCacheEntry>> = Mutex::new(None);
/// Single-flight for the `security` fork: parallel resolvers (turn boundary,
/// model picker, sub-agent spawn) coalesce on one read instead of forking a
/// `security` process each.
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
        // buffer would ask for a renewal anyway, so that timing is unchanged.
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

/// Result of inspecting a Claude Code credential blob.
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

/// Pure evaluation of a parsed credential blob against the current time
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

/// The session a usable blob opens.
fn session_of(blob: &Value, access_token: String) -> KeychainSession {
    let oauth = blob.get("claudeAiOauth");
    KeychainSession {
        access_token,
        expires_at_ms: oauth_expires_at(blob),
        plan: oauth.and_then(blob_plan),
    }
}

/// Read the Claude Code session from the store the CLI keeps, asking that
/// store's own CLI to renew it when it has expired. `None` when the store
/// holds no usable login and a renewal did not bring one back.
#[must_use]
pub fn read_claude_code_keychain_session() -> Option<KeychainSession> {
    read_claude_code_keychain_session_explained().ok()
}

/// [`read_claude_code_keychain_session`], and when there is no session, why:
/// [`CredentialMiss::Absent`] when the store holds no login at all,
/// [`CredentialMiss::Unusable`] when it holds one that could not be used — a
/// session that expired and was not renewed, a login without the inference
/// scope, a keychain that would not answer.
pub fn read_claude_code_keychain_session_explained() -> Result<KeychainSession, CredentialMiss> {
    // `ZO_DISABLE_KEYCHAIN` disables the operating-system keychain, not an
    // explicitly handed-off credentials file. Scoped stores also bypass the
    // keychain memo: the runtime owns the cheaper `(mtime, len)` cache.
    let store = ClaudeCodeStore::current();
    if store.folder.is_some() {
        return read_store_session(&store);
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
    let session = read_store_session(&store);
    store_keychain_session(&session);
    session
}

fn read_store_session(store: &ClaudeCodeStore) -> Result<KeychainSession, CredentialMiss> {
    let blob = match read_store_blob(store) {
        BlobRead::Found(blob) => blob,
        BlobRead::Absent => return Err(CredentialMiss::Absent),
        BlobRead::Unusable(why) => return Err(CredentialMiss::Unusable(why.to_string())),
    };
    match evaluate_keychain_credentials(&blob, now_unix_millis()) {
        KeychainOutcome::Usable(access_token) => {
            eprintln!("\x1b[2mUsing Claude Code session credentials.\x1b[0m");
            Ok(session_of(&blob, access_token))
        }
        KeychainOutcome::Expired => {
            let renewed = renew_through_its_cli(store, &blob);
            match &renewed {
                Ok(_) => eprintln!("\x1b[2mClaude Code renewed its sign-in; using it.\x1b[0m"),
                Err(_) => eprintln!(
                    "\x1b[33mClaude Code sign-in expired and was not renewed, falling back to Zo auth.\x1b[0m"
                ),
            }
            renewed
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
/// kept, not necessarily usable. Never reads a secret and never renews: the
/// scoped store's file or keychain item for a scoped login, the machine's
/// item otherwise. An answer the memo already holds is reused; otherwise one
/// `security` attribute lookup (no `-w`, so no access prompt) settles it.
#[must_use]
pub fn claude_code_login_configured() -> bool {
    let keychain_allowed = std::env::var_os(DISABLE_KEYCHAIN_ENV).is_none();
    let store = ClaudeCodeStore::current();
    if store.folder.is_some() {
        return store.credentials_file().is_some()
            || (keychain_allowed && keychain_item_kept(&store.service()));
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

/// Whether the keychain keeps an item under `service` for the CLI's account:
/// its attributes only, which no access list guards. A keychain that does not
/// answer may well keep one, and saying so costs one more question at the
/// next connection; a machine without `security` keeps none.
fn keychain_item_kept(service: &str) -> bool {
    let account = cli_keychain_account();
    let mut command = Command::new("security");
    command.args(["find-generic-password", "-s", service]);
    if let Some(account) = account.as_deref() {
        command.args(["-a", account]);
    }
    match command.output() {
        Ok(output) => output.status.code() != Some(KEYCHAIN_ITEM_NOT_FOUND),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The Claude Code login this process would read, for a diagnosis: what chose
/// the store — the variable that named its folder, or the keychain for the
/// machine's own login — and the blob it holds. Read only: nothing renewed,
/// nothing written, and no answer is remembered.
pub fn peek_claude_code_login() -> Result<(&'static str, Value), CredentialMiss> {
    let store = ClaudeCodeStore::current();
    match read_store_blob(&store) {
        BlobRead::Found(blob) => Ok((store.named_by, blob)),
        BlobRead::Absent => Err(CredentialMiss::Absent),
        BlobRead::Unusable(why) => Err(CredentialMiss::Unusable(why.to_string())),
    }
}

/// Token-only convenience over [`read_claude_code_keychain_session`].
#[must_use]
pub fn read_claude_code_keychain_token() -> Option<String> {
    read_claude_code_keychain_session().map(|session| session.access_token)
}

/// One ask to renew a Claude Code login: the folder its store is scoped to —
/// `None` for the machine's own login. The renewer runs the store's own CLI
/// with that folder as both its config and its credential folder (the
/// window's renewal of an account nobody runs, t-10915, runs it the same way),
/// so the run refreshes exactly this store, under the same refresh lock every
/// pane of the account takes, and writes its profile into the account's own
/// folder rather than a runtime home other accounts share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeLoginRenewal {
    pub folder: Option<PathBuf>,
}

/// What asking the CLI came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewalRun {
    /// The CLI ran to its end — which says nothing yet about the login: it
    /// exits 0 over a refresh that failed. The store, read again, answers.
    Ran,
    /// The CLI could not be asked — not found, would not start, or outlived
    /// its wall — and why, in words that carry no credential.
    NotRun(String),
}

/// The road by which an expired Claude Code login is renewed.
pub type ClaudeLoginRenewer = Arc<dyn Fn(&ClaudeLoginRenewal) -> RenewalRun + Send + Sync>;

/// Poison policy: recover — the slot is one pointer, written whole.
static RENEWER: RwLock<Option<ClaudeLoginRenewer>> = RwLock::new(None);

/// One renewal at a time in this process: parallel resolvers (a turn, a
/// sub-agent, the model list) that all find the login expired ask the CLI
/// once, and the ones that waited read what that ask brought back.
static RENEWAL_FLIGHT: Mutex<()> = Mutex::new(());

/// Install — or, with `None`, take away — the one road by which an expired
/// Claude Code login is renewed (t-11045). The zo binary installs the run of
/// the store's own CLI at start; a process that installs none never renews,
/// and says the login expired.
pub fn install_claude_login_renewer(renewer: Option<ClaudeLoginRenewer>) {
    *RENEWER
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = renewer;
}

fn installed_renewer() -> Option<ClaudeLoginRenewer> {
    RENEWER
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The store's login, when it is usable right now — a plain read, no renewal.
fn usable_session_in(store: &ClaudeCodeStore) -> Option<KeychainSession> {
    let BlobRead::Found(blob) = read_store_blob(store) else {
        return None;
    };
    match evaluate_keychain_credentials(&blob, now_unix_millis()) {
        KeychainOutcome::Usable(access_token) => Some(session_of(&blob, access_token)),
        _ => None,
    }
}

/// Ask the store's own CLI to renew an expired login, once, and read the
/// store again (t-11045). Zo never spends the refresh token itself: it is the
/// CLI's, and every pane of the same account renews it too.
///
/// What the ask came to is remembered against the expired access token — a
/// fingerprint of it, never the token ([`refresh_gate`]): a CLI that ran and
/// left the login expired means the login itself is gone, and nothing but a
/// new sign-in (which brings a different token) is asked again — never a
/// renewal on a timer; a CLI that could not be asked cools down and is asked
/// again at a later connection.
fn renew_through_its_cli(
    store: &ClaudeCodeStore,
    blob: &Value,
) -> Result<KeychainSession, CredentialMiss> {
    let unusable = |why: &str| CredentialMiss::Unusable(why.to_string());
    let oauth = blob.get("claudeAiOauth");
    let renewable = oauth
        .and_then(|oauth| oauth.get("refreshToken"))
        .and_then(Value::as_str)
        .is_some_and(|token| !token.is_empty());
    if !renewable {
        return Err(unusable(EXPIRED_NO_REFRESH_TOKEN));
    }
    let Some(expired) = oauth
        .and_then(|oauth| oauth.get("accessToken"))
        .and_then(Value::as_str)
    else {
        return Err(CredentialMiss::Absent);
    };
    let _flight = RENEWAL_FLIGHT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Whoever held the flight — or a pane of the same account — may have
    // renewed while this one waited.
    if let Some(session) = usable_session_in(store) {
        return Ok(session);
    }
    match refresh_gate::refresh_blocked(expired) {
        Some(refresh_gate::RefreshBlock::Retired) => return Err(unusable(EXPIRED_RENEWAL_REFUSED)),
        Some(refresh_gate::RefreshBlock::CoolingDown) => return Err(unusable(EXPIRED_RENEWAL_COOLING)),
        None => {}
    }
    let Some(renewer) = installed_renewer() else {
        return Err(unusable(EXPIRED_NO_RENEWER));
    };
    match renewer(&ClaudeLoginRenewal {
        folder: store.folder.clone(),
    }) {
        RenewalRun::Ran => {
            if let Some(session) = usable_session_in(store) {
                refresh_gate::record_success(expired);
                return Ok(session);
            }
            if refresh_gate::record_outcome(expired, true) {
                eprintln!(
                    "\x1b[33mClaude Code could not renew its sign-in; {}.\x1b[0m",
                    claude_sign_in_again!()
                );
            }
            Err(unusable(EXPIRED_RENEWAL_REFUSED))
        }
        RenewalRun::NotRun(why) => {
            refresh_gate::record_outcome(expired, false);
            Err(CredentialMiss::Unusable(format!(
                "the Claude Code sign-in expired and its CLI could not be asked to renew it ({why}); \
                 the next connection asks again"
            )))
        }
    }
}

/// Hard bounds on the refresh round-trip of zo's own saved login. Credential
/// resolution can run on a startup/turn-boundary path; a blackholed network
/// (offline, sandboxed test runner) must bound the wait instead of hanging
/// that path forever — the shared HTTP pool deliberately carries no overall
/// timeout for streaming, so the refresh uses its own client.
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
            .map_err(|_| ApiError::Auth("OAuth refresh thread panicked".to_string()))?
    })
}

/// What the store that holds the Claude Code login said.
enum BlobRead {
    Found(Value),
    /// No login is kept there.
    Absent,
    /// A login is kept there and could not be read — why.
    Unusable(&'static str),
}

fn read_store_blob(store: &ClaudeCodeStore) -> BlobRead {
    let keychain_allowed = std::env::var_os(DISABLE_KEYCHAIN_ENV).is_none();
    let account = cli_keychain_account();
    // A scoped login keeps two copies — the `.credentials.json` beside it and
    // the CLI's scoped keychain item — and the fresher one is the login.
    // Never the UNSCOPED item: that copy belongs to another lineage (the
    // bare-terminal `claude`). A folder with neither is "not signed in".
    if store.folder.is_some() {
        let file_path = store.credentials_file();
        let file = file_path.as_deref().and_then(read_credentials_file);
        let scoped = keychain_allowed
            .then(|| read_keychain_service_secret(&store.service(), account.as_deref()));
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
    if !keychain_allowed {
        return BlobRead::Absent;
    }
    match read_keychain_service_secret(KEYCHAIN_SERVICE, account.as_deref()) {
        KeychainAnswer::Found(raw) => {
            parse_keychain_blob(&raw).map_or(BlobRead::Unusable(LOGIN_NOT_A_DOCUMENT), BlobRead::Found)
        }
        KeychainAnswer::Absent => BlobRead::Absent,
        KeychainAnswer::Unanswered => BlobRead::Unusable(KEYCHAIN_UNANSWERED),
    }
}

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

/// One keychain item's secret as `security find-generic-password -w` prints
/// it, trimmed — filed under `account` when one is named.
fn read_keychain_service_secret(service: &str, account: Option<&str>) -> KeychainAnswer {
    let mut command = Command::new("security");
    command.args(["find-generic-password", "-s", service]);
    if let Some(account) = account {
        command.args(["-a", account]);
    }
    let output = match command.arg("-w").output() {
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
    read_keychain_service_secret(service, None)
}

/// The credentials file a scoped login keeps beside its settings.
const CLAUDE_CREDENTIALS_FILE: &str = ".credentials.json";

/// Current `(mtime, len)` identity of the credentials file beside the scoped
/// login this process reads — `None` when the login lives in the keychain
/// alone, as it does on macOS for a login the CLI keeps well.
#[must_use]
pub fn managed_claude_credentials_stamp() -> Option<ManagedCredentialsStamp> {
    let path = ClaudeCodeStore::current().credentials_file()?;
    let metadata = std::fs::metadata(&path).ok()?;
    Some(ManagedCredentialsStamp {
        path,
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
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

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod store_tests {
    use super::{
        CLI_FALLBACK_KEYCHAIN_ACCOUNT, ClaudeCodeStore, KEYCHAIN_SERVICE, MACHINE_KEYCHAIN,
        cli_keychain_account_from, scoped_keychain_service,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// The CLI's rule, row by row: a named credential folder wins over the
    /// config folder (the window pane's case — its config folder is the
    /// shared runtime home, a copy); a credential folder set but empty is the
    /// machine's own login even beside a config folder; without the credential
    /// variable the config folder scopes the login; with neither, the
    /// machine's own.
    #[test]
    fn the_store_is_the_one_the_cli_keeps_and_renews() {
        let runtime_home = OsString::from("/Users/dev/app/.claude");
        let account = OsString::from("/Users/dev/app/claude-accounts/a-1");
        let pane = ClaudeCodeStore::from_folders(Some(account.clone()), Some(runtime_home.clone()));
        assert_eq!(pane.folder, Some(PathBuf::from(&account)));
        assert_eq!(pane.named_by, crate::managed_account::CLAUDE_SECURE_STORAGE_DIR_ENV);
        assert_eq!(pane.service(), scoped_keychain_service(&account));
        assert_ne!(
            pane.service(),
            scoped_keychain_service(&runtime_home),
            "a pane read the window's copy of its login"
        );

        let machine = ClaudeCodeStore::from_folders(Some(OsString::new()), Some(runtime_home.clone()));
        assert_eq!(machine.folder, None);
        assert_eq!(machine.service(), KEYCHAIN_SERVICE);
        assert_eq!(machine.named_by, MACHINE_KEYCHAIN);

        let configured = ClaudeCodeStore::from_folders(None, Some(runtime_home.clone()));
        assert_eq!(configured.service(), scoped_keychain_service(&runtime_home));
        assert_eq!(configured.named_by, crate::managed_account::CLAUDE_CONFIG_DIR_ENV);

        assert_eq!(ClaudeCodeStore::from_folders(None, None).service(), KEYCHAIN_SERVICE);
        assert_eq!(ClaudeCodeStore::from_folders(None, Some(OsString::new())).folder, None);
    }

    /// `$USER` names the item as the CLI files it; a name the CLI would not
    /// keep becomes the CLI's own fallback; no `$USER` is no account at all.
    #[test]
    fn the_keychain_account_is_the_one_the_cli_files_under() {
        assert_eq!(cli_keychain_account_from(Some("dev".into())).as_deref(), Some("dev"));
        assert_eq!(
            cli_keychain_account_from(Some("dev.name_2-x".into())).as_deref(),
            Some("dev.name_2-x")
        );
        assert_eq!(
            cli_keychain_account_from(Some("dev name".into())).as_deref(),
            Some(CLI_FALLBACK_KEYCHAIN_ACCOUNT)
        );
        assert_eq!(cli_keychain_account_from(Some(String::new())), None);
        assert_eq!(cli_keychain_account_from(None), None);
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
    use super::read_credentials_file;
    use serde_json::json;

    /// The blob the CLI wrote is read as it is, and a loose file is tightened
    /// to owner-only on the way — the file holds a refresh credential.
    #[test]
    fn the_blob_the_cli_wrote_is_read_and_kept_owner_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        let blob = json!({"claudeAiOauth": {"accessToken": "a", "refreshToken": "r", "expiresAt": 1}});
        std::fs::write(&path, blob.to_string()).expect("write the CLI's blob");
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
        claude_code_oauth_config, evaluate_keychain_credentials,
    };
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
        // A token lapsing in under the buffer asks for its renewal now instead
        // of racing the boundary and 401ing mid-turn.
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
        // proactive buffer window.
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
        // "expired and not renewed" must not come back as "nothing here".
        let refused = Err(crate::credential::CredentialMiss::Unusable(
            super::EXPIRED_RENEWAL_REFUSED.to_string(),
        ));
        super::store_keychain_session(&refused);
        assert_eq!(
            super::cached_keychain_session(),
            super::KeychainCacheLookup::Fresh(refused)
        );
        super::invalidate_claude_code_keychain_cache();
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
