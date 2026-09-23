//! Holding more than one Codex account.
//!
//! The model is in `zerocode-core::codex_account`; what lives here is the
//! machine's half — a managed home per account, the CLI's own login run against
//! one of them, and the file that remembers which is selected.
//!
//! **This window never sees a credential.** Adding an account is `CODEX_HOME=
//! <ours> codex login`: the CLI opens its own browser flow and writes its own
//! `auth.json` into the directory we made. We read one thing back out of it —
//! who logged in — and the access and refresh tokens beside that name are not
//! parsed, not kept, and not sent.
//!
//! Two things here are Codex's own rather than borrowed from the Claude half,
//! and both are measured (Orca 1.4.164 `CodexAccountService`,
//! out/main/index.js:215690-216480):
//!
//! 1. **The login does not exit on its own.** `codex login` runs a local
//!    callback server and keeps running after the browser is done, so Orca
//!    watches `auth.json` for the write and then kills the process tree
//!    (:216355-216400). Waiting for exit alone would hang until the timeout on
//!    every successful login.
//! 2. **A managed home is a home.** Orca seeds the person's own `config.toml`
//!    into it and re-syncs it (`syncCanonicalConfigIntoManagedHome`, :216018) —
//!    the same reason the Claude half carries first-run answers: a directory
//!    with only credentials in it is a CLI that has lost every setting the
//!    person has.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub use zerocode_core::codex_account::AUTH_FILE;
use zerocode_core::codex_account::{
    AuthKind, CodexAccount, CodexIdentity, CodexSelection, duplicate_of, identity_from_auth,
};

// The store's name is `zerocode_core`'s: the window writes this file and a zo
// running outside a pane reads it to follow the chosen account (t-5777).
pub(crate) use zerocode_core::codex_account::STORE_FILE as ACCOUNT_STORE_FILE;
pub(crate) const MANAGED_ACCOUNTS_DIR: &str = "codex-accounts";
pub(crate) const MANAGED_HOME_DIR: &str = "home";
const SYSTEM_HOME_DIR: &str = ".codex";
const RUNTIME_RECORD_FILE: &str = "codex-runtime-auth.json";
const MODEL_CACHE_FILE: &str = "models_cache.json";
const RUNTIME_MODEL_CACHE_VERSION: u8 = 1;

/// What this window last materialized into the shared Codex runtime home.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexRuntimeAuth {
    #[serde(default)]
    pub version: u8,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub written: Option<String>,
}

/// The files a managed home inherits from the person's own Codex home.
///
/// `config.toml` is Orca's canonical config sync; `AGENTS.md` rides with it for
/// the same reason — it is the instruction file the person wrote for their
/// agent, and an account that cannot see it is an agent that forgot how they
/// work. Neither is a credential and neither is per-account.
const INHERITED_FILES: &[&str] = &["config.toml", "AGENTS.md"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexAccountStore {
    #[serde(default)]
    pub accounts: Vec<CodexAccount>,
    #[serde(default, flatten)]
    pub selection: CodexSelection,
}

fn store_file(config_root: &Path) -> PathBuf {
    config_root.join(ACCOUNT_STORE_FILE)
}

/// Where one account's home lives below the injected local-data root:
/// `codex-accounts/<id>/home`.
///
/// The extra `home` level is Orca's layout (:26905) and it earns its place —
/// the account directory can hold our own bookkeeping beside a home that is
/// entirely the CLI's, with no chance of a name of ours colliding with a name
/// of theirs.
pub(crate) fn account_home(local_data_root: &Path, id: &str) -> Option<PathBuf> {
    // Guarded even though the ids are ours: the id reaches a filesystem path,
    // and a value that only happens to be safe today is not a check.
    let safe = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    safe.then(|| {
        local_data_root
            .join(MANAGED_ACCOUNTS_DIR)
            .join(id)
            .join(MANAGED_HOME_DIR)
    })
}

/// The person's own Codex home — the one a `codex` with no `CODEX_HOME` uses.
pub(crate) fn system_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(SYSTEM_HOME_DIR))
}

/// The single shared Codex runtime home every ZeroCode session runs in.
pub fn runtime_home(local_data_root: &Path) -> PathBuf {
    zerocode_hookd::codex_mirror::Home::under(local_data_root)
        .path()
        .to_path_buf()
}

/// Resolve the shared runtime home from config root by discovering local data
/// root from existing accounts or falling back to config root.
pub fn resolve_runtime_home(config_root: &Path) -> PathBuf {
    let store = read_store(config_root);
    if let Some(first) = store.accounts.first()
        && let Some(data_root) = Path::new(&first.home_dir)
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
    {
        return runtime_home(data_root);
    }
    runtime_home(config_root)
}

/// Where the active credentials live for reading usage: the managed home if an
/// account is chosen, or ~/.codex for the system default — and, for a chosen
/// account, the shared runtime home instead when it holds a provably fresher
/// copy of that same login.
///
/// The runtime half is t-6583's Codex check. A chosen account's panes run with
/// `CODEX_HOME` set to the runtime home ([`launch_env`]), so that is where the
/// CLI refreshes — and rotates — its tokens; the account's own home receives
/// the fresher copy only at the next launch's read-back ([`materialize_into`],
/// step 1). A usage read in between asked with the older token. Which copy is
/// the live one is [`runtime_copy_wins`], the rule the materialize already
/// judges by, so the two roads cannot come to disagree. A read, never a write.
pub fn active_home(config_root: &Path) -> Option<PathBuf> {
    let (home, kind) = active_home_in(config_root, system_home())?;
    if kind == CodexHomeKind::Account {
        let runtime = resolve_runtime_home(config_root);
        if let (Ok(live), Ok(stored)) = (
            std::fs::read_to_string(runtime.join(AUTH_FILE)),
            std::fs::read_to_string(home.join(AUTH_FILE)),
        ) && runtime_copy_wins(&live, &stored)
        {
            return Some(runtime);
        }
    }
    Some(home)
}

/// Whose home [`active_home`] chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexHomeKind {
    /// A managed account's own directory.
    Account,
    /// The machine's own `~/.codex`.
    System,
}

/// [`active_home`] with the machine's own home named rather than read, so a
/// test can hand it one that is not the developer's — and saying which of
/// the two it chose, which the readiness evidence names.
pub(crate) fn active_home_in(
    config_root: &Path,
    system: Option<PathBuf>,
) -> Option<(PathBuf, CodexHomeKind)> {
    let store = read_store(config_root);
    zerocode_core::codex_account::active_account(&store.accounts, &store.selection)
        .map(|account| PathBuf::from(&account.home_dir))
        .filter(|home| signed_in(home))
        .map(|home| (home, CodexHomeKind::Account))
        .or_else(|| system.map(|home| (home, CodexHomeKind::System)))
}

pub fn runtime_record_file(runtime_home: &Path) -> PathBuf {
    runtime_home.join(RUNTIME_RECORD_FILE)
}

pub fn read_runtime_record(runtime_home: &Path) -> CodexRuntimeAuth {
    std::fs::read_to_string(runtime_record_file(runtime_home))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn write_runtime_record(runtime_home: &Path, record: &CodexRuntimeAuth) {
    if let Ok(text) = serde_json::to_string_pretty(record) {
        let _ = write_private(&runtime_record_file(runtime_home), &text);
    }
}

fn write_private(target: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::durable_file::replace_bytes(target, text.as_bytes())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn signed_in_content(text: &str) -> bool {
    !matches!(identity_from_auth(text).0, AuthKind::None)
}

/// Replace the shared runtime's model catalogue only when its account changes.
/// A Codex `models_cache.json` can contain account-specific availability, so
/// retaining it across a credential swap makes the picker advertise the
/// account that just left. A missing source cache clears the old one; Codex
/// will recreate it for the newly selected account.
fn sync_runtime_model_cache(source_home: &Path, runtime: &Path) {
    let source = source_home.join(MODEL_CACHE_FILE);
    let target = runtime.join(MODEL_CACHE_FILE);
    match std::fs::read(source) {
        Ok(cache) => {
            let _ = crate::durable_file::replace_bytes(&target, &cache);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = std::fs::remove_file(target);
        }
        Err(_) => {}
    }
}

/// One `auth.json` the way the CLI writes one: an `id_token` whose payload
/// carries the email, the account id beside it, and the `last_refresh`
/// stamp this module's direction rule reads.
#[cfg(test)]
pub(crate) fn auth_fixture(account: &str, refreshed_at: &str, access: &str) -> String {
    // base64url, no padding — the three segments `identity_from_auth`
    // needs before it will call this a login.
    fn segment(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut held = 0u32;
            for (at, byte) in chunk.iter().enumerate() {
                held |= u32::from(*byte) << (16 - 8 * at);
            }
            for at in 0..(chunk.len() * 8).div_ceil(6) {
                out.push(ALPHABET[((held >> (18 - 6 * at)) & 0x3f) as usize] as char);
            }
        }
        out
    }
    let claims = format!(r#"{{"email":"person@example.com","sub":"{account}"}}"#);
    let id_token = format!(
        "{}.{}.{}",
        segment(br#"{"alg":"none"}"#),
        segment(claims.as_bytes()),
        segment(b"signature")
    );
    format!(
        r#"{{"auth_mode":"chatgpt","tokens":{{"id_token":"{id_token}","access_token":"{access}","refresh_token":"rt-{access}","account_id":"{account}"}},"last_refresh":"{refreshed_at}"}}"#
    )
}

/// Does the runtime home's copy beat the stored one?
///
/// Only when it is the SAME login and its `last_refresh` is provably later.
/// The judgement itself is `zerocode_hookd::codex_runtime_auth`'s, which
/// already owns this direction between the mirror and `~/.codex`: ChatGPT
/// rotates refresh tokens, so two homes holding one login have exactly one
/// live branch, and a second copy of the rule is a second answer waiting to
/// disagree (t-5777).
pub(crate) fn runtime_copy_wins(runtime: &str, stored: &str) -> bool {
    zerocode_hookd::codex_runtime_auth::same_identity(runtime, stored)
        && zerocode_hookd::codex_runtime_auth::monotonically_fresher(runtime, stored)
}

/// Write the selected account's credentials into the shared runtime home,
/// preserving sessions across accounts and switching logins in real-time.
pub fn materialize(config_root: &Path, local_data_root: &Path) -> Result<(), String> {
    let runtime = runtime_home(local_data_root);
    materialize_into(config_root, &runtime)
}

pub fn materialize_into(config_root: &Path, runtime: &Path) -> Result<(), String> {
    std::fs::create_dir_all(runtime).map_err(|error| error.to_string())?;
    let store = read_store(config_root);
    let runtime_auth = runtime.join(AUTH_FILE);
    let mut record = read_runtime_record(runtime);

    let current_runtime_content = std::fs::read_to_string(&runtime_auth).ok();

    // 1. Read-back: if the previous account rotated its tokens inside the
    // runtime home, save them back to that account's store before overwriting.
    if let Some(prev_id) = &record.account
        && let Some(curr) = current_runtime_content.as_deref()
        && record.written.as_deref() != Some(curr)
        && signed_in_content(curr)
        && let Some(prev_account) = store.accounts.iter().find(|a| &a.id == prev_id)
    {
        let prev_auth = Path::new(&prev_account.home_dir).join(AUTH_FILE);
        // Only a PROVABLY fresher stored copy stops the write-back. A file
        // Codex stamped later is the live branch of a rotating refresh token,
        // and saving an older one over it logs that account out; anything the
        // stamps cannot decide stays as it was — the runtime home is where the
        // pane just worked.
        let held = std::fs::read_to_string(&prev_auth).ok();
        if !held
            .as_deref()
            .is_some_and(|held| runtime_copy_wins(held, curr))
        {
            let _ = write_private(&prev_auth, curr);
        }
    }

    // 2. Materialize the active account.
    let active = zerocode_core::codex_account::active_account(&store.accounts, &store.selection);
    if let Some(account) = active {
        let switched = record.account.as_deref() != Some(account.id.as_str());
        let account_home = Path::new(&account.home_dir);
        let stored = std::fs::read_to_string(account_home.join(AUTH_FILE))
            .map_err(|_| "계정의 자격 증명을 읽지 못했습니다".to_string())?;
        // The runtime home may hold a NEWER copy of this same login — a pane
        // refreshed inside it a moment ago, and step 1 has just saved that copy
        // into the account's home. Writing the older one back over it would
        // hand the pane a refresh token the endpoint has already rotated away.
        let creds = match current_runtime_content.as_deref() {
            Some(curr) if runtime_copy_wins(curr, &stored) => curr.to_string(),
            _ => stored,
        };

        write_private(&runtime_auth, &creds)?;
        let hash = sha256_hex(creds.as_bytes());
        let _ = write_private(&runtime.join(".zerocode-auth-provenance"), &hash);

        if switched || record.version < RUNTIME_MODEL_CACHE_VERSION {
            sync_runtime_model_cache(account_home, runtime);
        }
        carry_settings_into(runtime);

        record.version = RUNTIME_MODEL_CACHE_VERSION;
        record.account = Some(account.id.clone());
        record.written = Some(creds);
        write_runtime_record(runtime, &record);
    } else {
        // System default: read from ~/.codex if present.
        if let Some(sys_home) = system_home() {
            let sys_auth = sys_home.join(AUTH_FILE);
            if let Ok(stored) = std::fs::read_to_string(&sys_auth)
                && signed_in_content(&stored)
            {
                // Same rule against the machine's own login: a fresher copy in
                // the runtime home stands. Writing it back into `~/.codex` is
                // `zerocode_hookd::codex_runtime_auth`'s job — it holds the
                // provenance that proves the copy started as ours — and it runs
                // on both the launch and the pane-exit road.
                let creds = match current_runtime_content.as_deref() {
                    Some(curr) if runtime_copy_wins(curr, &stored) => curr.to_string(),
                    _ => stored,
                };
                write_private(&runtime_auth, &creds)?;
                let hash = sha256_hex(creds.as_bytes());
                let _ = write_private(&runtime.join(".zerocode-auth-provenance"), &hash);
                if record.account.is_some() || record.version < RUNTIME_MODEL_CACHE_VERSION {
                    sync_runtime_model_cache(&sys_home, runtime);
                }
                carry_settings_into(runtime);
                record.version = RUNTIME_MODEL_CACHE_VERSION;
                record.account = None;
                record.written = Some(creds);
                write_runtime_record(runtime, &record);
                return Ok(());
            }
        }
        let _ = std::fs::remove_file(&runtime_auth);
        let _ = std::fs::remove_file(runtime.join(".zerocode-auth-provenance"));
        record.account = None;
        record.written = None;
        write_runtime_record(runtime, &record);
    }
    Ok(())
}

pub fn read_store(config_root: &Path) -> CodexAccountStore {
    std::fs::read_to_string(store_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn write_store(config_root: &Path, store: &CodexAccountStore) -> Result<(), String> {
    let file = store_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(store).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// Who is signed in inside this home, and how.
pub fn identity_in(home: &Path) -> (AuthKind, CodexIdentity) {
    std::fs::read_to_string(home.join(AUTH_FILE))
        .map(|text| identity_from_auth(&text))
        .unwrap_or((AuthKind::None, CodexIdentity::default()))
}

/// Does this home still hold a login?
///
/// An API-key login counts. It has no email, and a row that called it signed
/// out would be wrong about an account that works.
pub fn signed_in(home: &Path) -> bool {
    !matches!(identity_in(home).0, AuthKind::None)
}

/// The login the machine already had, read live and never written.
///
/// Orca's "system default" (`resolveSystemDefaultIdentity`, :215812): selecting
/// no account is a real choice, and the window has to be able to say whose
/// login that is.
pub fn system_identity() -> Option<(AuthKind, CodexIdentity)> {
    let home = system_home()?;
    let (kind, identity) = identity_in(&home);
    (!matches!(kind, AuthKind::None)).then_some((kind, identity))
}

/// Refuse to seed a managed home from a config that names another provider.
///
/// The person's own config is left exactly as it is — this refuses the login,
/// it does not edit their file. The message names the provider so the way out
/// is obvious, and says which file to look in.
fn refuse_custom_provider_carry() -> Result<(), String> {
    let Some(source) = system_home() else {
        return Ok(());
    };
    let Some(named) = pinned_model_provider(&source.join("config.toml")) else {
        return Ok(());
    };
    Err(format!(
        "`~/{SYSTEM_HOME_DIR}/config.toml` 이 `model_provider = \"{named}\"` 를 지정하고 있습니다. \
         그 설정이 새 계정의 홈으로 함께 옮겨지면 방금 만든 OAuth 자격 증명이 쓰이지 않습니다. \
         그 줄을 지우거나 주석 처리한 뒤 다시 추가해 주세요 — 설정 파일은 건드리지 않았습니다."
    ))
}

/// The top-level `model_provider` a config pins, if it pins one that is not
/// OpenAI's own.
///
/// Read line by line through the shared TOML scanner rather than with a
/// parser: the file belongs to somebody else, and a read that can fail on an
/// unfamiliar construct would refuse logins over a comment. Only the top-level
/// table counts — `[model_providers.x]` naming a provider is a definition, not
/// a selection.
fn pinned_model_provider(config: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config).ok()?;
    let mut scan = zerocode_hookd::toml_lines::Scan::new();
    for line in text.lines() {
        // Read the line against the state it arrived in, THEN advance — the
        // scanner's own stated call order, because a line that opens `"""` is
        // itself still a key assignment.
        let structural = scan.structural();
        scan = scan.advance(line);
        if !structural {
            continue;
        }
        // Past the first table header, a `model_provider =` belongs to that
        // table and not to the document.
        if zerocode_hookd::toml_lines::table_header(line).is_some() {
            return None;
        }
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("model_provider") else {
            continue;
        };
        let rest = rest.trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        let offset = line.len() - rest.len() + 1;
        let value = zerocode_hookd::toml_lines::single_line_string_value(line, offset)?;
        let named = value.value;
        return (!named.is_empty() && named != "openai").then_some(named);
    }
    None
}

/// Copy the person's own settings into a managed home.
///
/// Only the files in [`INHERITED_FILES`], only when the managed home does not
/// already have one, and never a credential — `auth.json` is exactly what must
/// NOT travel, because the whole point of the directory is to hold a different
/// login. Silent on every failure: a config that could not be copied is a
/// Codex with default settings, not a reason to refuse somebody an account.
fn carry_settings_into(home: &Path) {
    let Some(source) = system_home() else { return };
    carry_settings(&source, home);
}

/// The carry itself, from an explicit source directory.
///
/// Separated from the wrapper above so a test can hand it a fixture instead
/// of swapping `HOME` around the call — tests run in parallel threads, other
/// tests read `HOME` (the icon cache resolves through it), and a setenv
/// racing those getenvs is both a flaky suite and, on some platforms, a
/// crash. The environment is read in exactly one place now, and never
/// written.
fn carry_settings(source: &Path, home: &Path) {
    for name in INHERITED_FILES {
        let landing = home.join(name);
        if landing.exists() {
            continue;
        }
        let Ok(text) = std::fs::read(source.join(name)) else {
            continue;
        };
        let _ = std::fs::write(&landing, text);
    }
}

/// The `codex` process every road here starts, aimed at one home.
///
/// The shared CLI login builder (`cli_login::command`) with this provider's
/// home variable: the shell's PATH, this home as `CODEX_HOME` so an ambient
/// one cannot make every account answer for the same person, stdin closed
/// and both pipes taken. Spelled once there rather than here and in every
/// other provider's module.
fn codex_command(program: &str, home: &Path) -> Command {
    crate::cli_login::command(program, zerocode_core::codex_account::HOME_VAR, home)
}

/// Log the machine's own login (`~/.codex`) in again.
///
/// The same login road the managed homes take, on the one home this window
/// otherwise never writes: the CLI owns the browser round trip, and this side
/// only waits for the credential file to change. Nothing is carried and no
/// store is touched — the row that names this login reads it live.
pub fn relogin_system(program: &str) -> Result<(), String> {
    let home = system_home().ok_or("HOME이 설정되어 있지 않습니다")?;
    refuse_pinned_provider_in(&home)?;
    run_login(program, &home)
}

/// Log the machine's own login out, through the CLI's own verb.
///
/// `codex logout` removes the stored credential; we do not delete the file
/// ourselves, because the CLI is the one that knows what else it keeps
/// beside it. The mirror homes follow on their next launch: a system home with
/// no login takes back the copy that was planted there (`codex_runtime_auth`).
pub fn logout_system(program: &str) -> Result<(), String> {
    let home = system_home().ok_or("HOME이 설정되어 있지 않습니다")?;
    let output = codex_command(program, &home)
        .arg("logout")
        .output()
        .map_err(|error| format!("{program}을(를) 실행할 수 없습니다: {error}"))?;
    if output.status.success() || !signed_in(&home) {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if said.is_empty() {
        "로그아웃이 완료되지 않았습니다".to_string()
    } else {
        said
    })
}

/// Run the CLI's own login against a managed home.
///
/// `program` is the command the agent catalogue found — a machine may have
/// `codex` under another name and the catalogue already knows which.
///
/// The loop is the shared runner's (`cli_login::run_login`): the file first,
/// because `codex login` holds a local callback server open and does not
/// exit on its own — waiting for the process would time out on every
/// SUCCESSFUL login; both pipes drained; the CLI given its grace and then
/// taken down. What is Codex's here is only the verb and the witness, and
/// the judgement that ANY change to `auth.json` is the login landing —
/// `identity_in` reads who arrived afterwards, as it always did.
fn run_login(program: &str, home: &Path) -> Result<(), String> {
    let witness = home.join(AUTH_FILE);
    let any_change = |_: &str| true;
    crate::cli_login::run_login(
        program,
        &["login"],
        zerocode_core::codex_account::HOME_VAR,
        home,
        Some(crate::cli_login::Witness::taken(&witness, &any_change)),
    )
}

/// Add an account: make a home, seed it, log in against it, keep it if a real
/// login came back.
///
/// The home is removed again on every failure path. A half-made account would
/// sit in the list and point a launch at a directory with no login in it.
pub fn add_account(
    config_root: &Path,
    local_data_root: &Path,
    program: &str,
    now: i64,
) -> Result<CodexAccountStore, String> {
    let id = new_id(now);
    let home = account_home(local_data_root, &id).ok_or("계정을 저장할 위치를 찾지 못했습니다")?;
    let result = add_into(config_root, program, &id, &home, now);
    if result.is_err() {
        // The account directory, not just the home inside it.
        let _ = std::fs::remove_dir_all(home.parent().unwrap_or(&home));
    } else {
        let _ = materialize(config_root, local_data_root);
    }
    result
}

fn add_into(
    config_root: &Path,
    program: &str,
    id: &str,
    home: &Path,
    now: i64,
) -> Result<CodexAccountStore, String> {
    std::fs::create_dir_all(home).map_err(|error| error.to_string())?;
    // Before a single byte is copied and before the browser opens: a config
    // that pins a custom `model_provider` would be carried into the managed
    // home, and the CLI reads the provider before it reads the login — so the
    // brand-new OAuth credentials would sit in the directory doing nothing,
    // and nothing on screen would say why. Orca refuses at the same point and
    // for the same stated reason (`codex-accounts/service.ts:1390-1403`).
    refuse_custom_provider_carry()?;
    // Before the login rather than after: the CLI reads its config on the way
    // in, and a home seeded afterwards is a first run that already happened.
    carry_settings_into(home);
    run_login(program, home)?;
    let (kind, identity) = identity_in(home);
    if matches!(kind, AuthKind::None) {
        return Err("로그인이 계정 정보를 남기지 않았습니다".into());
    }
    let mut store = read_store(config_root);
    if let Some(held) = duplicate_of(&store.accounts, &identity) {
        return Err(format!("{}은(는) 이미 추가된 계정입니다", held.label()));
    }
    store.accounts.push(CodexAccount {
        id: id.to_string(),
        email: identity.email,
        workspace_label: identity.workspace_label,
        provider_account_id: identity.provider_account_id,
        home_dir: home.to_string_lossy().into_owned(),
        added_at: now,
    });
    // The first account added becomes the selected one. Unlike the Claude half
    // this is a real change of state — with no selection, Codex runs as the
    // machine's own login, and somebody who just added an account meant to use
    // it.
    if store.selection.active.is_none() {
        store.selection.active = Some(id.to_string());
    }
    write_store(config_root, &store)?;
    Ok(store)
}

/// Whether the login in this home is still alive, asked of the CLI.
///
/// The store can only say whether `auth.json` has something in it
/// (`signed_in`), and a token that has DIED leaves that file exactly as it
/// was — so a dead account reads as a healthy row. The Claude half met the
/// same wall and answered it with `claude auth status` (`accounts.rs`,
/// 1-g15); this provider has its own oracle, measured: `codex login status`
/// answers `Not logged in` with exit 1 in a home with no login, and
/// `Logged in …` with exit 0 in one that has one.
///
/// The verdict is the exit status AND the absence of a signed-out phrase,
/// through the shared phrase list rather than a second copy of it
/// (`usage::shows_codex_signed_out`) — the mistake those phrases guard
/// against is the same mistake here.
///
/// Any failure to run counts as "not alive", which is safe because the row is
/// only marked when the STORE also says it has credentials: a broken CLI
/// cannot invent an expiry for an account that never had a login.
///
/// The environment matches `run_login`'s: this home chosen, so an ambient
/// `CODEX_HOME` cannot make every account answer for the same one.
pub fn login_alive(program: &str, home: &Path) -> bool {
    let answered = codex_command(program, home)
        .args(["login", "status"])
        .output();
    let Ok(output) = answered else { return false };
    if !output.status.success() {
        return false;
    }
    let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
    said.push('\n');
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    !crate::usage::shows_codex_signed_out(&said)
}

/// Log in again into the home an account already has.
///
/// The Claude pair of this exists because 지우기 + 계정 추가 threw away the
/// directory, its settings and its place in the list for what is only a dead
/// credential (`accounts.rs::relogin_account`, 1-g15). Codex had no such road
/// at all: the only way out of a dead token was to delete the account, which
/// takes the carried `AGENTS.md` and every setting inside that home with it.
///
/// The same `run_login` the add road uses — one login mechanism, so the PATH
/// hydration and the drained pipes cannot come to differ between adding and
/// repairing.
///
/// Three deliberate differences from `add_into`:
///
/// * `carry_settings_into` is NOT called. This home has already had its first
///   run; carrying again would overwrite whatever the person has changed
///   inside it since, which is the very thing this repair exists to keep.
/// * The refusal reads THIS HOME's config rather than the system one. Nothing
///   is being carried, so the add road's question ("would the carry bring a
///   pinned provider along?") does not apply — but the failure it guards
///   against does: a home that pins its own `model_provider` leaves the fresh
///   OAuth credentials sitting unused, and nothing on screen says why.
/// * No duplicate check. The Claude pair has none either, and a repair that
///   lands on somebody already in the list is a conflict neither half decides
///   today — inventing a rule here alone would make the two roads disagree.
pub fn relogin_account(
    config_root: &Path,
    program: &str,
    id: &str,
) -> Result<CodexAccountStore, String> {
    let mut store = read_store(config_root);
    let held = store
        .accounts
        .iter_mut()
        .find(|account| account.id == id)
        .ok_or("그런 계정이 없습니다")?;
    let home = PathBuf::from(held.home_dir.clone());
    refuse_pinned_provider_in(&home)?;
    run_login(program, &home)?;
    let (kind, identity) = identity_in(&home);
    if matches!(kind, AuthKind::None) {
        return Err("로그인이 계정 정보를 남기지 않았습니다".into());
    }
    // Re-read rather than kept: the CLI asks which account to use and nobody
    // promised it would be the same person as before.
    held.email = identity.email;
    held.workspace_label = identity.workspace_label;
    held.provider_account_id = identity.provider_account_id;
    write_store(config_root, &store)?;
    let runtime = resolve_runtime_home(config_root);
    let _ = materialize_into(config_root, &runtime);
    Ok(store)
}

/// Refuse a login into a home whose own config pins a provider that is not
/// OpenAI's.
///
/// Same failure as `refuse_custom_provider_carry` and the same shared scanner —
/// a different file, because the carry road asks about the source and the
/// repair road asks about the destination.
fn refuse_pinned_provider_in(home: &Path) -> Result<(), String> {
    let Some(named) = pinned_model_provider(&home.join("config.toml")) else {
        return Ok(());
    };
    Err(format!(
        "이 계정의 `config.toml` 이 `model_provider = \"{named}\"` 를 지정하고 있습니다. \
         그 설정이 남아 있으면 다시 로그인해도 새 OAuth 자격 증명이 쓰이지 않습니다. \
         그 줄을 지우거나 주석 처리한 뒤 다시 시도해 주세요 — 설정 파일은 건드리지 않았습니다."
    ))
}

/// An id from the clock plus a counter, so two adds in one millisecond cannot
/// collide. No randomness: this crate has none, and a monotonic suffix is
/// enough for a list a person edits by hand.
fn new_id(now: i64) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("c{now}-{seq}")
}

/// Choose an account, or `None` for the machine's own login.
pub fn select_account(
    config_root: &Path,
    local_data_root: &Path,
    id: Option<&str>,
) -> Result<CodexAccountStore, String> {
    let mut store = read_store(config_root);
    if let Some(id) = id {
        if !store.accounts.iter().any(|account| account.id == id) {
            return Err("그런 계정이 없습니다".into());
        }
        store.selection.active = Some(id.to_string());
    } else {
        // A real choice, not a cleared field: Codex has a system default and
        // going back to it is something a person does on purpose.
        store.selection.active = None;
    }
    write_store(config_root, &store)?;
    materialize(config_root, local_data_root)?;
    Ok(store)
}

/// Remove an account, and its login with it.
pub fn remove_account(
    config_root: &Path,
    local_data_root: &Path,
    id: &str,
) -> Result<CodexAccountStore, String> {
    let mut store = read_store(config_root);
    let Some(at) = store.accounts.iter().position(|account| account.id == id) else {
        return Err("그런 계정이 없습니다".into());
    };
    let gone = store.accounts.remove(at);
    // Only under our own directory, checked rather than trusted: the stored
    // path came off disk, and a path outside is not ours to delete.
    if let Some(mine) = account_home(local_data_root, &gone.id)
        && Path::new(&gone.home_dir) == mine
        && let Some(owned) = mine.parent()
    {
        let _ = std::fs::remove_dir_all(owned);
    }
    if store.selection.active.as_deref() == Some(id) {
        store.selection.active = None;
    }
    write_store(config_root, &store)?;
    materialize(config_root, local_data_root)?;
    Ok(store)
}

/// The environment a Codex launch gets, pointing at the single shared runtime
/// home so conversations and sessions are shared across account switches.
pub fn launch_env(config_root: &Path) -> Vec<(String, String)> {
    let store = read_store(config_root);
    let Some(account) =
        zerocode_core::codex_account::active_account(&store.accounts, &store.selection)
    else {
        return Vec::new();
    };
    let runtime = resolve_runtime_home(config_root);
    let home = Path::new(&account.home_dir);
    if !signed_in(home) {
        return Vec::new();
    }
    carry_settings_into(home);
    let _ = materialize_into(config_root, &runtime);
    vec![(
        zerocode_core::codex_account::HOME_VAR.to_string(),
        runtime.to_string_lossy().into_owned(),
    )]
}

#[cfg(test)]
mod tests {
    /// A config that pins another provider refuses the login BEFORE it runs.
    ///
    /// The carry would put that pin in the managed home, and the CLI reads the
    /// provider before it reads the login — so the new OAuth credentials would
    /// be inert and nothing on screen would say why. The person's own file is
    /// never edited; the refusal says which line and leaves it alone.
    #[test]
    fn a_pinned_custom_provider_is_read_out_of_the_document_only() {
        let directory = tempfile::tempdir().expect("config sandbox");
        let config = directory.path().join("config.toml");
        let read = |text: &str| {
            std::fs::write(&config, text).expect("config");
            pinned_model_provider(&config)
        };

        // The shape the bridge writes, and the one that must refuse.
        assert_eq!(
            read("model = \"claude-opus-5\"\nmodel_provider = \"anthropic-bridge\"\n"),
            Some("anthropic-bridge".to_string())
        );
        // OpenAI's own is not a custom provider, and neither is no pin at all.
        assert_eq!(read("model_provider = \"openai\"\n"), None);
        assert_eq!(read("model = \"gpt-5.6-sol\"\n"), None);
        assert_eq!(read(""), None);

        // A DEFINITION is not a SELECTION: naming a provider inside its own
        // table says it exists, not that it is in use.
        assert_eq!(
            read("[model_providers.bridge]\nmodel_provider = \"bridge\"\n"),
            None
        );
        // And a key that merely starts the same way is somebody else's.
        assert_eq!(read("model_providers = 1\n"), None);

        // A pin inside a multi-line string is text, not a key.
        assert_eq!(
            read("notes = \"\"\"\nmodel_provider = \"bridge\"\n\"\"\"\n"),
            None
        );

        // A file that is not there at all is not a refusal.
        assert_eq!(
            pinned_model_provider(&directory.path().join("gone.toml")),
            None
        );
    }

    use super::*;

    /// A store with one account, selected, whose home holds `held`.
    fn one_selected_account(root: &Path, held: &str) -> (PathBuf, PathBuf) {
        let home = account_home(root, "acct-1").expect("a safe id");
        std::fs::create_dir_all(&home).expect("account home");
        std::fs::write(home.join(AUTH_FILE), held).expect("seed the account home");
        let store = CodexAccountStore {
            accounts: vec![CodexAccount {
                id: "acct-1".to_string(),
                email: Some("person@example.com".to_string()),
                provider_account_id: Some("account-one".to_string()),
                workspace_label: None,
                home_dir: home.to_string_lossy().into_owned(),
                added_at: 1,
            }],
            selection: CodexSelection {
                active: Some("acct-1".to_string()),
            },
        };
        write_store(root, &store).expect("store");
        (home, runtime_home(root))
    }

    /// A usage read asks with the copy the panes refreshed (t-6583).
    ///
    /// A chosen account's panes run in the runtime home and rotate their tokens
    /// there; until the next launch reads that copy back, the account's own
    /// home holds the older one, and a usage read of it asked with a token the
    /// pane had already moved past. The same rule as the materialize decides
    /// which copy is live, and the look writes nothing.
    #[test]
    fn a_usage_read_asks_with_the_copy_the_panes_refreshed() {
        let root = tempfile::tempdir().expect("a data root");
        let older = auth_fixture("account-one", "2026-09-17T02:48:26Z", "at-old");
        let newer = auth_fixture("account-one", "2026-09-21T06:32:33Z", "at-new");
        let (home, runtime) = one_selected_account(root.path(), &older);
        assert_eq!(active_home(root.path()), Some(home.clone()));

        std::fs::create_dir_all(&runtime).expect("runtime home");
        std::fs::write(runtime.join(AUTH_FILE), &newer).expect("the pane refreshed");
        assert_eq!(
            active_home(root.path()),
            Some(runtime.clone()),
            "the usage read asked with the copy the pane had rotated past"
        );

        // An older runtime copy does not win, and neither does another login.
        std::fs::write(runtime.join(AUTH_FILE), &older).expect("a stale runtime");
        std::fs::write(home.join(AUTH_FILE), &newer).expect("the account refreshed");
        assert_eq!(active_home(root.path()), Some(home.clone()));
        let stranger = auth_fixture("account-two", "2026-09-22T00:00:00Z", "at-other");
        std::fs::write(runtime.join(AUTH_FILE), &stranger).expect("another login");
        assert_eq!(active_home(root.path()), Some(home.clone()));
        assert_eq!(
            (
                std::fs::read_to_string(runtime.join(AUTH_FILE)).expect("runtime"),
                std::fs::read_to_string(home.join(AUTH_FILE)).expect("home"),
            ),
            (stranger, newer),
            "a usage look wrote a login"
        );
    }

    /// ChatGPT rotates refresh tokens, so the two homes holding one login have
    /// exactly one live branch. Whichever side refreshed LAST is that branch,
    /// and a materialize that ignored `last_refresh` handed the pane a token
    /// the endpoint had already rotated away — the "logs out every day" the
    /// person reported (t-5777).
    #[test]
    fn the_fresher_last_refresh_wins_in_both_directions() {
        let root = tempfile::tempdir().expect("a data root");
        let older = auth_fixture("account-one", "2026-09-17T02:48:26Z", "at-old");
        let newer = auth_fixture("account-one", "2026-09-21T06:32:33Z", "at-new");

        // The pane refreshed inside the runtime home: that copy reaches the
        // account's own home AND survives the materialize.
        let (home, runtime) = one_selected_account(root.path(), &older);
        std::fs::create_dir_all(&runtime).expect("runtime home");
        std::fs::write(runtime.join(AUTH_FILE), &newer).expect("seed the runtime");
        write_runtime_record(
            &runtime,
            &CodexRuntimeAuth {
                version: RUNTIME_MODEL_CACHE_VERSION,
                account: Some("acct-1".to_string()),
                written: Some(older.clone()),
            },
        );
        materialize_into(root.path(), &runtime).expect("materialize");
        assert_eq!(
            std::fs::read_to_string(home.join(AUTH_FILE)).expect("read the account home"),
            newer,
            "the rotation inside the runtime home never reached the account"
        );
        assert_eq!(
            std::fs::read_to_string(runtime.join(AUTH_FILE)).expect("read the runtime"),
            newer,
            "the older stored copy was written back over a fresher runtime one"
        );

        // The other direction: a `codex login` refreshed the account's own home
        // while the runtime kept an older copy. The account home stands and the
        // runtime follows it.
        std::fs::write(home.join(AUTH_FILE), &newer).expect("the account refreshed");
        std::fs::write(runtime.join(AUTH_FILE), &older).expect("a stale runtime");
        write_runtime_record(
            &runtime,
            &CodexRuntimeAuth {
                version: RUNTIME_MODEL_CACHE_VERSION,
                account: Some("acct-1".to_string()),
                written: Some(newer.clone()),
            },
        );
        materialize_into(root.path(), &runtime).expect("materialize");
        assert_eq!(
            std::fs::read_to_string(home.join(AUTH_FILE)).expect("read the account home"),
            newer,
            "an older runtime copy overwrote the account's newer login"
        );
        assert_eq!(
            std::fs::read_to_string(runtime.join(AUTH_FILE)).expect("read the runtime"),
            newer,
            "the runtime kept a stale login"
        );
    }

    /// Another account's file is not a fresher copy of this one, whatever its
    /// stamp says — the identity gate, checked here because the direction rule
    /// is what would otherwise move tokens between two people's homes.
    #[test]
    fn a_different_account_never_wins_on_its_stamp() {
        let mine = auth_fixture("account-one", "2026-09-17T02:48:26Z", "at-old");
        let theirs = auth_fixture("account-two", "2026-09-21T06:32:33Z", "at-new");
        assert!(!runtime_copy_wins(&theirs, &mine));
        assert!(runtime_copy_wins(
            &auth_fixture("account-one", "2026-09-21T06:32:33Z", "at-new"),
            &mine
        ));
        // No stamp at all is not a proof of freshness.
        assert!(!runtime_copy_wins(
            &auth_fixture("account-one", "", "at-new"),
            &mine
        ));
    }

    #[test]
    fn a_hostile_id_never_becomes_a_path() {
        let root = tempfile::tempdir().expect("no temp dir");
        for hostile in ["..", "../../etc", "a/b", "", "a b", &"x".repeat(65)] {
            assert!(
                account_home(root.path(), hostile).is_none(),
                "`{hostile}` was accepted as a directory name"
            );
        }
        let good = new_id(1_700_000_000_000);
        assert!(account_home(root.path(), &good).is_some());
        // And the home is one level under the account's own directory, so our
        // bookkeeping can never collide with a name the CLI wants.
        assert!(
            account_home(root.path(), &good)
                .unwrap()
                .ends_with(Path::new(&good).join(MANAGED_HOME_DIR))
        );
    }

    #[test]
    fn account_index_and_managed_homes_use_their_injected_roots() {
        let config = tempfile::tempdir().expect("no config dir");
        let local_data = tempfile::tempdir().expect("no local data dir");

        assert_eq!(
            store_file(config.path()),
            config.path().join(ACCOUNT_STORE_FILE)
        );
        assert_eq!(
            account_home(local_data.path(), "c1").expect("safe account id"),
            local_data
                .path()
                .join(MANAGED_ACCOUNTS_DIR)
                .join("c1")
                .join(MANAGED_HOME_DIR)
        );
    }

    #[test]
    fn removal_deletes_only_the_home_owned_by_the_injected_local_data_root() {
        let config = tempfile::tempdir().expect("no config dir");
        let local_data = tempfile::tempdir().expect("no local data dir");
        let external = tempfile::tempdir().expect("no external dir");
        let foreign = external.path().join("foreign-home");
        std::fs::create_dir_all(&foreign).expect("make foreign home");

        let account = |id: &str, home: &Path| CodexAccount {
            id: id.to_string(),
            email: None,
            workspace_label: None,
            provider_account_id: None,
            home_dir: home.to_string_lossy().into_owned(),
            added_at: 1,
        };
        write_store(
            config.path(),
            &CodexAccountStore {
                accounts: vec![account("c-foreign", &foreign)],
                selection: CodexSelection {
                    active: Some("c-foreign".into()),
                },
            },
        )
        .expect("write store");
        remove_account(config.path(), local_data.path(), "c-foreign").expect("remove row");
        assert!(foreign.is_dir(), "an external agent home was deleted");

        let managed = account_home(local_data.path(), "c-managed").expect("managed path");
        std::fs::create_dir_all(&managed).expect("make managed home");
        let owned = managed.parent().expect("account directory").to_path_buf();
        write_store(
            config.path(),
            &CodexAccountStore {
                accounts: vec![account("c-managed", &managed)],
                selection: CodexSelection::default(),
            },
        )
        .expect("write store");
        remove_account(config.path(), local_data.path(), "c-managed").expect("remove account");
        assert!(!owned.exists(), "the app-owned account home was retained");
    }

    #[test]
    fn an_api_key_login_is_a_login_and_a_missing_one_is_not() {
        let dir = tempfile::tempdir().expect("no temp dir");
        assert!(!signed_in(dir.path()));
        std::fs::write(dir.path().join(AUTH_FILE), r#"{"OPENAI_API_KEY":"sk-x"}"#).expect("write");
        assert!(
            signed_in(dir.path()),
            "a key login read as signed out, which is wrong about an account \
             that works"
        );
    }

    #[test]
    fn a_managed_home_inherits_settings_but_never_a_login() {
        let root = tempfile::tempdir().expect("no temp dir");
        let source = root.path().join(SYSTEM_HOME_DIR);
        std::fs::create_dir_all(&source).expect("mkdir");
        std::fs::write(source.join("config.toml"), "model = \"gpt-5\"\n").expect("write");
        std::fs::write(source.join("AGENTS.md"), "# house rules\n").expect("write");
        std::fs::write(source.join(AUTH_FILE), r#"{"OPENAI_API_KEY":"sk-theirs"}"#).expect("write");
        let home = root.path().join("managed");
        std::fs::create_dir_all(&home).expect("mkdir");

        // The fixture is handed in directly. This test used to swap `HOME`
        // around the call and swear no other thread read the environment —
        // which was false: tests run in parallel, the icon cache resolves its
        // path through `HOME` twice in a row, and the swap landing between
        // those two reads was a flaky suite (and setenv racing getenv can
        // crash outright). Injection is the version of this test that cannot
        // lie about its neighbours.
        carry_settings(&source, &home);

        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).expect("config"),
            "model = \"gpt-5\"\n"
        );
        assert!(home.join("AGENTS.md").exists());
        // The one file that must never travel: the whole point of this
        // directory is that it holds a DIFFERENT login.
        assert!(
            !home.join(AUTH_FILE).exists(),
            "a login was copied into a managed home"
        );
    }

    #[test]
    fn materializing_switches_credentials_into_shared_runtime_in_real_time() {
        let config = tempfile::tempdir().expect("no config dir");
        let local_data = tempfile::tempdir().expect("no local data dir");

        let acct1_home = account_home(local_data.path(), "c1").expect("c1 home");
        let acct2_home = account_home(local_data.path(), "c2").expect("c2 home");
        std::fs::create_dir_all(&acct1_home).expect("mkdir c1");
        std::fs::create_dir_all(&acct2_home).expect("mkdir c2");

        let token1 = r#"{"OPENAI_API_KEY":"sk-account-1"}"#;
        let token2 = r#"{"OPENAI_API_KEY":"sk-account-2"}"#;
        std::fs::write(acct1_home.join(AUTH_FILE), token1).expect("write token1");
        std::fs::write(acct2_home.join(AUTH_FILE), token2).expect("write token2");
        let models1 = r#"{"models":[{"slug":"account-1-only"}]}"#;
        let models2 = r#"{"models":[{"slug":"account-2-only"}]}"#;
        std::fs::write(acct1_home.join(MODEL_CACHE_FILE), models1).expect("write models1");
        std::fs::write(acct2_home.join(MODEL_CACHE_FILE), models2).expect("write models2");

        let account = |id: &str, home: &Path| CodexAccount {
            id: id.to_string(),
            email: Some(format!("{id}@example.com")),
            workspace_label: None,
            provider_account_id: None,
            home_dir: home.to_string_lossy().into_owned(),
            added_at: 1,
        };

        write_store(
            config.path(),
            &CodexAccountStore {
                accounts: vec![account("c1", &acct1_home), account("c2", &acct2_home)],
                selection: CodexSelection {
                    active: Some("c1".into()),
                },
            },
        )
        .expect("write store");

        // 1. Initial selection: c1 is active.
        materialize(config.path(), local_data.path()).expect("materialize c1");
        let runtime = runtime_home(local_data.path());
        assert_eq!(
            std::fs::read_to_string(runtime.join(AUTH_FILE)).expect("read runtime auth"),
            token1
        );
        assert_eq!(
            std::fs::read_to_string(runtime.join(MODEL_CACHE_FILE)).expect("read runtime models"),
            models1,
            "the first selected account's model cache was not materialized"
        );

        // 2. Real-time switch to c2:
        select_account(config.path(), local_data.path(), Some("c2")).expect("select c2");
        assert_eq!(
            std::fs::read_to_string(runtime.join(AUTH_FILE)).expect("read runtime auth"),
            token2,
            "switching to c2 did not update runtime auth in real-time"
        );
        assert_eq!(
            std::fs::read_to_string(runtime.join(MODEL_CACHE_FILE)).expect("read runtime models"),
            models2,
            "switching to c2 retained c1's model cache"
        );

        // 3. Launch env points at the single shared runtime home.
        let env = launch_env(config.path());
        assert_eq!(
            env,
            vec![(
                zerocode_core::codex_account::HOME_VAR.to_string(),
                runtime.to_string_lossy().into_owned()
            )],
            "launch_env must point at the shared runtime home"
        );

        // 4. Token rotation read-back: simulate CLI refreshing token in runtime home.
        let refreshed_token2 = r#"{"OPENAI_API_KEY":"sk-account-2-refreshed"}"#;
        std::fs::write(runtime.join(AUTH_FILE), refreshed_token2).expect("write refreshed token2");

        // Switch back to c1: c2's refreshed token must be read back to c2's home!
        select_account(config.path(), local_data.path(), Some("c1")).expect("select c1");
        assert_eq!(
            std::fs::read_to_string(acct2_home.join(AUTH_FILE)).expect("read c2 auth"),
            refreshed_token2,
            "refreshed token was not read back into c2 account store"
        );
        assert_eq!(
            std::fs::read_to_string(runtime.join(AUTH_FILE)).expect("read runtime auth"),
            token1,
            "runtime auth was not updated to c1"
        );
    }
}
