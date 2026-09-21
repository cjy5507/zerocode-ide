//! Filesystem-backed OAuth credential persistence.
//!
//! This module provides load/save/clear operations for OAuth token sets,
//! as well as PKCE helper functions needed by the OAuth authorization flow.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use core_types::paths::restrict_permissions_owner_only;
use core_types::{OAuthTokenSet, OpenAiOAuthTokens};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use core_types::PkceCodePair;

const OAUTH_KEY: &str = "oauth";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredOAuthCredentials {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    scopes: Vec<String>,
}

impl From<OAuthTokenSet> for StoredOAuthCredentials {
    fn from(value: OAuthTokenSet) -> Self {
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_at: value.expires_at,
            scopes: value.scopes,
        }
    }
}

impl From<&OAuthTokenSet> for StoredOAuthCredentials {
    fn from(value: &OAuthTokenSet) -> Self {
        Self {
            access_token: value.access_token.clone(),
            refresh_token: value.refresh_token.clone(),
            expires_at: value.expires_at,
            scopes: value.scopes.clone(),
        }
    }
}

impl From<StoredOAuthCredentials> for OAuthTokenSet {
    fn from(value: StoredOAuthCredentials) -> Self {
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_at: value.expires_at,
            scopes: value.scopes,
        }
    }
}

// --- Generic credential-key helpers ---
//
// These compose [`read_credentials_root`]/[`update_credentials_root`] with the
// `StoredOAuthCredentials` wire shape, and are the shared building blocks for
// the Anthropic and Gemini entries below as well as the runtime's per-server
// MCP token storage (which layers its own nested layout on top).

/// Decode a stored credential JSON value into an [`OAuthTokenSet`].
pub fn token_set_from_value(value: &Value) -> io::Result<OAuthTokenSet> {
    StoredOAuthCredentials::deserialize(value)
        .map(Into::into)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Encode an [`OAuthTokenSet`] into its stored JSON representation.
pub fn token_set_to_value(token_set: &OAuthTokenSet) -> io::Result<Value> {
    serde_json::to_value(StoredOAuthCredentials::from(token_set))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Load the [`OAuthTokenSet`] stored under `key` at the top level of
/// `credentials.json`, treating a missing or `null` entry as `None`.
pub fn load_token_set(key: &str) -> io::Result<Option<OAuthTokenSet>> {
    let root = read_credentials_root(&credentials_path()?)?;
    match root.get(key) {
        Some(entry) if !entry.is_null() => token_set_from_value(entry).map(Some),
        _ => Ok(None),
    }
}

/// Persist `token_set` under `key`, leaving every other entry untouched.
pub fn save_token_set(key: &str, token_set: &OAuthTokenSet) -> io::Result<()> {
    update_credentials_root(&credentials_path()?, |root| {
        root.insert(key.to_owned(), token_set_to_value(token_set)?);
        Ok(())
    })
}

/// Remove the entry stored under `key`, leaving every other entry untouched.
pub fn clear_token_key(key: &str) -> io::Result<()> {
    update_credentials_root(&credentials_path()?, |root| {
        root.remove(key);
        Ok(())
    })
}

pub fn generate_pkce_pair() -> io::Result<PkceCodePair> {
    let verifier = generate_random_token(32)?;
    Ok(PkceCodePair {
        challenge: code_challenge_s256(&verifier),
        verifier,
        challenge_method: core_types::PkceChallengeMethod::S256,
    })
}

pub fn generate_state() -> io::Result<String> {
    generate_random_token(32)
}

#[must_use]
pub fn code_challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64url_encode(&digest)
}

#[must_use]
pub fn loopback_redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

pub fn credentials_path() -> io::Result<PathBuf> {
    Ok(credentials_home_dir().join("credentials.json"))
}

pub fn load_oauth_credentials() -> io::Result<Option<OAuthTokenSet>> {
    load_token_set(OAUTH_KEY)
}

pub fn save_oauth_credentials(token_set: &OAuthTokenSet) -> io::Result<()> {
    save_token_set(OAUTH_KEY, token_set)
}

pub fn clear_oauth_credentials() -> io::Result<()> {
    clear_token_key(OAUTH_KEY)
}

const OPENAI_OAUTH_KEY: &str = "openai_oauth";

/// On-disk representation of [`OpenAiOAuthTokens`] (camelCase), stored under its
/// own key so the Anthropic and ChatGPT credentials never collide. Mirrors the
/// `runtime::oauth` copy the CLI writes; this api-side reader is what the
/// provider router and token refresh consume.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredOpenAiOAuth {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
}

impl From<&OpenAiOAuthTokens> for StoredOpenAiOAuth {
    fn from(value: &OpenAiOAuthTokens) -> Self {
        Self {
            access_token: value.access_token.clone(),
            refresh_token: value.refresh_token.clone(),
            expires_at: value.expires_at,
            account_id: value.account_id.clone(),
            scopes: value.scopes.clone(),
        }
    }
}

impl From<StoredOpenAiOAuth> for OpenAiOAuthTokens {
    fn from(value: StoredOpenAiOAuth) -> Self {
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_at: value.expires_at,
            account_id: value.account_id,
            scopes: value.scopes,
        }
    }
}

/// Which login this pane speaks as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiAuthSource {
    /// A Codex `auth.json` — the window's account, however it was found
    /// (`crate::managed_account::resolve_codex_home`).
    CodexHome(crate::managed_account::CodexHomeSource),
    /// zo's own `credentials.json`, from `zo login openai`.
    OwnLogin,
}

/// Load saved ChatGPT OAuth tokens for the provider router.
///
/// Resolution: the window's account first — the `auth.json` of the Codex home
/// [`crate::managed_account::resolve_codex_home`] names, which is the account
/// the person is signed in to in the ZeroCode window whether or not this shell
/// was handed a `CODEX_HOME` — then zo's own `credentials.json`
/// (`zo login openai`). Codex and zo share one OAuth client id, so its tokens
/// are ours to use; see [`save_openai_oauth`] for why a rotation then goes back
/// to that same file.
///
/// The window's account wins because every usage surface already reads it: the
/// quota probe bills `/status` against that home, so an older `zo login openai`
/// taking precedence spent one account while the whole UI reported another —
/// requests 429'd on an exhausted plan under a status line showing a full one,
/// and, once that own login expired, 401'd while `codex login status` next to
/// it said the person was signed in (2026-09-21, t-5777).
pub fn load_openai_oauth() -> io::Result<Option<OpenAiOAuthTokens>> {
    Ok(resolve_openai_oauth()?.map(|resolved| resolved.tokens))
}

/// [`load_openai_oauth`] plus the source, for the surfaces that name it: the
/// `/status` card's origin column and the message a dead login prints.
pub fn load_openai_oauth_with_source(
) -> io::Result<Option<(OpenAiOAuthTokens, OpenAiAuthSource)>> {
    Ok(resolve_openai_oauth()?.map(|resolved| (resolved.tokens, resolved.source)))
}

/// Where this pane's ChatGPT login came from, without reading the tokens
/// themselves into a caller that only draws a row.
#[must_use]
pub fn openai_oauth_source() -> Option<OpenAiAuthSource> {
    resolve_openai_oauth().ok().flatten().map(|resolved| resolved.source)
}

/// One resolved ChatGPT login: the tokens, where they came from, and where a
/// rotation must be written back — `Some(path)` for a Codex home, `None` for
/// zo's own store.
struct ResolvedOpenAiOAuth {
    tokens: OpenAiOAuthTokens,
    source: OpenAiAuthSource,
    write_back: Option<PathBuf>,
}

fn resolve_openai_oauth() -> io::Result<Option<ResolvedOpenAiOAuth>> {
    // A hand-off that cannot be read is not a hand-off. Falling through to zo's
    // own login beats failing the turn on a half-written mirror.
    if let Some((path, source)) = codex_auth::auth_json_path_with_source() {
        if let Some(tokens) = codex_auth::load_at(&path).ok().flatten() {
            return Ok(Some(ResolvedOpenAiOAuth {
                tokens,
                source: OpenAiAuthSource::CodexHome(source),
                write_back: Some(path),
            }));
        }
    }
    Ok(load_own_openai_oauth()?.map(|tokens| ResolvedOpenAiOAuth {
        tokens,
        source: OpenAiAuthSource::OwnLogin,
        write_back: None,
    }))
}

/// Whether an incoming token set belongs to the account a file already holds.
/// An unknown account on either side counts as the same one: a rotation carries
/// the account forward, so its absence means "unchanged", not "someone else".
fn same_openai_account(held: &OpenAiOAuthTokens, incoming: &OpenAiOAuthTokens) -> bool {
    match (held.account_id.as_deref(), incoming.account_id.as_deref()) {
        (Some(held), Some(incoming)) => held == incoming,
        _ => true,
    }
}

fn load_own_openai_oauth() -> io::Result<Option<OpenAiOAuthTokens>> {
    let path = credentials_path()?;
    let root = read_credentials_root(&path)?;
    let Some(entry) = root.get(OPENAI_OAUTH_KEY) else {
        return Ok(None);
    };
    if entry.is_null() {
        return Ok(None);
    }
    let stored = StoredOpenAiOAuth::deserialize(entry)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(stored.into()))
}

/// Persist refreshed ChatGPT OAuth tokens (after a token refresh).
///
/// A rotation of the handed-off Codex login is written back THERE: ChatGPT
/// rotates refresh tokens, so keeping the new one only in zo's store would
/// strand the Codex CLI on a dead branch — the same discipline the Claude
/// keychain write-back follows. Only a rotation of *that* account goes there;
/// a fresh `zo login openai` naming a different account is zo's own and lands
/// in `credentials.json`, which is also the write [`codex_auth::save_at`] would
/// (rightly) refuse against someone else's login file.
pub fn save_openai_oauth(tokens: &OpenAiOAuthTokens) -> io::Result<()> {
    if let Some(held) = resolve_openai_oauth()? {
        if let Some(path) = held.write_back {
            if same_openai_account(&held.tokens, tokens) {
                return codex_auth::save_at(&path, tokens);
            }
        }
    }
    let path = credentials_path()?;
    update_credentials_root(&path, |root| {
        root.insert(
            OPENAI_OAUTH_KEY.to_owned(),
            serde_json::to_value(StoredOpenAiOAuth::from(tokens))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        Ok(())
    })
}

/// Remove saved ChatGPT OAuth tokens.
pub fn clear_openai_oauth() -> io::Result<()> {
    let path = credentials_path()?;
    update_credentials_root(&path, |root| {
        root.remove(OPENAI_OAUTH_KEY);
        Ok(())
    })
}

const OPENAI_COMPAT_API_KEYS_KEY: &str = "openai_compat_api_keys";

/// Load a saved API key for an OpenAI-compatible adapter env var (for example
/// `DEEPSEEK_API_KEY`). Environment variables still take precedence at request
/// time; this store is the durable fallback populated by TUI `/connect`.
pub fn load_openai_compat_api_key(env_key: &str) -> io::Result<Option<String>> {
    let path = credentials_path()?;
    let root = read_credentials_root(&path)?;
    let Some(entry) = root.get(OPENAI_COMPAT_API_KEYS_KEY).and_then(Value::as_object) else {
        return Ok(None);
    };
    Ok(entry
        .get(env_key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string))
}

/// Persist an OpenAI-compatible adapter API key under its configured env var
/// name, leaving OAuth credentials and other adapter keys untouched.
pub fn save_openai_compat_api_key(env_key: &str, api_key: &str) -> io::Result<()> {
    let trimmed_env = env_key.trim();
    let trimmed_key = api_key.trim();
    if trimmed_env.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "adapter API-key env var name cannot be empty",
        ));
    }
    if trimmed_key.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "adapter API key cannot be empty",
        ));
    }
    let path = credentials_path()?;
    update_credentials_root(&path, |root| {
        let entry = root
            .entry(OPENAI_COMPAT_API_KEYS_KEY.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(keys) = entry.as_object_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "openai-compatible API key store must be a JSON object",
            ));
        };
        keys.insert(trimmed_env.to_string(), Value::String(trimmed_key.to_string()));
        Ok(())
    })
}

/// Forget one adapter API key, leaving OAuth credentials and every other
/// adapter key in place. Used when a provider is deleted from `/providers` and
/// the user asks for its stored key to go with it.
///
/// Removing the entry from the effective view is enough: `update_credentials_root`
/// reconciles the write back into a per-entry `null` tombstone whenever a lower
/// credential root still carries the key, so the deletion is durable rather than
/// silently reappearing from `$HOME/.zo` on the next read.
///
/// Returns whether a key was actually stored under `env_key`.
pub fn delete_openai_compat_api_key(env_key: &str) -> io::Result<bool> {
    let trimmed_env = env_key.trim();
    if trimmed_env.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "adapter API-key env var name cannot be empty",
        ));
    }
    let path = credentials_path()?;
    let mut removed = false;
    update_credentials_root(&path, |root| {
        let Some(keys) = root
            .get_mut(OPENAI_COMPAT_API_KEYS_KEY)
            .and_then(Value::as_object_mut)
        else {
            return Ok(());
        };
        removed = keys.remove(trimmed_env).is_some();
        Ok(())
    })?;
    Ok(removed)
}

const GOOGLE_CODE_ASSIST_OAUTH_KEY: &str = "google_code_assist_oauth";

/// Load saved Gemini Code Assist OAuth tokens, if `/login google` has run.
pub fn load_google_code_assist_oauth() -> io::Result<Option<OAuthTokenSet>> {
    load_token_set(GOOGLE_CODE_ASSIST_OAUTH_KEY)
}

/// Persist Gemini Code Assist OAuth tokens.
pub fn save_google_code_assist_oauth(tokens: &OAuthTokenSet) -> io::Result<()> {
    save_token_set(GOOGLE_CODE_ASSIST_OAUTH_KEY, tokens)
}

/// Remove saved Gemini Code Assist OAuth tokens — and the project resolved
/// for them, which belongs to that login.
pub fn clear_google_code_assist_oauth() -> io::Result<()> {
    clear_token_key(GOOGLE_CODE_ASSIST_PROJECT_KEY)?;
    clear_token_key(GOOGLE_CODE_ASSIST_OAUTH_KEY)
}

const GOOGLE_CODE_ASSIST_PROJECT_KEY: &str = "google_code_assist_project";

/// Hex digits of the refresh token's SHA-256 that name a Google grant in the
/// project record — enough to tell one login from another, never the token.
const GRANT_FINGERPRINT_LEN: usize = 16;

/// A fingerprint of the Google grant a Code Assist project was resolved for.
/// Google refresh tokens do not rotate on refresh, so this names the login
/// across processes without storing anything a reader could use.
#[must_use]
pub fn grant_fingerprint(refresh_token: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(refresh_token.as_bytes()));
    digest[..GRANT_FINGERPRINT_LEN].to_string()
}

/// What the store remembers about a grant's Code Assist project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RememberedProject {
    /// Nothing for THIS grant — another login's record does not count.
    NotRemembered,
    /// The resolution, which may itself be "no project" (free tier).
    Remembered(Option<String>),
}

/// The Code Assist project `loadCodeAssist` / `onboardUser` resolved for the
/// grant `grant` while the configuration named the project `configured`
/// (`GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_PROJECT_ID`, or none), remembered
/// across processes.
///
/// The answer depends on both: the configured project is sent to
/// `loadCodeAssist` and is the answer when the backend names none, so a record
/// made under another configuration — or before the record said which, as
/// records written before 2026-09-11 do not — is not this configuration's.
///
/// Why a record on disk: the resolution is one or two round-trips to the Code
/// Assist backend and it was paid before the FIRST request of every zo
/// process — measured 2026-09-10 as the difference between a session's first
/// and second request to Gemini (first byte 6.07 s vs 1.83 s).
pub fn load_google_code_assist_project(
    grant: &str,
    configured: Option<&str>,
) -> io::Result<RememberedProject> {
    let root = read_credentials_root(&credentials_path()?)?;
    let Some(record) = root.get(GOOGLE_CODE_ASSIST_PROJECT_KEY) else {
        return Ok(RememberedProject::NotRemembered);
    };
    if record.get("grant").and_then(Value::as_str) != Some(grant) {
        return Ok(RememberedProject::NotRemembered);
    }
    let recorded_configuration = match record.get(CONFIGURED_PROJECT_FIELD) {
        Some(Value::Null) => None,
        Some(Value::String(project)) => Some(project.as_str()),
        _ => return Ok(RememberedProject::NotRemembered),
    };
    if recorded_configuration != configured {
        return Ok(RememberedProject::NotRemembered);
    }
    Ok(RememberedProject::Remembered(
        record
            .get("project")
            .and_then(Value::as_str)
            .filter(|project| !project.is_empty())
            .map(str::to_string),
    ))
}

/// The project record's field naming the configured project it was resolved under.
const CONFIGURED_PROJECT_FIELD: &str = "configured_project";

/// Remember the project resolved for `grant` under the configured project
/// `configured`, replacing any other record.
pub fn save_google_code_assist_project(
    grant: &str,
    configured: Option<&str>,
    project: Option<&str>,
) -> io::Result<()> {
    update_credentials_root(&credentials_path()?, |root| {
        root.insert(
            GOOGLE_CODE_ASSIST_PROJECT_KEY.to_owned(),
            serde_json::json!({
                "grant": grant,
                CONFIGURED_PROJECT_FIELD: configured,
                "project": project,
            }),
        );
        Ok(())
    })
}

/// Forget the remembered project — the backend rejected it, or the login is gone.
pub fn clear_google_code_assist_project() -> io::Result<()> {
    clear_token_key(GOOGLE_CODE_ASSIST_PROJECT_KEY)
}

/// A saved-OAuth provider whose presence `doctor` reports without reading token
/// values. Each maps to the top-level credentials key written by its login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedOAuthProvider {
    /// Anthropic Claude OAuth (`oauth`).
    Anthropic,
    /// ChatGPT / OpenAI OAuth (`openai_oauth`).
    OpenAi,
    /// Gemini Code Assist OAuth (`google_code_assist_oauth`).
    GoogleCodeAssist,
}

impl SavedOAuthProvider {
    const fn key(self) -> &'static str {
        match self {
            Self::Anthropic => OAUTH_KEY,
            Self::OpenAi => OPENAI_OAUTH_KEY,
            Self::GoogleCodeAssist => GOOGLE_CODE_ASSIST_OAUTH_KEY,
        }
    }
}

/// Whether a saved OAuth credential is present for `provider`, established
/// without following a symlink at the `credentials.json` leaf and without
/// exposing any token value. This is a secret-safe *presence* probe for
/// `doctor`: it never parses, prints, refreshes, or mints credentials.
///
/// The credentials file is read once through a no-follow secure reader
/// (`read_file`), so a `credentials.json` replaced by a symlink is rejected
/// rather than followed — closing the preflight-lstat-then-following-read race
/// that the ordinary `fs::read_to_string`-backed loaders would leave open. A
/// missing or empty file, or a `null`/absent entry, reports absent.
pub fn saved_oauth_present(
    provider: SavedOAuthProvider,
    read_file: &dyn Fn(&Path) -> io::Result<Option<String>>,
) -> io::Result<bool> {
    let path = credentials_path()?;
    let Some(contents) = read_file(&path)? else {
        return Ok(false);
    };
    if contents.trim().is_empty() {
        return Ok(false);
    }
    let root: Map<String, Value> = serde_json::from_str::<Value>(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "credentials file must contain a JSON object",
            )
        })?;
    Ok(saved_oauth_entry_present(&root, provider))
}

/// Whether a saved OAuth credential is present for `provider` across the full
/// effective credential view — the `ZO_CONFIG_HOME → ZO_HOME → ~/.zo → ~/.forge`
/// chain that the normal provider loaders read — established without following a
/// symlink at any `credentials.json` and without exposing any token value.
///
/// Each root's `credentials.json` is read once through the caller-supplied
/// no-follow `read_file` (so a symlinked or unsafe leaf at any root surfaces as
/// an error rather than being followed), then layered highest-priority-first
/// with the same atomic-replacement and `null`-tombstone semantics as
/// [`read_credentials_root`]. A provider whose highest-priority present entry is
/// a `null` tombstone reports absent, matching a logout. This is the effective
/// counterpart of [`saved_oauth_present`], which only inspects the primary root.
pub fn saved_oauth_present_effective(
    provider: SavedOAuthProvider,
    read_file: &dyn Fn(&Path) -> io::Result<Option<String>>,
) -> io::Result<bool> {
    Ok(saved_oauth_liveness_effective(provider, read_file)?.is_some())
}

/// What a diagnostic can say about a saved credential without contacting anyone.
///
/// Scalars only — never a token, and never a hash of one. `expires_at` is the
/// stored Unix-second expiry; `has_refresh_token` is whether a renewal is even
/// possible. Together they separate "expired, refreshes on next use" (routine)
/// from "expired with nothing to refresh from" (a dead credential that only a
/// new sign-in fixes), which a presence-only probe reported identically — as a
/// clean `PASS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedOAuthLiveness {
    pub expires_at: Option<u64>,
    pub has_refresh_token: bool,
}

impl SavedOAuthLiveness {
    /// Whether the stored access token is past its recorded expiry.
    #[must_use]
    pub fn expired_at(self, now_unix: u64) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now_unix)
    }

    /// Expired with no way to renew: nothing short of a new sign-in helps.
    #[must_use]
    pub fn unusable_at(self, now_unix: u64) -> bool {
        self.expired_at(now_unix) && !self.has_refresh_token
    }
}

/// [`saved_oauth_present_effective`] with the non-secret liveness fields the
/// caller needs to say something useful. `None` when no credential is present.
pub fn saved_oauth_liveness_effective(
    provider: SavedOAuthProvider,
    read_file: &dyn Fn(&Path) -> io::Result<Option<String>>,
) -> io::Result<Option<SavedOAuthLiveness>> {
    // Layer low-to-high so the highest-priority root wins, mirroring
    // `read_lower_roots` + primary. `null` acts as a tombstone via `apply_root`.
    let mut effective = Map::new();
    for root_dir in credential_roots().into_iter().rev() {
        let path = root_dir.join("credentials.json");
        let Some(contents) = read_file(&path)? else {
            continue;
        };
        if contents.trim().is_empty() {
            continue;
        }
        let overlay: Map<String, Value> = serde_json::from_str::<Value>(&contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .as_object()
            .cloned()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "credentials root is not a JSON object",
                )
            })?;
        effective = apply_root(effective, &overlay);
    }
    // `apply_root` already drops `null` tombstones, so a present entry here is a
    // live credential.
    let Some(entry) = effective
        .get(provider.key())
        .filter(|entry| !entry.is_null())
    else {
        return Ok(None);
    };
    Ok(Some(SavedOAuthLiveness {
        expires_at: entry.get("expiresAt").and_then(Value::as_u64),
        has_refresh_token: entry
            .get("refreshToken")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty()),
    }))
}

/// Whether `root` carries a non-null saved-OAuth entry for `provider`.
fn saved_oauth_entry_present(root: &Map<String, Value>, provider: SavedOAuthProvider) -> bool {
    root.get(provider.key())
        .is_some_and(|entry| !entry.is_null())
}

fn generate_random_token(bytes: usize) -> io::Result<String> {
    let mut buffer = vec![0_u8; bytes];
    File::open("/dev/urandom")?.read_exact(&mut buffer)?;
    Ok(base64url_encode(&buffer))
}

fn credential_roots() -> Vec<PathBuf> {
    let mut roots = core_types::paths::zo_global_config_roots();
    if roots.is_empty() {
        roots.push(core_types::paths::default_config_home());
    }
    roots
}

fn credentials_home_dir() -> PathBuf {
    credential_roots()
        .into_iter()
        .next()
        .unwrap_or_else(core_types::paths::default_config_home)
}

fn ensure_credentials_parent(parent: &Path) -> io::Result<()> {
    #[cfg(windows)]
    core_types::paths::ensure_windows_owner_only_dir_no_follow(parent)?;
    #[cfg(not(windows))]
    fs::create_dir_all(parent)?;
    restrict_permissions_owner_only(parent)
}

/// Top-level credential keys whose value is a *collection map* of independent
/// per-entry credentials: the MCP server token map (`mcp_oauth`, owned by
/// `runtime::oauth`) and the adapter API-key map ([`OPENAI_COMPAT_API_KEYS_KEY`]).
/// Only these keys merge per entry across roots — each entry is atomic, and a
/// `null` entry value is a per-entry tombstone. Every other top-level entry is
/// an atomic credential object: the highest root wins the whole object and its
/// fields are never combined across roots.
const MCP_OAUTH_COLLECTION_KEY: &str = "mcp_oauth";

fn is_collection_map_key(key: &str) -> bool {
    key == OPENAI_COMPAT_API_KEYS_KEY || key == MCP_OAUTH_COLLECTION_KEY
}

/// Atomically read, mutate, and write back the credential root object under an
/// advisory file lock. Exposed so dependent crates (e.g. the runtime's MCP
/// token storage) can compose their own credential layouts on the same file.
///
/// Writes go only to the primary root. The closure operates on the merged
/// *effective* view (primary over the lower `ZO_HOME`/`HOME`/.zo roots); the
/// resulting primary file is reconciled against those lower roots so a value
/// the closure removed is written back as a `null` tombstone whenever a lower
/// root still carries it. That keeps logout durable even though the lower copy
/// is never touched.
pub fn update_credentials_root(
    path: &Path,
    update: impl FnOnce(&mut Map<String, Value>) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_credentials_parent(parent)?;
    }
    let lock_path = path.with_extension("json.lock");
    let _lock = CredentialFileLock::acquire(&lock_path)?;

    let primary = core_types::paths::default_config_home();
    let is_primary_root =
        path.file_name().is_some() && path.parent() == Some(primary.as_path());
    if !is_primary_root {
        // Files outside the primary config home have no lower-root layering, so
        // a plain single-file read-modify-write is the whole story.
        let mut root = read_one_credentials_root(path)?;
        update(&mut root)?;
        return write_credentials_root(path, &root);
    }

    let file_name = path.file_name().expect("primary root checked above");
    let lower = read_lower_roots(file_name)?;
    let primary_before = read_one_credentials_root(path)?;
    let mut effective = apply_root(lower.clone(), &primary_before);
    update(&mut effective)?;
    let primary_after = reconcile_primary(&lower, &effective);
    write_credentials_root(path, &primary_after)
}

/// Read the *effective* credential view for `path`: the primary root layered
/// over the lower canonical roots. Non-collection entries are atomic (the
/// highest root wins the whole object); the known collection maps merge per
/// entry. `null` values act as tombstones and are dropped from the result, so
/// callers see a cleaned view with no deleted keys or entries.
pub fn read_credentials_root(path: &Path) -> io::Result<Map<String, Value>> {
    let primary = core_types::paths::default_config_home();
    let Some(file_name) = path.file_name() else {
        return read_one_credentials_root(path);
    };
    if path.parent() != Some(primary.as_path()) {
        return read_one_credentials_root(path);
    }

    let lower = read_lower_roots(file_name)?;
    let primary_root = read_one_credentials_root(path)?;
    Ok(apply_root(lower, &primary_root))
}

/// Merge the lower canonical roots (every root except the primary) low-to-high
/// into a single effective view. Returns an empty map when only the primary
/// root exists.
fn read_lower_roots(file_name: &std::ffi::OsStr) -> io::Result<Map<String, Value>> {
    let mut roots = credential_roots();
    if roots.is_empty() {
        return Ok(Map::new());
    }
    let lower = roots.split_off(1);
    let mut acc = Map::new();
    for root in lower.into_iter().rev() {
        let candidate = root.join(file_name);
        acc = apply_root(acc, &read_one_credentials_root(&candidate)?);
    }
    Ok(acc)
}

/// Layer `overlay` onto `base`, producing the effective view. Non-collection
/// keys replace atomically (fields are never combined); collection-map keys
/// merge per entry. `null` in `overlay` deletes: a top-level `null` drops the
/// whole entry, a per-entry `null` inside a collection map drops that entry.
/// The returned map never contains `null` tombstones.
fn apply_root(mut base: Map<String, Value>, overlay: &Map<String, Value>) -> Map<String, Value> {
    for (key, value) in overlay {
        if is_collection_map_key(key) {
            apply_collection_overlay(&mut base, key, value);
        } else if value.is_null() {
            base.remove(key);
        } else {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}

/// Apply a collection-map overlay entry-by-entry, honoring per-entry `null`
/// tombstones and a whole-map `null` tombstone.
fn apply_collection_overlay(base: &mut Map<String, Value>, key: &str, overlay_value: &Value) {
    if overlay_value.is_null() {
        base.remove(key);
        return;
    }
    let Some(overlay_map) = overlay_value.as_object() else {
        // Malformed overlay (not an object): fall back to atomic replacement.
        base.insert(key.to_string(), overlay_value.clone());
        return;
    };
    let mut merged = base
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (entry_key, entry_value) in overlay_map {
        if entry_value.is_null() {
            merged.remove(entry_key);
        } else {
            merged.insert(entry_key.clone(), entry_value.clone());
        }
    }
    if merged.is_empty() {
        base.remove(key);
    } else {
        base.insert(key.to_string(), Value::Object(merged));
    }
}

/// Compute the primary-root contents so that layering the lower roots under it
/// reproduces `effective`. Present values are written through (migrating
/// lower-root-only credentials up); values that `effective` dropped but a lower
/// root still carries are written as `null` tombstones so the deletion sticks.
fn reconcile_primary(
    lower: &Map<String, Value>,
    effective: &Map<String, Value>,
) -> Map<String, Value> {
    let keys: BTreeSet<&String> = effective.keys().chain(lower.keys()).collect();
    let mut out = Map::new();
    for key in keys {
        let eff = effective.get(key).filter(|value| !value.is_null());
        let low = lower.get(key).filter(|value| !value.is_null());
        if is_collection_map_key(key) {
            let eff_map = eff.and_then(Value::as_object);
            let low_map = low.and_then(Value::as_object);
            let mut sub = Map::new();
            if let Some(entries) = eff_map {
                for (entry_key, entry_value) in entries {
                    if !entry_value.is_null() {
                        sub.insert(entry_key.clone(), entry_value.clone());
                    }
                }
            }
            if let Some(entries) = low_map {
                for entry_key in entries.keys() {
                    let still_present = eff_map
                        .and_then(|map| map.get(entry_key))
                        .is_some_and(|value| !value.is_null());
                    if !still_present {
                        sub.entry(entry_key.clone()).or_insert(Value::Null);
                    }
                }
            }
            if !sub.is_empty() {
                out.insert(key.clone(), Value::Object(sub));
            }
        } else {
            match (eff, low) {
                (Some(value), _) => {
                    out.insert(key.clone(), value.clone());
                }
                (None, Some(_)) => {
                    out.insert(key.clone(), Value::Null);
                }
                (None, None) => {}
            }
        }
    }
    out
}

fn read_one_credentials_root(path: &Path) -> io::Result<Map<String, Value>> {
    match restrict_permissions_owner_only(path)
        .and_then(|()| core_types::paths::read_private_file(path))
    {
        Ok(contents) => {
            let contents = String::from_utf8(contents)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if contents.trim().is_empty() {
                return Ok(Map::new());
            }
            serde_json::from_str::<Value>(&contents)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                .as_object()
                .cloned()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "credentials file must contain a JSON object",
                    )
                })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(error),
    }
}

static CREDENTIAL_TEMP_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn write_credentials_root(path: &Path, root: &Map<String, Value>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_credentials_parent(parent)?;
    }
    let mut rendered = serde_json::to_string_pretty(root)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    rendered.push('\n');

    let temp_path = write_credentials_candidate(path, rendered.as_bytes())?;
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Some(parent) = path.parent() {
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(())
}

/// Write and sync one owner-only candidate before it becomes visible at the
/// public credential path. `create_new` plus a process/counter suffix prevents
/// concurrent writers or a planted fixed temp symlink from sharing the inode.
fn write_credentials_candidate(path: &Path, contents: &[u8]) -> io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "credentials path must have a file name",
        )
    })?;
    for _ in 0..64 {
        let sequence = CREDENTIAL_TEMP_SEQUENCE
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_path = path.with_file_name(format!(
            ".{}.tmp.{}.{sequence}",
            file_name.to_string_lossy(),
            std::process::id(),
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = match options.open(&temp_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        #[cfg(windows)]
        if let Err(error) = restrict_permissions_owner_only(&temp_path) {
            drop(file);
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        let write = std::io::Write::write_all(&mut file, contents)
            .and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = write {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        return Ok(temp_path);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate a unique credentials temp file",
    ))
}

/// Cross-process advisory lock held by the open file descriptor. The lock file
/// stays in place permanently: dropping the descriptor releases the OS lock
/// without an unlink/recreate race or a crash-stale existence lock.
struct CredentialFileLock {
    _file: File,
}

impl CredentialFileLock {
    fn acquire(lock_path: &Path) -> io::Result<Self> {
        let file = open_credentials_lock(lock_path)?;
        for attempt in 0..50 {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(error) => {
                    let error: io::Error = error.into();
                    if error.kind() != io::ErrorKind::WouldBlock {
                        return Err(error);
                    }
                    if attempt < 49 {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "failed to acquire credentials file lock",
        ))
    }
}

fn open_credentials_lock(lock_path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        if !lock_path.exists() {
            match core_types::paths::write_private_file(
                lock_path,
                b"",
                &core_types::paths::ParentDirPolicy::LeaveParent,
            ) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        core_types::paths::open_windows_owner_only_regular_file(lock_path, true)
    }

    #[cfg(not(windows))]
    let mut options = fs::OpenOptions::new();
    #[cfg(not(windows))]
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(not(windows))]
    let file = options.open(lock_path)?;
    #[cfg(not(windows))]
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("credentials lock path is not a regular file: {}", lock_path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(windows))]
    Ok(file)
}

fn base64url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let block = (u32::from(bytes[index]) << 16)
            | (u32::from(bytes[index + 1]) << 8)
            | u32::from(bytes[index + 2]);
        output.push(TABLE[((block >> 18) & 0x3F) as usize] as char);
        output.push(TABLE[((block >> 12) & 0x3F) as usize] as char);
        output.push(TABLE[((block >> 6) & 0x3F) as usize] as char);
        output.push(TABLE[(block & 0x3F) as usize] as char);
        index += 3;
    }
    match bytes.len().saturating_sub(index) {
        1 => {
            let block = u32::from(bytes[index]) << 16;
            output.push(TABLE[((block >> 18) & 0x3F) as usize] as char);
            output.push(TABLE[((block >> 12) & 0x3F) as usize] as char);
        }
        2 => {
            let block = (u32::from(bytes[index]) << 16) | (u32::from(bytes[index + 1]) << 8);
            output.push(TABLE[((block >> 18) & 0x3F) as usize] as char);
            output.push(TABLE[((block >> 12) & 0x3F) as usize] as char);
            output.push(TABLE[((block >> 6) & 0x3F) as usize] as char);
        }
        _ => {}
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialFileLock, GOOGLE_CODE_ASSIST_PROJECT_KEY, GRANT_FINGERPRINT_LEN, OAUTH_KEY, RememberedProject, SavedOAuthProvider, clear_google_code_assist_oauth, clear_oauth_credentials, clear_openai_oauth, codex_auth, credentials_path, grant_fingerprint, load_google_code_assist_oauth, load_google_code_assist_project, load_oauth_credentials, load_openai_compat_api_key, load_openai_oauth, read_one_credentials_root, save_google_code_assist_oauth, save_google_code_assist_project, save_oauth_credentials, save_openai_compat_api_key, save_openai_oauth, saved_oauth_liveness_effective, update_credentials_root, write_credentials_root,
    };
    use core_types::{OAuthTokenSet, OpenAiOAuthTokens};
    use serde_json::Value;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn unique_config_home() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zo-oauth-store-tests-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn no_home_oauth_round_trip_uses_private_fallback() {
        let _env_lock = crate::test_env_lock();
        let prior = [
            (
                core_types::paths::ZO_CONFIG_HOME_ENV,
                std::env::var_os(core_types::paths::ZO_CONFIG_HOME_ENV),
            ),
            (
                core_types::paths::ZO_HOME_ENV,
                std::env::var_os(core_types::paths::ZO_HOME_ENV),
            ),
            ("HOME", std::env::var_os("HOME")),
        ];
        for (key, _) in &prior {
            std::env::remove_var(key);
        }

        let path = credentials_path().expect("secure fallback credentials path");
        let token_set = OAuthTokenSet {
            access_token: "no-home-access".into(),
            refresh_token: Some("no-home-refresh".into()),
            expires_at: Some(9876),
            scopes: vec!["user:inference".into()],
        };
        save_oauth_credentials(&token_set).expect("fallback credentials should save");
        assert_eq!(
            load_oauth_credentials().expect("fallback credentials should load"),
            Some(token_set)
        );
        clear_oauth_credentials().expect("fallback credentials should clear");

        for (key, value) in prior {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        assert!(path.is_absolute());
        assert_eq!(
            path.parent().and_then(|parent| parent.file_name()),
            Some(std::ffi::OsStr::new(".zo"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_code_assist_project_is_remembered_for_its_grant_only() {
        let _env_lock = crate::test_env_lock();
        let config_home = unique_config_home();
        std::env::set_var("ZO_CONFIG_HOME", &config_home);
        let grant = grant_fingerprint("refresh-token-a");
        assert_eq!(grant.len(), GRANT_FINGERPRINT_LEN);
        assert_eq!(
            load_google_code_assist_project(&grant, None).unwrap(),
            RememberedProject::NotRemembered
        );
        save_google_code_assist_project(&grant, None, Some("projects/abc")).unwrap();
        assert_eq!(
            load_google_code_assist_project(&grant, None).unwrap(),
            RememberedProject::Remembered(Some("projects/abc".to_string()))
        );
        // Another login's record is not this login's project.
        assert_eq!(
            load_google_code_assist_project(&grant_fingerprint("refresh-token-b"), None).unwrap(),
            RememberedProject::NotRemembered
        );
        // A free-tier resolution ("no project") is remembered as such, not as "unknown".
        save_google_code_assist_project(&grant, None, None).unwrap();
        assert_eq!(
            load_google_code_assist_project(&grant, None).unwrap(),
            RememberedProject::Remembered(None)
        );
        // A resolution under one configured project answers that configuration
        // only — never an unconfigured process, nor another configured project.
        save_google_code_assist_project(&grant, Some("proj-env"), Some("proj-env")).unwrap();
        assert_eq!(
            load_google_code_assist_project(&grant, Some("proj-env")).unwrap(),
            RememberedProject::Remembered(Some("proj-env".to_string()))
        );
        for other in [None, Some("proj-other")] {
            assert_eq!(
                load_google_code_assist_project(&grant, other).unwrap(),
                RememberedProject::NotRemembered,
                "{other:?}"
            );
        }
        // A record that does not say which configuration it was made under
        // (written before it did) is nobody's.
        update_credentials_root(&credentials_path().unwrap(), |root| {
            root.insert(
                GOOGLE_CODE_ASSIST_PROJECT_KEY.to_owned(),
                serde_json::json!({ "grant": grant, "project": "projects/old" }),
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(
            load_google_code_assist_project(&grant, None).unwrap(),
            RememberedProject::NotRemembered
        );
        save_google_code_assist_project(&grant, None, None).unwrap();
        // Logging the Google account out forgets the project with it.
        save_google_code_assist_oauth(&OAuthTokenSet {
            access_token: "a".into(),
            refresh_token: Some("refresh-token-a".into()),
            expires_at: None,
            scopes: Vec::new(),
        })
        .unwrap();
        clear_google_code_assist_oauth().unwrap();
        assert_eq!(
            load_google_code_assist_project(&grant, None).unwrap(),
            RememberedProject::NotRemembered
        );
        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
    }

    #[test]
    fn oauth_credentials_round_trip_and_clear() {
        let _env_lock = crate::test_env_lock();
        let config_home = unique_config_home();
        std::env::set_var("ZO_CONFIG_HOME", &config_home);

        let token_set = OAuthTokenSet {
            access_token: "access-token".into(),
            refresh_token: Some("refresh-token".into()),
            expires_at: Some(1234),
            scopes: vec!["org:read".into(), "user:write".into()],
        };

        save_oauth_credentials(&token_set).expect("credentials should save");
        assert_eq!(
            load_oauth_credentials().expect("credentials should load"),
            Some(token_set)
        );

        clear_oauth_credentials().expect("credentials should clear");
        assert_eq!(
            load_oauth_credentials().expect("cleared credentials should load"),
            None
        );

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
    }

    #[test]
    fn save_preserves_unrelated_entries_and_clear_is_scoped() {
        let _env_lock = crate::test_env_lock();
        let config_home = unique_config_home();
        let lower_home = config_home.with_extension("lower");
        let user_home = config_home.with_extension("home");
        let _roots = RootEnvGuard::set(&config_home, &lower_home, &user_home);
        let path = credentials_path().expect("credentials path");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, "{\"other\":\"value\"}\n").expect("seed credentials");

        let token_set = OAuthTokenSet {
            access_token: "access-token".into(),
            refresh_token: Some("refresh-token".into()),
            expires_at: Some(123),
            scopes: vec!["scope:a".into()],
        };
        save_oauth_credentials(&token_set).expect("save credentials");
        let saved = fs::read_to_string(&path).expect("read saved");
        assert!(saved.contains("\"other\": \"value\""));
        assert!(saved.contains("\"oauth\""));

        clear_oauth_credentials().expect("clear credentials");
        let cleared = fs::read_to_string(&path).expect("read cleared");
        assert!(cleared.contains("\"other\": \"value\""));
        assert!(!cleared.contains("\"oauth\""));

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
    }

    #[test]
    fn openai_oauth_round_trip_isolated_from_anthropic() {
        let _env_lock = crate::test_env_lock();
        let _codex_home = CodexHomeGuard::clear();
        let config_home = unique_config_home();
        std::env::set_var("ZO_CONFIG_HOME", &config_home);

        let anthropic = OAuthTokenSet {
            access_token: "anthropic-at".into(),
            refresh_token: Some("anthropic-rt".into()),
            expires_at: Some(111),
            scopes: vec!["user:inference".into()],
        };
        save_oauth_credentials(&anthropic).expect("anthropic credentials should save");

        let openai = OpenAiOAuthTokens {
            access_token: "openai-at".into(),
            refresh_token: Some("openai-rt".into()),
            expires_at: Some(222),
            account_id: Some("acc_123".into()),
            scopes: vec!["openid".into()],
        };
        save_openai_oauth(&openai).expect("openai credentials should save");

        assert_eq!(
            load_openai_oauth().expect("openai credentials should load"),
            Some(openai)
        );
        assert_eq!(
            load_oauth_credentials().expect("anthropic credentials should load"),
            Some(anthropic)
        );

        clear_openai_oauth().expect("openai credentials should clear");
        assert_eq!(
            load_openai_oauth().expect("cleared openai credentials should load"),
            None
        );
        assert!(load_oauth_credentials()
            .expect("anthropic credentials should survive openai clear")
            .is_some());

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
    }

    /// zerocode-IDE hands a pane its account through `CODEX_HOME`. That account
    /// is the one zo speaks and bills as — including over a `zo login openai`
    /// already on disk, which is what used to spend one plan while `/status`
    /// (which reads the hand-off) reported the other one's headroom.
    #[test]
    fn the_handed_off_ide_account_outranks_zos_own_login() {
        let _env_lock = crate::test_env_lock();
        let _codex_home = CodexHomeGuard::clear();
        let config_home = unique_config_home();
        std::env::set_var("ZO_CONFIG_HOME", &config_home);

        let own = OpenAiOAuthTokens {
            access_token: "own-at".into(),
            refresh_token: Some("own-rt".into()),
            expires_at: Some(222),
            account_id: Some("own-account".into()),
            scopes: vec!["openid".into()],
        };
        save_openai_oauth(&own).expect("zo's own login should save");
        assert_eq!(
            load_openai_oauth().expect("load without a hand-off"),
            Some(own),
            "with no CODEX_HOME, zo's own login is the only one there is"
        );

        let codex_home = config_home.with_extension("codex");
        fs::create_dir_all(&codex_home).expect("codex home dir");
        fs::write(
            codex_home.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"ide-at","refresh_token":"ide-rt","account_id":"ide-account"}}"#,
        )
        .expect("seed the handed-off login");
        std::env::set_var(codex_auth::CODEX_HOME_ENV, &codex_home);

        let handed_off = load_openai_oauth()
            .expect("load with a hand-off")
            .expect("the hand-off is a login");
        assert_eq!(handed_off.access_token, "ide-at");
        assert_eq!(handed_off.account_id.as_deref(), Some("ide-account"));

        // A rotation of that account goes back to the file it came from, so the
        // Codex CLI beside zo is not stranded on the superseded refresh token.
        save_openai_oauth(&OpenAiOAuthTokens {
            access_token: "ide-at-2".into(),
            refresh_token: Some("ide-rt-2".into()),
            expires_at: None,
            account_id: Some("ide-account".into()),
            scopes: Vec::new(),
        })
        .expect("a rotation of the handed-off account should save");
        let auth: Value =
            serde_json::from_str(&fs::read_to_string(codex_home.join("auth.json")).expect("read"))
                .expect("auth.json json");
        assert_eq!(auth["tokens"]["access_token"], "ide-at-2");
        assert_eq!(
            load_openai_oauth()
                .expect("reload the hand-off")
                .map(|tokens| tokens.access_token),
            Some("ide-at-2".to_string())
        );

        // A fresh login for a DIFFERENT account is zo's own; it must not be
        // written over someone else's login file.
        let replacement = OpenAiOAuthTokens {
            access_token: "own-at-2".into(),
            refresh_token: Some("own-rt-2".into()),
            expires_at: Some(333),
            account_id: Some("own-account".into()),
            scopes: vec!["openid".into()],
        };
        save_openai_oauth(&replacement).expect("a foreign-account login should save");
        let auth: Value =
            serde_json::from_str(&fs::read_to_string(codex_home.join("auth.json")).expect("read"))
                .expect("auth.json json");
        assert_eq!(
            auth["tokens"]["access_token"], "ide-at-2",
            "the handed-off login must survive an unrelated zo login"
        );

        // The hand-off still wins while it is there, and zo's own login is what
        // remains once the pane's account is gone.
        assert_eq!(
            load_openai_oauth()
                .expect("reload with the hand-off present")
                .map(|tokens| tokens.access_token),
            Some("ide-at-2".to_string())
        );
        std::env::remove_var(codex_auth::CODEX_HOME_ENV);
        assert_eq!(
            load_openai_oauth().expect("reload without the hand-off"),
            Some(replacement)
        );

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(codex_home);
        let _ = fs::remove_dir_all(config_home);
    }

    #[test]
    fn an_old_accounts_refresh_cannot_overwrite_a_newly_switched_ide_mirror() {
        let _env_lock = crate::test_env_lock();
        let _codex_home = CodexHomeGuard::clear();
        let config_home = unique_config_home();
        std::env::set_var("ZO_CONFIG_HOME", &config_home);
        let codex_home = config_home.with_extension("codex-switch");
        fs::create_dir_all(&codex_home).expect("codex home");
        std::env::set_var(codex_auth::CODEX_HOME_ENV, &codex_home);
        fs::write(
            codex_home.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"old-at","refresh_token":"old-rt","account_id":"old-account"}}"#,
        )
        .expect("old IDE account");
        let old = load_openai_oauth()
            .expect("load old IDE account")
            .expect("old IDE account tokens");

        fs::write(
            codex_home.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"new-at","refresh_token":"new-rt","account_id":"new-account"}}"#,
        )
        .expect("new IDE account");
        save_openai_oauth(&OpenAiOAuthTokens {
            access_token: "old-at-refreshed".into(),
            refresh_token: Some("old-rt-refreshed".into()),
            expires_at: None,
            account_id: old.account_id,
            scopes: Vec::new(),
        })
        .expect("persist stale account refresh");

        let mirror: Value = serde_json::from_str(
            &fs::read_to_string(codex_home.join("auth.json")).expect("read switched mirror"),
        )
        .expect("mirror json");
        assert_eq!(mirror["tokens"]["account_id"], "new-account");
        assert_eq!(mirror["tokens"]["access_token"], "new-at");

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(codex_home);
    }

    #[test]
    fn openai_compat_api_key_round_trip_preserves_oauth_entries() {
        let _env_lock = crate::test_env_lock();
        let config_home = unique_config_home();
        std::env::set_var("ZO_CONFIG_HOME", &config_home);

        let anthropic = OAuthTokenSet {
            access_token: "anthropic-at".into(),
            refresh_token: Some("anthropic-rt".into()),
            expires_at: Some(111),
            scopes: vec!["user:inference".into()],
        };
        save_oauth_credentials(&anthropic).expect("anthropic credentials should save");

        save_openai_compat_api_key("DEEPSEEK_API_KEY", "sk-deepseek")
            .expect("adapter API key should save");
        assert_eq!(
            load_openai_compat_api_key("DEEPSEEK_API_KEY").expect("adapter key should load"),
            Some("sk-deepseek".to_string())
        );
        assert_eq!(
            load_oauth_credentials().expect("anthropic credentials should survive"),
            Some(anthropic)
        );

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
    }

    /// A presence probe cannot tell a working credential from a dead one, and
    /// reported both as healthy. The liveness probe separates the two without
    /// reading a token value: an expired entry that still carries a refresh token
    /// renews itself on next use, while an expired entry with none can only be
    /// fixed by signing in again.
    #[test]
    fn liveness_separates_a_renewable_credential_from_a_dead_one() {
        let _env_lock = crate::test_env_lock();
        let config_home = unique_config_home();
        fs::create_dir_all(&config_home).expect("config home should exist");
        std::env::set_var("ZO_CONFIG_HOME", &config_home);
        let read_file = |path: &Path| -> io::Result<Option<String>> {
            match fs::read_to_string(path) {
                Ok(contents) => Ok(Some(contents)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        };
        let now = 1_000_000_u64;

        // Renewable: expired access token, refresh token on hand.
        save_oauth_credentials(&OAuthTokenSet {
            access_token: "expired".into(),
            refresh_token: Some("renewable".into()),
            expires_at: Some(now - 1),
            scopes: Vec::new(),
        })
        .expect("save renewable credentials");
        let liveness = saved_oauth_liveness_effective(SavedOAuthProvider::Anthropic, &read_file)
            .expect("probe")
            .expect("present");
        assert!(liveness.expired_at(now));
        assert!(liveness.has_refresh_token);
        assert!(
            !liveness.unusable_at(now),
            "an expired token with a refresh token is routine, not a fault"
        );

        // Dead: expired with nothing to refresh from.
        save_oauth_credentials(&OAuthTokenSet {
            access_token: "expired".into(),
            refresh_token: None,
            expires_at: Some(now - 1),
            scopes: Vec::new(),
        })
        .expect("save dead credentials");
        let liveness = saved_oauth_liveness_effective(SavedOAuthProvider::Anthropic, &read_file)
            .expect("probe")
            .expect("present");
        assert!(liveness.unusable_at(now), "this one needs a new sign-in");

        // A logged-out entry stays absent rather than reporting a fabricated
        // liveness. Written directly: this is the `null` tombstone shape a logout
        // leaves behind, and the probe must read it as "nothing here".
        fs::write(
            config_home.join("credentials.json"),
            format!("{{\"{OAUTH_KEY}\":null}}\n"),
        )
        .expect("write a tombstoned entry");
        assert_eq!(
            saved_oauth_liveness_effective(SavedOAuthProvider::Anthropic, &read_file)
                .expect("probe"),
            None
        );

        clear_oauth_credentials().expect("clear");
        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
    }

    #[test]
    fn load_oauth_credentials_treats_null_entry_as_missing() {
        let _env_lock = crate::test_env_lock();
        let config_home = unique_config_home();
        let credentials_file = config_home.join("credentials.json");
        fs::create_dir_all(&config_home).expect("config home should exist");
        fs::write(&credentials_file, format!("{{\"{OAUTH_KEY}\":null}}\n"))
            .expect("credentials file should write");
        std::env::set_var("ZO_CONFIG_HOME", &config_home);

        assert_eq!(
            load_oauth_credentials().expect("null oauth entry should load"),
            None
        );
        assert_eq!(
            credentials_path().expect("config-home override should drive the credentials path"),
            credentials_file
        );

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = fs::remove_dir_all(config_home);
    }

    fn unique_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zo-oauth-store-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[cfg(unix)]
    #[test]
    fn credential_write_does_not_follow_fixed_temp_symlink() {
        let dir = unique_root("temp-symlink");
        fs::create_dir_all(&dir).expect("credential dir");
        let path = dir.join("credentials.json");
        let temp_path = path.with_extension("json.tmp");
        let victim = dir.join("victim.txt");
        fs::write(&victim, "untouched\n").expect("victim");
        std::os::unix::fs::symlink(&victim, &temp_path).expect("temp symlink");

        let mut root = serde_json::Map::new();
        root.insert(
            OAUTH_KEY.to_string(),
            serde_json::json!({ "accessToken": "must-not-leak" }),
        );
        write_credentials_root(&path, &root).expect("credential write");

        assert_eq!(
            fs::read_to_string(&victim).expect("victim after write"),
            "untouched\n",
            "a planted fixed-temp symlink must never receive credential bytes",
        );
        assert!(
            fs::symlink_metadata(&path)
                .expect("published credentials")
                .file_type()
                .is_file(),
            "the published credential path must be a regular file, not the planted symlink",
        );
        assert_eq!(
            read_one_credentials_root(&path)
                .expect("published root")
                .get(OAUTH_KEY)
                .and_then(|value| value.get("accessToken"))
                .and_then(Value::as_str),
            Some("must-not-leak"),
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn live_credentials_lock_is_not_stolen_when_mtime_is_old() {
        let dir = unique_root("live-old-lock");
        fs::create_dir_all(&dir).expect("credential dir");
        let lock_path = dir.join("credentials.json.lock");
        let held = CredentialFileLock::acquire(&lock_path).expect("first lock");
        fs::OpenOptions::new()
            .write(true)
            .open(&lock_path)
            .expect("open lock for mtime")
            .set_modified(SystemTime::now() - Duration::from_secs(20))
            .expect("backdate live lock");

        let Err(error) = CredentialFileLock::acquire(&lock_path) else {
            panic!("a live holder must not lose its lock because its mtime is old");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);

        drop(held);
        assert!(lock_path.exists(), "the reusable lock inode must remain after release");
        let reacquired = CredentialFileLock::acquire(&lock_path).expect("reacquire persistent lock");
        drop(reacquired);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_credential_updates_preserve_disjoint_keys() {
        use std::sync::{Arc, Barrier};

        let dir = unique_root("concurrent-updates");
        fs::create_dir_all(&dir).expect("credential dir");
        let path = Arc::new(dir.join("credentials.json"));
        let barrier = Arc::new(Barrier::new(2));
        let workers = [("provider-a", "secret-a"), ("provider-b", "secret-b")]
            .into_iter()
            .map(|(key, value)| {
                let path = Arc::clone(&path);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    update_credentials_root(path.as_path(), |root| {
                        root.insert(key.to_string(), Value::String(value.to_string()));
                        Ok(())
                    })
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker
                .join()
                .expect("credential writer thread")
                .expect("credential update");
        }
        let root = read_one_credentials_root(path.as_path()).expect("parse credentials");
        assert_eq!(root.get("provider-a").and_then(Value::as_str), Some("secret-a"));
        assert_eq!(root.get("provider-b").and_then(Value::as_str), Some("secret-b"));
        let _ = fs::remove_dir_all(dir);
    }

    /// Pins the canonical root environment for a test and restores the prior
    /// values on drop. `CODEX_HOME` is pinned *empty*: zerocode-IDE exports one
    /// into every pane it opens, and a handed-off account now outranks zo's own
    /// store, so a test of that store must not inherit the developer's.
    struct RootEnvGuard {
        prior: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl RootEnvGuard {
        fn set(primary: &std::path::Path, lower: &std::path::Path, home: &std::path::Path) -> Self {
            let prior = Self::capture();
            std::env::set_var("ZO_CONFIG_HOME", primary);
            std::env::set_var("ZO_HOME", lower);
            std::env::set_var("HOME", home);
            std::env::remove_var(codex_auth::CODEX_HOME_ENV);
            Self { prior }
        }

        fn home_only(home: &std::path::Path) -> Self {
            let prior = Self::capture();
            std::env::remove_var("ZO_CONFIG_HOME");
            std::env::remove_var("ZO_HOME");
            std::env::set_var("HOME", home);
            std::env::remove_var(codex_auth::CODEX_HOME_ENV);
            Self { prior }
        }

        fn capture() -> Vec<(&'static str, Option<std::ffi::OsString>)> {
            [
                "ZO_CONFIG_HOME",
                "ZO_HOME",
                "HOME",
                codex_auth::CODEX_HOME_ENV,
            ]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect()
        }
    }

    impl Drop for RootEnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.prior {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    /// Clears `CODEX_HOME` for a test that exercises zo's own credential store,
    /// restoring the ambient value on drop. See [`RootEnvGuard`] for why.
    struct CodexHomeGuard(Option<std::ffi::OsString>);

    impl CodexHomeGuard {
        fn clear() -> Self {
            let prior = std::env::var_os(codex_auth::CODEX_HOME_ENV);
            std::env::remove_var(codex_auth::CODEX_HOME_ENV);
            Self(prior)
        }
    }

    impl Drop for CodexHomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var(codex_auth::CODEX_HOME_ENV, value),
                None => std::env::remove_var(codex_auth::CODEX_HOME_ENV),
            }
        }
    }

    #[test]
    fn legacy_forge_credentials_load_but_save_writes_primary_zo_only() {
        let _env_lock = crate::test_env_lock();
        let home = unique_root("legacy-home");
        let legacy = home.join(".forge");
        let primary = home.join(".zo");
        let _roots = RootEnvGuard::home_only(&home);
        fs::create_dir_all(&legacy).expect("legacy credentials dir");
        let legacy_contents = r#"{
  "oauth": {
    "accessToken": "legacy-access",
    "refreshToken": "legacy-refresh",
    "expiresAt": 123,
    "scopes": ["user:inference"]
  }
}
"#;
        fs::write(legacy.join("credentials.json"), legacy_contents)
            .expect("seed legacy credentials");

        let loaded = load_oauth_credentials()
            .expect("load legacy credentials")
            .expect("legacy oauth present");
        assert_eq!(loaded.access_token, "legacy-access");
        assert_eq!(loaded.refresh_token.as_deref(), Some("legacy-refresh"));

        let replacement = OAuthTokenSet {
            access_token: "zo-access".into(),
            refresh_token: Some("zo-refresh".into()),
            expires_at: Some(456),
            scopes: vec!["user:inference".into()],
        };
        save_oauth_credentials(&replacement).expect("save primary credentials");

        assert_eq!(
            credentials_path().expect("primary credentials path"),
            primary.join("credentials.json")
        );
        assert!(primary.join("credentials.json").exists());
        assert_eq!(
            fs::read_to_string(legacy.join("credentials.json"))
                .expect("legacy credentials after save"),
            legacy_contents,
            "legacy credentials must remain read-only"
        );
        assert_eq!(
            load_oauth_credentials().expect("reload primary credentials"),
            Some(replacement)
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn lower_root_logout_stays_cleared_with_primary_tombstones() {
        let _env_lock = crate::test_env_lock();
        let primary = unique_root("primary-a");
        let lower = unique_root("lower-a");
        let home = unique_root("home-a");
        let _roots = RootEnvGuard::set(&primary, &lower, &home);

        fs::create_dir_all(&lower).expect("lower root dir");
        fs::write(
            lower.join("credentials.json"),
            r#"{
  "oauth": {"accessToken":"low-anthropic","refreshToken":"low-anthropic-rt","expiresAt":111,"scopes":["user:inference"]},
  "openai_oauth": {"accessToken":"low-openai","refreshToken":"low-openai-rt","expiresAt":222,"scopes":["openid"]},
  "google_code_assist_oauth": {"accessToken":"low-google","refreshToken":"low-google-rt","expiresAt":333,"scopes":["cloud"]}
}
"#,
        )
        .expect("seed lower credentials");

        // The merged effective view exposes every lower-root credential.
        assert!(load_oauth_credentials().expect("load anthropic").is_some());
        assert!(load_openai_oauth().expect("load openai").is_some());
        assert!(load_google_code_assist_oauth().expect("load google").is_some());

        // Primary-only logout.
        clear_oauth_credentials().expect("clear anthropic");
        clear_openai_oauth().expect("clear openai");
        clear_google_code_assist_oauth().expect("clear google");

        // A fresh read re-merges the untouched lower root; the deletions stick.
        assert!(load_oauth_credentials().expect("reload anthropic").is_none());
        assert!(load_openai_oauth().expect("reload openai").is_none());
        assert!(load_google_code_assist_oauth().expect("reload google").is_none());

        // The primary file records `null` tombstones (not absence), which is
        // what suppresses the still-present lower-root copies.
        let primary_json: Value = serde_json::from_str(
            &fs::read_to_string(primary.join("credentials.json")).expect("primary credentials"),
        )
        .expect("primary json");
        for key in ["oauth", "openai_oauth", "google_code_assist_oauth"] {
            assert_eq!(primary_json.get(key), Some(&Value::Null), "{key} tombstone");
        }

        let _ = fs::remove_dir_all(&primary);
        let _ = fs::remove_dir_all(&lower);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn lower_root_adapter_key_merges_and_tombstones_per_entry() {
        let _env_lock = crate::test_env_lock();
        let primary = unique_root("primary-b");
        let lower = unique_root("lower-b");
        let home = unique_root("home-b");
        let _roots = RootEnvGuard::set(&primary, &lower, &home);

        fs::create_dir_all(&lower).expect("lower dir");
        fs::write(
            lower.join("credentials.json"),
            r#"{"openai_compat_api_keys":{"DEEPSEEK_API_KEY":"sk-low-deepseek","OPENAI_API_KEY":"sk-low-openai"}}
"#,
        )
        .expect("seed lower adapter keys");
        fs::create_dir_all(&primary).expect("primary dir");
        fs::write(
            primary.join("credentials.json"),
            r#"{"openai_compat_api_keys":{"DEEPSEEK_API_KEY":"sk-primary-deepseek","OPENAI_API_KEY":null}}
"#,
        )
        .expect("seed primary adapter keys");

        // The collection map merges per entry: primary overrides one entry and
        // tombstones another, while the untouched lower entry survives.
        assert_eq!(
            load_openai_compat_api_key("DEEPSEEK_API_KEY").expect("deepseek"),
            Some("sk-primary-deepseek".to_string())
        );
        assert_eq!(
            load_openai_compat_api_key("OPENAI_API_KEY").expect("openai tombstoned"),
            None
        );

        // A subsequent primary-only save must not resurrect the tombstoned
        // entry when it rewrites the primary map.
        save_openai_compat_api_key("ANTHROPIC_API_KEY", "sk-new").expect("save new key");
        assert_eq!(
            load_openai_compat_api_key("OPENAI_API_KEY").expect("still tombstoned"),
            None
        );
        assert_eq!(
            load_openai_compat_api_key("DEEPSEEK_API_KEY").expect("deepseek after save"),
            Some("sk-primary-deepseek".to_string())
        );

        let _ = fs::remove_dir_all(&primary);
        let _ = fs::remove_dir_all(&lower);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn mixed_token_fields_are_atomic_across_roots() {
        let _env_lock = crate::test_env_lock();
        let primary = unique_root("primary-c");
        let lower = unique_root("lower-c");
        let home = unique_root("home-c");
        let _roots = RootEnvGuard::set(&primary, &lower, &home);

        fs::create_dir_all(&lower).expect("lower dir");
        fs::write(
            lower.join("credentials.json"),
            r#"{"oauth":{"accessToken":"low-at","refreshToken":"low-rt","expiresAt":1,"scopes":["low"]}}
"#,
        )
        .expect("seed lower oauth");
        fs::create_dir_all(&primary).expect("primary dir");
        fs::write(
            primary.join("credentials.json"),
            r#"{"oauth":{"accessToken":"primary-at","scopes":["primary"]}}
"#,
        )
        .expect("seed primary oauth");

        let loaded = load_oauth_credentials()
            .expect("load merged oauth")
            .expect("oauth present");
        // The primary object wins wholesale; lower-root fields are never
        // combined into it, so the missing refresh token stays missing.
        assert_eq!(loaded.access_token, "primary-at");
        assert_eq!(loaded.refresh_token, None);
        assert_eq!(loaded.expires_at, None);
        assert_eq!(loaded.scopes, vec!["primary".to_string()]);

        let _ = fs::remove_dir_all(&primary);
        let _ = fs::remove_dir_all(&lower);
        let _ = fs::remove_dir_all(&home);
    }
}

/// The Codex CLI's credential file — the second source for ChatGPT tokens.
///
/// Shape (measured off codex-cli 0.149.1): `{ "auth_mode", "OPENAI_API_KEY",
/// "tokens": { "id_token", "access_token", "refresh_token", "account_id" },
/// "last_refresh": <RFC 3339> }`. There is no expiry field; the access token
/// is a JWT and its `exp` claim is the expiry.
pub mod codex_auth {
    use std::io;
    use std::path::{Path, PathBuf};

    use base64::Engine as _;
    use core_types::OpenAiOAuthTokens;
    use serde_json::Value;

    pub use crate::managed_account::{CODEX_AUTH_FILE as AUTH_FILE, CODEX_HOME_ENV};
    use crate::managed_account::CodexHomeSource;

    /// The Codex `auth.json` this pane speaks as, and where it came from.
    ///
    /// The order is [`crate::managed_account::resolve_codex_home`]'s — the
    /// channel's account, the launch `CODEX_HOME`, then the window's own
    /// managed home. Never an ambient grab of `~/.codex`: that file is the
    /// person's CLI login, not the account the window chose, and a bare
    /// terminal user who happens to have the Codex CLI installed must not find
    /// zo speaking as it without being told. `Some` only when the file exists.
    #[must_use]
    pub fn auth_json_path_with_source() -> Option<(PathBuf, CodexHomeSource)> {
        let home = crate::managed_account::resolve_codex_home()?;
        let path = home.path.join(AUTH_FILE);
        path.is_file().then_some((path, home.source))
    }

    /// [`auth_json_path_with_source`] for a caller that only needs the file.
    #[must_use]
    pub fn auth_json_path() -> Option<PathBuf> {
        auth_json_path_with_source().map(|(path, _)| path)
    }

    /// Read the tokens out of one `auth.json`. `Ok(None)` when the file holds
    /// no OAuth tokens (API-key mode, or logged out).
    pub fn load_at(path: &Path) -> io::Result<Option<OpenAiOAuthTokens>> {
        let raw = std::fs::read(path)?;
        let root: Value = serde_json::from_slice(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let Some(tokens) = root.get("tokens") else {
            return Ok(None);
        };
        let Some(access_token) = tokens
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
        else {
            return Ok(None);
        };
        let text = |key: &str| {
            tokens
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        Ok(Some(OpenAiOAuthTokens {
            access_token: access_token.to_string(),
            refresh_token: text("refresh_token"),
            expires_at: jwt_exp_seconds(access_token),
            account_id: text("account_id"),
            scopes: Vec::new(),
        }))
    }

    /// `auth.json` 이 이미 담고 있는 계정 — 비교용. 파싱 실패는 "모른다".
    fn account_id_of(raw: &[u8]) -> Option<String> {
        serde_json::from_slice::<Value>(raw)
            .ok()?
            .get("tokens")?
            .get("account_id")?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    /// Write refreshed tokens back, preserving everything else in the file
    /// (`id_token`, `auth_mode`, …) and stamping `last_refresh` the way the
    /// Codex CLI does.
    pub fn save_at(path: &Path, tokens: &OpenAiOAuthTokens) -> io::Result<()> {
        let raw = std::fs::read(path)?;
        // 이 파일은 **사람의 codex 로그인**이다. 우리가 여기 쓰는 유일한 이유는
        // 우리가 방금 그 로그인을 갱신했기 때문이므로, 계정이 다르면 쓸 일이
        // 없다 — 다른 계정(또는 테스트 픽스처)의 토큰을 얹으면 codex 는 그
        // 파일을 파싱하지 못하고 사람은 다시 로그인해야 한다(2026-08-27 실측).
        if let Some(existing) = account_id_of(&raw) {
            let incoming = tokens.account_id.as_deref().unwrap_or_default();
            if !incoming.is_empty() && incoming != existing {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "refusing to overwrite another account's codex login",
                ));
            }
        }
        let mut root: Value = serde_json::from_slice(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let Some(object) = root.as_object_mut() else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "auth.json is not an object"));
        };
        let entry = object
            .entry("tokens".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let Some(slot) = entry.as_object_mut() else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "auth.json tokens is not an object"));
        };
        slot.insert("access_token".into(), Value::String(tokens.access_token.clone()));
        if let Some(refresh) = &tokens.refresh_token {
            slot.insert("refresh_token".into(), Value::String(refresh.clone()));
        }
        if let Some(account_id) = &tokens.account_id {
            slot.insert("account_id".into(), Value::String(account_id.clone()));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        object.insert("last_refresh".into(), Value::String(rfc3339_utc(now)));
        let payload = serde_json::to_vec_pretty(&root)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        core_types::paths::write_private_file(
            path,
            &payload,
            &core_types::paths::ParentDirPolicy::LeaveParent,
        )
    }

    /// The `exp` claim (Unix seconds) of a JWT, read without verifying — this is
    /// scheduling information, not trust.
    #[must_use]
    pub fn jwt_exp_seconds(token: &str) -> Option<u64> {
        let payload = token.split('.').nth(1)?;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?;
        let claims: Value = serde_json::from_slice(&decoded).ok()?;
        claims.get("exp").and_then(Value::as_u64)
    }

    /// `YYYY-MM-DDTHH:MM:SSZ` for a Unix timestamp — enough for `last_refresh`
    /// without pulling a date crate into the api crate.
    #[must_use]
    pub fn rfc3339_utc(secs: u64) -> String {
        let days = secs / 86_400;
        let rem = secs % 86_400;
        let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
        // Howard Hinnant's civil-from-days.
        let z = i64::try_from(days).unwrap_or(0) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if month <= 2 { year + 1 } else { year };
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    }

    #[cfg(test)]
    mod tests {
        use super::{jwt_exp_seconds, load_at, rfc3339_utc, save_at};
        use base64::Engine as _;
        use core_types::OpenAiOAuthTokens;

        fn fake_jwt(exp: u64) -> String {
            let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let header = engine.encode(br#"{"alg":"none"}"#);
            let payload = engine.encode(format!(r#"{{"exp":{exp},"sub":"x"}}"#));
            format!("{header}.{payload}.sig")
        }

        /// 남의 계정 로그인 파일은 덮지 않는다.
        ///
        /// 2026-08-27 실측: 픽스처 토큰(`account_id: "acct"`)이 사람의 codex
        /// 미러 `auth.json` 을 덮어 패인이 전부 "access token could not be
        /// refreshed" 로 죽었다. 계정이 다르면 쓰기를 거부한다.
        #[test]
        fn save_at_refuses_a_different_account() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("auth.json");
            std::fs::write(
                &path,
                r#"{"auth_mode":"chatgpt","tokens":{"access_token":"a","refresh_token":"r","account_id":"real-account"}}"#,
            )
            .expect("seed");
            let intruder = OpenAiOAuthTokens {
                access_token: "oauth-token".to_string(),
                refresh_token: None,
                expires_at: None,
                account_id: Some("acct".to_string()),
                scopes: Vec::new(),
            };
            let refused = save_at(&path, &intruder).expect_err("a foreign account must be refused");
            assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
            let held: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
            assert_eq!(held["tokens"]["access_token"], "a", "the real login must survive");

            // 같은 계정이면 그대로 쓴다 — 이것이 이 폴백의 본래 일이다.
            let rotated = OpenAiOAuthTokens {
                access_token: "fresh".to_string(),
                refresh_token: Some("r2".to_string()),
                expires_at: None,
                account_id: Some("real-account".to_string()),
                scopes: Vec::new(),
            };
            save_at(&path, &rotated).expect("same account writes");
            let held: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
            assert_eq!(held["tokens"]["access_token"], "fresh");
        }

        #[test]
        fn rfc3339_matches_known_instants() {
            assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
            assert_eq!(rfc3339_utc(1_000_000_000), "2001-09-09T01:46:40Z");
            assert_eq!(rfc3339_utc(1_787_717_906), "2026-08-26T04:18:26Z");
        }

        #[test]
        fn jwt_exp_is_read_from_the_payload_segment() {
            assert_eq!(jwt_exp_seconds(&fake_jwt(1_800_000_000)), Some(1_800_000_000));
            assert_eq!(jwt_exp_seconds("not-a-jwt"), None);
        }

        #[test]
        fn auth_json_round_trip_preserves_foreign_fields() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("auth.json");
            let access = fake_jwt(1_900_000_000);
            std::fs::write(
                &path,
                format!(
                    r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"idt","access_token":"{access}","refresh_token":"rt-old","account_id":"acct-1"}},"last_refresh":"2026-01-01T00:00:00Z"}}"#
                ),
            )
            .expect("write");

            let loaded = load_at(&path).expect("load").expect("tokens");
            assert_eq!(loaded.access_token, access);
            assert_eq!(loaded.refresh_token.as_deref(), Some("rt-old"));
            assert_eq!(loaded.account_id.as_deref(), Some("acct-1"));
            assert_eq!(loaded.expires_at, Some(1_900_000_000));

            let refreshed = OpenAiOAuthTokens {
                access_token: fake_jwt(1_900_003_600),
                refresh_token: Some("rt-new".to_string()),
                expires_at: Some(1_900_003_600),
                account_id: None,
                scopes: Vec::new(),
            };
            save_at(&path, &refreshed).expect("save");
            let root: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
            assert_eq!(root["auth_mode"], "chatgpt");
            assert_eq!(root["tokens"]["id_token"], "idt");
            assert_eq!(root["tokens"]["refresh_token"], "rt-new");
            assert_eq!(root["tokens"]["account_id"], "acct-1", "unchanged when refresh carries none");
            assert_ne!(root["last_refresh"], "2026-01-01T00:00:00Z");
        }

        #[test]
        fn api_key_mode_file_yields_no_tokens() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("auth.json");
            std::fs::write(&path, br#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-x"}"#).expect("write");
            assert!(load_at(&path).expect("load").is_none());
        }
    }
}
