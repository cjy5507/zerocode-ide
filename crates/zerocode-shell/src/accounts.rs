//! Holding more than one Claude account.
//!
//! The model is in `zerocode-core::account`; what lives here is the machine's
//! half — a directory per account, the CLI's own login run against one of them,
//! and the file that remembers which is selected.
//!
//! **This window never sees a credential.** Adding an account is
//! `CLAUDE_CONFIG_DIR=<ours> claude setup-token`-style login: the CLI opens its
//! own browser flow and writes its own credentials into the directory we made.
//! We read exactly one thing back out of that directory — who logged in — and
//! nothing else is ever parsed, sent, or copied. Removing one is deleting a
//! directory.
//!
//! That shape is not a shortcut around an API; it is the only sound way to do
//! this. There is no "log me in" endpoint to call, the browser flow belongs to
//! the CLI, and a window that held tokens of its own would be a window that
//! could leak them.
//!
//! **Switching, though, is not naming a different directory.** It used to be,
//! and that is the whole of a reported defect: the CLI keeps its CONVERSATIONS
//! in the config directory as well as its credentials, so pointing a launch at
//! an account's own directory pointed it at an account's own history. Switching
//! accounts changed the drawer the person's past was read from and their
//! conversations disappeared — "계정을 바꾸면 세션이 공유가 안됨".
//!
//! Orca's useful invariant is that switching is MATERIALIZING — writing the
//! selected account's login into one runtime home (`runtime-auth-service.ts:
//! 421-443`) — rather than naming an account directory. This window keeps that
//! invariant but owns the runtime home too: `<config>/.claude`. Every Zerocode
//! account therefore sees the same conversations, while a `claude` launched in
//! somebody's ordinary terminal keeps using its own `~/.claude` and keychain.
//!
//! [`materialize`] is our half of that, and the invariants it keeps are written
//! on it. They are not caution: each one is a way to log somebody out.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zerocode_core::account::{AccountSelection, ClaudeAccount, ClaudeIdentity, duplicate_of};

/// How long the CLI's login may take. It opens a browser and waits for a human,
/// so this is minutes rather than seconds — and it is bounded anyway, because a
/// login nobody finished must not hold a thread forever.
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
pub(crate) const ACCOUNT_STORE_FILE: &str = "claude-accounts.json";
pub(crate) const MANAGED_ACCOUNTS_DIR: &str = "claude-accounts";
const CREDENTIALS_FILE: &str = ".credentials.json";
const CLAUDE_SETTINGS_FILE: &str = ".claude.json";
const CLAUDE_CONFIG_FILE: &str = ".config.json";
const RUNTIME_AUTH_VERSION: u8 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountStore {
    #[serde(default)]
    pub accounts: Vec<ClaudeAccount>,
    #[serde(default, flatten)]
    pub selection: AccountSelection,
}

fn store_file(config_root: &Path) -> PathBuf {
    config_root.join(ACCOUNT_STORE_FILE)
}

/// Where one account's credentials live. Named after the account's own id, which
/// this window generates — never after the email, which would put an address in
/// a path and change when somebody's address does.
pub(crate) fn account_dir(local_data_root: &Path, id: &str) -> Option<PathBuf> {
    // Guarded even though the ids are ours: the id reaches a filesystem path
    // and a value that only happens to be safe today is not a check.
    let safe = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    safe.then(|| local_data_root.join(MANAGED_ACCOUNTS_DIR).join(id))
}

pub fn read_store(config_root: &Path) -> AccountStore {
    std::fs::read_to_string(store_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_store(config_root: &Path, store: &AccountStore) -> Result<(), String> {
    let file = store_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(store).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// The files inside a config directory that can name who logged in, in Orca's
/// own order (`readCapturedCredentials` :209981 and
/// `readOauthAccountFromConfigDir` :209992, 1.4.164).
///
/// Three rather than one, and the reason is the platform this runs on: **on
/// macOS the CLI puts the token in the keychain**, so `.credentials.json` may
/// never appear (`writeManagedCredentials` :210019 branches on darwin for
/// exactly this). A reader that knew only that file would call a perfectly good
/// login "left no credentials" on every Mac. What the CLI does leave in the
/// directory is its settings file with an `oauthAccount` block in it, which is
/// the half with the email — and the half we want.
fn identity_files(dir: &Path) -> [PathBuf; 3] {
    [
        dir.join(CLAUDE_SETTINGS_FILE),
        dir.join(CREDENTIALS_FILE),
        dir.join(CLAUDE_CONFIG_FILE),
    ]
}

/// Who logged in, read out of the directory the CLI just wrote.
///
/// The ONE thing this window reads from a login. The first file that names
/// somebody wins; a file that is missing, unreadable, or has no account block in
/// it is simply not an answer. The token is deliberately not something we look
/// for — on macOS it is not even here.
fn identity_in(dir: &Path) -> ClaudeIdentity {
    identity_files(dir)
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .map(|text| ClaudeIdentity::from_credentials(&text))
        .find(ClaudeIdentity::is_namable)
        .unwrap_or_default()
}

/// Does this directory still hold a login?
///
/// Asked as "can it name who" rather than "does a file exist", because the file
/// that exists is not the same one on every platform and because a directory
/// the CLI logged OUT of keeps its settings file and loses the account block.
/// Naming somebody is the fact a row is reporting either way.
pub fn signed_in(dir: &Path) -> bool {
    identity_in(dir).is_namable()
}

/* ---- the one home, and putting a login into it ------------------------ */

/// The directory name the CLI keeps everything in, under `$HOME`.
const CLAUDE_HOME_DIR: &str = ".claude";

/// The file an account's own store keeps its OAuth identity block in.
const OAUTH_ACCOUNT_FILE: &str = "oauth-account.json";

/// What this window last put into the runtime home.
///
/// On disk beside the account list, because it has to survive a restart: the
/// question it answers — "are these bytes ours, or did the CLI rotate the token
/// since?" — is meaningless if the answer starts empty every boot.
#[derive(Debug, Default, Serialize, Deserialize)]
struct RuntimeAuth {
    /// Bumped when the evidence required to trust `written` changes.
    #[serde(default)]
    version: u8,
    /// The runtime home this record describes. A record written by the old
    /// global-home implementation is not evidence about the app-owned home.
    #[serde(default)]
    home: Option<String>,
    /// The account whose login is in the runtime home right now.
    #[serde(default)]
    account: Option<String>,
    /// The exact bytes this window last wrote there.
    #[serde(default)]
    written: Option<String>,
    /// Whether the conversations the account directories were holding have been
    /// brought into the one home. Once, and then never again: the walk is cheap
    /// but it is not free, and after it there is nothing left to find.
    #[serde(default)]
    gathered: bool,
}

fn runtime_file(config_root: &Path) -> PathBuf {
    config_root.join("claude-runtime-auth.json")
}

fn read_runtime(config_root: &Path) -> RuntimeAuth {
    std::fs::read_to_string(runtime_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_runtime(config_root: &Path, state: &RuntimeAuth) {
    let Ok(text) = serde_json::to_string_pretty(state) else {
        return;
    };
    // It carries a copy of a credential, so it wears the same clothes as one.
    let _ = write_private(&runtime_file(config_root), &text);
}

/// Force the next launch to compare an account's newly authenticated store
/// with the runtime instead of taking the unchanged-runtime fast path.
fn invalidate_materialized_account(config_root: &Path, id: &str) {
    let mut state = read_runtime(config_root);
    if state.account.as_deref() != Some(id) {
        return;
    }
    state.account = None;
    state.written = None;
    write_runtime(config_root, &state);
}

/// The one Claude Code home every Zerocode account runs in.
///
/// It is below the app's injected config root, so it is shared by this window's
/// accounts and independent of both `~/.claude` and an ambient
/// `CLAUDE_CONFIG_DIR` belonging to another terminal.
pub(crate) fn runtime_home(config_root: &Path) -> PathBuf {
    config_root.join(CLAUDE_HOME_DIR)
}

/// The home an ordinary terminal used before Zerocode became isolated.
///
/// Read only for the one-time conversation copy. Credentials are never read
/// from or written to this path by the app-owned runtime.
#[cfg(not(test))]
fn external_runtime_home() -> Option<PathBuf> {
    if let Ok(set) = std::env::var(zerocode_core::account::CONFIG_DIR_VAR)
        && !set.trim().is_empty()
    {
        return Some(PathBuf::from(set.trim()));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(CLAUDE_HOME_DIR))
}

#[cfg(test)]
fn external_runtime_home() -> Option<PathBuf> {
    // A suite may exercise materialization hundreds of times in parallel. It
    // must never copy a developer's real conversation store into its fixtures.
    None
}

/// Write a file only its owner can read.
///
/// Through `durable_file`, which already carries the platform difference this
/// would otherwise have to invent: `0600` is a unix answer and Windows has its
/// own.
fn write_private(target: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::durable_file::replace_bytes(target, text.as_bytes())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The `oauthAccount` block of a settings file, when it has one.
fn oauth_block(settings: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(settings).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    parsed.get("oauthAccount").cloned()
}

/// Put an `oauthAccount` block into a settings file, or take it out.
///
/// Refuses a file that is not a JSON object, which is the original's own rule
/// (`runtime-auth-service.ts:1578-1581` — `existing === null` answers `false`
/// and nothing is written). Merging into a file we do not understand is how a
/// settings file gets destroyed.
fn set_oauth_block(settings: &Path, block: Option<&serde_json::Value>) {
    let mut held: serde_json::Value = match std::fs::read_to_string(settings) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return,
        },
        // No settings file yet is not a file we misread — the CLI makes one on
        // first run, and starting it with the identity in it is right.
        Err(_) => serde_json::json!({}),
    };
    let Some(object) = held.as_object_mut() else {
        return;
    };
    match block {
        Some(value) => {
            if object.get("oauthAccount") == Some(value) {
                return;
            }
            object.insert("oauthAccount".to_string(), value.clone());
        }
        None => {
            if object.remove("oauthAccount").is_none() {
                return;
            }
        }
    }
    if let Ok(text) = serde_json::to_string_pretty(&held) {
        let _ = write_private(settings, &text);
    }
}

/// Carry only the first-run answers the selected account has already made into
/// the shared runtime. Credentials still switch; theme/onboarding do not need
/// to be answered again for every home this app creates.
fn carry_first_run_into(settings: &Path, store_settings: &Path) {
    let Ok(source_text) = std::fs::read_to_string(store_settings) else {
        return;
    };
    let Ok(source) = serde_json::from_str::<serde_json::Value>(&source_text) else {
        return;
    };
    let mut runtime = match std::fs::read_to_string(settings) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return,
        },
        Err(_) => serde_json::json!({}),
    };
    if !zerocode_core::account::carry_first_run(&mut runtime, &source) {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(&runtime) {
        let _ = write_private(settings, &text);
    }
}

/// The macOS keychain services one config directory answers to.
///
/// Claude Code 2.1+ scopes its entry by the config directory, as the first
/// eight hex characters of `sha256(dir)`. The unsuffixed service is deliberately
/// excluded: it belongs to an ordinary terminal, and writing it would couple
/// that terminal's login to Zerocode's picker.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn keychain_services(home: &Path) -> Vec<String> {
    use sha2::{Digest, Sha256};
    const ACTIVE: &str = "Claude Code-credentials";
    let mut hasher = Sha256::new();
    hasher.update(home.to_string_lossy().as_bytes());
    let suffix = format!("{:x}", hasher.finalize());
    let scoped = format!("{ACTIVE}-{}", &suffix[..8]);
    vec![scoped]
}

/// Who the keychain entry is filed under: `$USER` the way the original reads it
/// (`keychain.ts:80-82`), then `$USERNAME`, then this uid's own account — and
/// nothing at all when none of the three answers.
///
/// The environment is asked first because these items are the CLI's, and the
/// CLI files them under `$USER`. A uid consulted ahead of it would address a
/// different account than the one the item is actually under.
///
/// The chain ends in `None`, never in a name. The version before this one
/// returned the literal `"user"` once both variables were absent, and that is a
/// real execution environment: MEASURED 2026-09-20 (w-4837), an exec'd child of
/// the window had no `USER` at all, `LOGNAME=root`, and `joe` as the uid's own
/// account. The window then asked the keychain about an account nobody has, was
/// told there is no such item, and reported that the person kept no key while
/// the key sat right there. A made-up name turns "I do not know" into a
/// confident wrong answer about somebody else's keychain; `None` keeps it a
/// question about ours.
///
/// `LOGNAME` is deliberately NOT in the chain. It is the variable that DID
/// answer in that environment, and what it answered was the wrong account. A
/// third guess is not a third chance at the truth.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn keychain_user() -> Option<String> {
    keychain_user_from(|name| std::env::var(name).ok(), account_of_this_uid)
}

/// The chain with both of its sources handed in.
///
/// Separate from [`keychain_user`] so the environment that names nobody can be
/// read in a test without `remove_var`, which is process-global and would run
/// underneath every other test in this binary.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn keychain_user_from(
    environment: impl Fn(&str) -> Option<String>,
    uid_account: impl FnOnce() -> Option<String>,
) -> Option<String> {
    ["USER", "USERNAME"]
        .into_iter()
        .filter_map(environment)
        .find(|name| !name.is_empty())
        .or_else(|| uid_account().filter(|name| !name.is_empty()))
}

/// This process's own account, as the password database names it — the one
/// source of the answer an execution environment cannot leave blank.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn account_of_this_uid() -> Option<String> {
    #[cfg(unix)]
    {
        // `getpwuid_r` rather than `getpwuid`: the plain call answers out of a
        // buffer libc keeps for the whole process, and keychain items are read
        // from whichever thread wants one. The EFFECTIVE uid, because
        // `security` inherits it and opens that account's login keychain — the
        // name written on the item and the keychain it lands in have to be one
        // person.
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::zeroed();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let mut room = vec![0_u8; PASSWD_ENTRY_BYTES];
        // SAFETY: both out-parameters belong to this frame, and the buffer is
        // passed with its own length.
        let code = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                entry.as_mut_ptr(),
                room.as_mut_ptr().cast(),
                room.len(),
                &raw mut found,
            )
        };
        if code != 0 || found.is_null() {
            return None;
        }
        // SAFETY: `found` points at `entry`, which the call above filled in.
        let name = unsafe { (*found).pw_name };
        if name.is_null() {
            return None;
        }
        // SAFETY: `pw_name` is libc's own NUL-terminated string inside `room`,
        // which outlives the copy this line makes of it.
        let name = unsafe { std::ffi::CStr::from_ptr(name) };
        name.to_str().ok().map(str::to_owned)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// How much room one password-database entry is given. A system whose entry
/// does not fit answers `ERANGE`, which this road reads as "we do not know" —
/// the honest answer, and the one this function exists to keep available.
#[cfg(unix)]
const PASSWD_ENTRY_BYTES: usize = 4096;

/// The word every keychain door refuses in when it cannot name the account to
/// ask about. One spelling, so a log, a pane and a readiness line agree — and a
/// token rather than prose, because it points the reader at this machine's
/// execution environment rather than at their own keychain.
///
/// Emphatically not `no_key`: that word is a claim about the PERSON's keychain,
/// and this refusal is a statement about ours.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const UNKNOWN_USER: &str = "unknown_user";

/// That refusal as the sentence a caller hands upward.
///
/// It must not carry [`NO_SUCH_ITEM`]: `read_keychain_service_if_present` folds
/// that word into `Ok(None)`, and `Ok(None)` is read all the way up the router
/// and Jev roads as "this person has no key" — the very lie this road exists to
/// stop repeating one level higher.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn unknown_user_refusal() -> String {
    format!(
        "{UNKNOWN_USER}: 이 실행 환경이 키체인 계정 이름을 말하지 않습니다 — USER·USERNAME 이 비어 있고 uid 도 계정을 답하지 않았습니다. 키가 아니라 환경의 문제입니다"
    )
}

/// The account a keychain door files its item under, or the refusal that says
/// nobody told us.
///
/// Takes the answer instead of fetching it, so the refusal can be read on a
/// machine that does know who it is.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn keychain_account(named: Option<String>) -> Result<String, String> {
    named.ok_or_else(unknown_user_refusal)
}

/// The one key a Claude credentials object is known by.
const CLAUDE_OAUTH_KEY: &str = "claudeAiOauth";

/// The fields inside it that a login is a login BY. Asked only ever whether
/// they are empty — see `holds_login`.
const CLAUDE_SECRET_FIELDS: [&str; 2] = ["accessToken", "refreshToken"];

/// Whether these bytes are a login, or the shape a logged-OUT CLI leaves behind.
///
/// The distinction is the whole of a defect that reached this machine. A Claude
/// CLI that is not signed in writes a complete, well-formed credentials document
/// — seven fields, a real `expiresAt`, a real `subscriptionType` — with its two
/// secret fields set to the empty string. Judged at the object, as this was, that
/// document is indistinguishable from a login: the object is present and not
/// empty. So the window treated a logged-out file as credentials, and invariant 1
/// dutifully "preserved" it INTO an account's own store, on top of the token that
/// was there. Logging in inside the app then never stuck, because the next
/// trigger copied the empty document back over it.
///
/// The previous version of this refused to name the secret fields at all, and
/// that instinct was right about one thing and wrong about the cost. It came from
/// a real error — asking the token to NAME somebody, which no keychain payload
/// can do, and which refused every login on this Mac four times in a row. The
/// correction over-swung: never mentioning the field produced a test that
/// accepts logged-out documents and overwrites real ones, which is a worse
/// outcome than the thing being avoided.
///
/// So the fields are named, and the only question ever asked of them is whether
/// they are empty. The value is never bound, returned, formatted, or compared to
/// anything — emptiness is not the secret, and the gate in `main.rs` holds that
/// line precisely rather than by refusing the word.
///
/// Namability stays as the second answer, for file-shaped credentials that carry
/// an address; a keychain payload has none.
fn holds_login(text: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    let signed_in = parsed
        .get(CLAUDE_OAUTH_KEY)
        .and_then(serde_json::Value::as_object)
        .is_some_and(|held| {
            CLAUDE_SECRET_FIELDS.iter().any(|field| {
                held.get(*field)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|held| !held.is_empty())
            })
        });
    signed_in || ClaudeIdentity::from_credentials(text).is_namable()
}

/// The login a directory stands for, out of wherever the CLI actually put it.
///
/// **On macOS it is not in the directory.** The CLI writes its token to the
/// keychain, scoped to the config dir the login ran with, and leaves no
/// `.credentials.json` at all — which the reader above this one has said in its
/// own words since the day it was written (`identity_files`). A materialization
/// that only knew the file worked on no Mac.
fn credentials_at(dir: &Path) -> Option<String> {
    login_at(dir).0
}

/// The same read, plus WHICH HALF answered.
///
/// The two are not interchangeable on macOS, and collapsing them is a live
/// injury rather than a tidiness question. The CLI reads the keychain there, so
/// a home whose file is perfect and whose keychain holds a signed-out document
/// looks materialized to us and opens `Please run /login` for the person. That
/// state was on this machine at 17:10 — the boot said so in its own words —
/// and the only reason it healed is that another clause of the skip happened
/// to be false that time.
fn login_at(dir: &Path) -> (Option<String>, KeychainSays) {
    let says = keychain_says(dir);
    // On macOS the keychain is canonical. A stale fallback file may carry only
    // an access token while the keychain also holds the refresh token, expiry,
    // scopes and tier. Treating that shorter file as a complete login is how a
    // selected account reached the first-run screen while `claude auth status`
    // still succeeded in the account's own directory.
    if let Some(text) = says.login() {
        return (Some(text.to_string()), says);
    }
    let on_file = std::fs::read_to_string(dir.join(CREDENTIALS_FILE))
        .ok()
        .filter(|text| holds_login(text));
    (on_file, says)
}

/// Prepare the selected secure store before a background CLI can read it.
pub fn prepare_selected_store(config_root: &Path) -> Result<(), String> {
    let store = read_store(config_root);
    if let Some(account) = zerocode_core::active_account(&store.accounts, &store.selection) {
        require_unchanged_identity(config_root, account)?;
        seed_scoped_keychain(Path::new(&account.config_dir))?;
    }
    Ok(())
}

/// Upgrade a file-era store before a CLI can fall back to the global login.
/// A refused read or an explicit logout is never permission to replace it.
pub(crate) fn seed_scoped_keychain(store: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let Some(credentials) = std::fs::read_to_string(store.join(CREDENTIALS_FILE))
            .ok()
            .filter(|text| holds_login(text))
        else {
            return Ok(());
        };
        seed_scoped_keychain_if_missing(store, &credentials, &keychain_says(store))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = store;
    Ok(())
}

#[cfg(target_os = "macos")]
fn seed_scoped_keychain_if_missing(
    store: &Path,
    credentials: &str,
    says: &KeychainSays,
) -> Result<(), String> {
    if matches!(says, KeychainSays::Missing | KeychainSays::Damaged) {
        write_keychain(store, credentials)?;
    }
    Ok(())
}

/// What the keychain answered about one directory.
///
/// Five answers because there were five repairs behind one word, and the file
/// has said so since it was written: "a refusal SAYS WHICH DOOR IT WAS". What
/// this adds is the half that matters to a CALLER rather than to a reader of
/// logs — whether the answer is EVIDENCE. "It holds something that is not a
/// login" and "I could not ask" are opposite facts, and treating the second as
/// the first would put a keychain write on every launch, which is the dialog
/// storm this road already paid for once ("키체인이 계속").
pub(crate) enum KeychainSays {
    /// It holds this login.
    Login(String),
    /// It answered, and what it holds is not a login. A signed-out CLI writes a
    /// complete document with empty token fields, which is this.
    NotALogin,
    /// It answered with bytes that are not a document at all. No CLI state
    /// looks like this; it is a broken write (the 128-byte prompt cut-off of
    /// 09-06..09-10 left exactly this), so a seed may replace it.
    Damaged,
    /// It answered that there is nothing there. Our own write is a delete
    /// followed by a create, so a process that dies between the two leaves
    /// exactly this.
    Missing,
    /// Nothing was learned: refused, locked, slower than the bound — or a
    /// platform with no keychain at all, where the file is the whole story.
    NoAnswer,
    /// There was nobody to ask ABOUT: `$USER`, `$USERNAME` and this uid's own
    /// account all declined to name anybody, so there is no account to put
    /// after `-a`.
    ///
    /// The fifth answer, and the one this window got wrong for a day. It used
    /// to invent the name `"user"`, ask about a keychain nobody has, and report
    /// the empty answer as the person keeping no key (t-5419). It is not
    /// `Missing`: an item we never addressed is not an item that is not there.
    UnknownUser,
}

impl KeychainSays {
    /// The answer as one word for a readiness evidence line — the kind and
    /// never the secret.
    pub(crate) const fn word(&self) -> &'static str {
        match self {
            Self::Login(_) => "login",
            Self::NotALogin => "signed-out document",
            Self::Damaged => "damaged item",
            Self::Missing => "no item",
            Self::NoAnswer => "no answer",
            Self::UnknownUser => UNKNOWN_USER,
        }
    }

    /// Whether the item is the complete-but-empty document a signed-out CLI
    /// writes — the one keychain answer that accuses a login on its own. A
    /// missing or damaged item is re-seeded from the file at launch, and an
    /// unanswered question is not a verdict.
    pub(crate) const fn is_signed_out_document(&self) -> bool {
        matches!(self, Self::NotALogin)
    }

    const fn login(&self) -> Option<&str> {
        match self {
            Self::Login(text) => Some(text.as_str()),
            _ => None,
        }
    }

    /// Whether this answer contradicts a home we believe is materialized.
    ///
    /// `NoAnswer` deliberately does not: an unanswered question is not a wrong
    /// answer, and a skip refused on silence is a write on every launch.
    const fn contradicts_a_login(&self) -> bool {
        matches!(self, Self::NotALogin | Self::Missing | Self::Damaged)
    }
}

/// The credentials the keychain holds for one config directory.
///
/// The SCOPED service only, never the unsuffixed one: that name is whichever
/// account is active right now, and reading it while asking about a particular
/// directory is how one account's token gets filed under another's name.
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub(crate) fn keychain_says(dir: &Path) -> KeychainSays {
    #[cfg(target_os = "macos")]
    {
        let Some(service) = keychain_services(dir).into_iter().next() else {
            return KeychainSays::NoAnswer;
        };
        let Some(user) = keychain_user() else {
            return KeychainSays::UnknownUser;
        };
        // A refusal SAYS WHICH DOOR IT WAS. The version of this that swallowed
        // the reason cost a whole morning: the person was told "이 계정의
        // 자격증명을 읽지 못했습니다" thirty-four times, and the sentence was
        // equally true of a wrong service name, a wrong account name, a locked
        // keychain, and a login that genuinely is not there — four different
        // repairs behind one word. The name and the kind are not the secret.
        match security_command(
            &["find-generic-password", "-s", &service, "-a", &user, "-w"],
            None,
        ) {
            Ok(text) => {
                if holds_login(&text) {
                    return KeychainSays::Login(text);
                }
                if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                    eprintln!(
                        "zerocode-shell: 키체인 {service}/{user}이(가) 문서가 아닌 것을 들고 있습니다({}바이트) — 깨진 쓰기, 다시 심습니다",
                        text.len()
                    );
                    return KeychainSays::Damaged;
                }
                eprintln!(
                    "zerocode-shell: 키체인 {service}/{user}이(가) 로그인이 아닌 것을 들고 있습니다"
                );
                KeychainSays::NotALogin
            }
            // The word both the tool and the suite's double use for it, so the
            // two answers cannot come to differ between a Mac and a test.
            Err(error) if error.contains(NO_SUCH_ITEM) => {
                eprintln!("zerocode-shell: 키체인 {service}/{user}에 항목이 없습니다");
                KeychainSays::Missing
            }
            Err(error) => {
                eprintln!("zerocode-shell: 키체인 {service}/{user} 읽기 거절: {error}");
                KeychainSays::NoAnswer
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        KeychainSays::NoAnswer
    }
}

/// `security`'s own words for "there is no such item", shared by the tool's
/// exit-code translation and by the suite's double so a classification cannot
/// mean one thing on a Mac and another in a test.
#[cfg(target_os = "macos")]
const NO_SUCH_ITEM: &str = "항목 없음";

/// `security`'s exit 45 — `errSecDuplicateItem`: `add-generic-password` met an
/// item already standing under the same service and account. Shared with the
/// double for `NO_SUCH_ITEM`'s reason. Seen live 2026-09-13 ("security 종료
/// 45"): two panes resumed one conversation within 300 ms of each other, both
/// ran the delete-then-add in `write_keychain_service`, and the second's add
/// found the first's item.
#[cfg(target_os = "macos")]
const DUPLICATE_ITEM: &str = "이미 있음";

/// One keychain writer at a time in this process. The write is two `security`
/// runs — delete, then add — and two launches interleaving them is how one
/// launch came to be refused as a duplicate. The lock lines this process's
/// writers up; the retry inside the writer covers a writer it cannot see.
#[cfg(target_os = "macos")]
static KEYCHAIN_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// How long `/usr/bin/security` may take, and the reason there is a bound at
/// all.
///
/// A keychain item whose access list does not name the caller does not FAIL —
/// it puts a password dialog on the person's screen and waits, forever, for an
/// answer. An unbounded wait would park a launch behind a window nobody asked
/// for. The original bounds the same call the same way and kills the child on
/// the way out (`keychain.ts:6`, `:190-196`); killing it is what takes the
/// dialog down again.
#[cfg(all(target_os = "macos", not(test)))]
const KEYCHAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// How much of a password `security add-generic-password -w` keeps when the
/// value is typed at its prompt instead of passed as the argument — measured
/// 2026-09-10 (a 562-byte value came back as 128). The shipped writer never
/// takes that road; the fake tool models it so the round-trip test stays red
/// on the prompt and green on the argument.
#[cfg(test)]
const SECURITY_PROMPT_BYTES: usize = 128;

/// Every keychain call goes through Apple's own tool, and the reason is the
/// ACCESS LIST rather than convenience.
///
/// A macOS keychain item remembers which binaries may read it. Writing one
/// in-process put THIS BINARY on that list — and this app is adhoc-signed, so
/// its identity changes with every build. The item written by yesterday's build
/// belonged to a stranger today, and the CLI that had been reading its own login
/// for months started asking the person for a password on every launch
/// ("키체인이 계속"). Going through `/usr/bin/security` puts a stable system
/// binary on the list instead of a name that is different every time we compile.
///
/// `security` does not read a piped stdin for `-w`: leaving the value off opens
/// an interactive terminal prompt and parks this call until the timeout. The
/// password therefore follows `-w`, matching Orca (`keychain.ts:135`). That
/// briefly exposes it in this short-lived process's argv; the three-second bound
/// above limits that window and is preferable to an authentication dialog that
/// can never consume the bytes we sent it.
///
/// **Under test this never runs.** The suite exercises the whole switch, and a
/// switch reaches the keychain — so a version of this without the guard below
/// wrote fixture credentials into the developer's own login keychain and left
/// the junk items behind. It cost this machine's user their Claude session
/// mid-conversation. The store this replaced was memory-backed under `cfg(test)`
/// for exactly this reason; the guard keeps that property rather than trusting
/// every future test to remember.
#[cfg(all(target_os = "macos", not(test)))]
fn security_command(args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    use std::io::{Read, Write};

    let mut child = crate::proc::quiet_command("/usr/bin/security")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    if let Some(secret) = stdin
        && let Some(pipe) = child.stdin.as_mut()
    {
        // Twice, because that is what the tool asks for when `-w` carries no
        // value. Small enough never to fill a pipe buffer, which is why this can
        // be written inline rather than from a thread.
        let _ = pipe.write_all(format!("{secret}\n{secret}\n").as_bytes());
    }
    // Closed either way: a `security` that is waiting on a pipe nobody will
    // write to waits until the deadline and looks like a hung keychain.
    drop(child.stdin.take());
    let deadline = std::time::Instant::now() + KEYCHAIN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(pipe) = child.stdout.as_mut() {
                    let _ = pipe.read_to_string(&mut out);
                }
                if status.success() {
                    return Ok(out.trim_end_matches(['\n', '\r']).to_string());
                }
                // The tool's own sentence rides with the code. "종료 45" alone
                // left 2026-09-13's refusal undiagnosable — a duplicate, a
                // refused delete or a locked keychain all exit with a number.
                // stderr never carries the secret: `security` names the call
                // it made and the OSStatus it got.
                let mut said = String::new();
                if let Some(pipe) = child.stderr.as_mut() {
                    let _ = pipe.read_to_string(&mut said);
                }
                return Err(format!(
                    "security {}",
                    security_failure_word(status.code(), &said)
                ));
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("키체인이 응답하지 않습니다(암호를 묻고 있을 수 있습니다)".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// The exit of a `security` run, in words. Code 44 is its "no such item" and 45
/// its "already exists" — answers rather than faults, each given the word the
/// callers classify by. Anything else keeps the code, and every refusal keeps
/// the tool's first stderr line, which is where `security` says WHICH call
/// failed and why.
#[cfg(target_os = "macos")]
fn security_failure_word(code: Option<i32>, stderr: &str) -> String {
    let said = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let word = match code {
        Some(44) => NO_SUCH_ITEM.to_string(),
        Some(45) => DUPLICATE_ITEM.to_string(),
        Some(code) => format!("종료 {code}"),
        None => "신호로 끝남".to_string(),
    };
    if said.is_empty() {
        word
    } else {
        format!("{word} — {said}")
    }
}

/// The keychain the suite gets: a map in this process, keyed the same way the
/// real one is.
///
/// Same shape of answer as the tool — a missing item is an `Err`, not an empty
/// string — so the code under test takes the same branches it takes on a real
/// Mac, and no test can reach the login keychain to find out.
#[cfg(all(target_os = "macos", test))]
fn security_command(args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    type TestKeychain = HashMap<String, Result<String, String>>;
    static HELD: OnceLock<Mutex<TestKeychain>> = OnceLock::new();
    // Items a test asked the fixture to refuse deleting, once each — the
    // keychain's own "could not delete" (locked, or an access list that does
    // not name the caller), spent by the refusal so a retry meets the
    // ordinary keychain.
    static PINNED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let held = HELD.get_or_init(|| Mutex::new(HashMap::new()));
    let mut pinned = PINNED
        .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
        .lock()
        .expect("test keychain pins");
    let value_after = |flag: &str| -> String {
        args.iter()
            .position(|arg| *arg == flag)
            .and_then(|at| args.get(at + 1))
            .map(|found| (*found).to_string())
            .unwrap_or_default()
    };
    let key = format!("{}\u{0}{}", value_after("-s"), value_after("-a"));
    let mut table = held.lock().expect("test keychain");
    match args.first().copied() {
        Some("add-generic-password") => {
            // Two roads, both measured against the real tool (2026-09-10): a
            // value after `-w` is stored whole; a bare `-w` answered on stdin
            // goes through the tool's password prompt, which keeps the first
            // `SECURITY_PROMPT_BYTES` and drops the rest. An empty stdin would
            // mean the tool was left asking a question nobody is there to answer.
            let argv_value = value_after("-w");
            let password = if !argv_value.is_empty() {
                argv_value
            } else {
                let Some(typed) = stdin.filter(|held| !held.is_empty()) else {
                    return Err("`security -w` was left interactive".into());
                };
                let typed = typed.split('\n').next().unwrap_or_default();
                typed.chars().take(SECURITY_PROMPT_BYTES).collect()
            };
            // Like the tool: a second add under one service and account is
            // exit 45 unless `-U` asked for an update in place.
            if table.contains_key(&key) && !args.contains(&"-U") {
                return Err(format!("security {DUPLICATE_ITEM}"));
            }
            table.insert(key, Ok(password));
            Ok(String::new())
        }
        // Refused in the tool's own words, out of the same constant the real
        // exit code is translated with — a classification that read one word on
        // a Mac and another in a test would be a gate that proves nothing.
        Some("find-generic-password") => table
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Err(format!("security {NO_SUCH_ITEM}"))),
        Some("test-refuse-read") => {
            table.insert(key, Err("fixture locked keychain".into()));
            Ok(String::new())
        }
        Some("delete-generic-password") if pinned.remove(&key) => {
            Err("security 종료 51 — fixture refused this delete".into())
        }
        Some("delete-generic-password") => table
            .remove(&key)
            .map(|_| String::new())
            .ok_or_else(|| format!("security {NO_SUCH_ITEM}")),
        Some("test-pin-item") => {
            pinned.insert(key);
            Ok(String::new())
        }
        other => Err(format!("이 시험 키체인이 모르는 명령입니다: {other:?}")),
    }
}

/// Whether this build has a keychain to keep a secret in — the one statement of
/// the platform fact every keychain function here is gated on. A caller whose
/// file holds only a variable's name (a router row) must refuse a secret when
/// this is false, because the no-op writes below would keep it nowhere.
pub(crate) const KEYCHAIN_AVAILABLE: bool = cfg!(target_os = "macos");

/// Put the credentials into a named service.
///
/// A no-op off macOS: the keychain is Apple's, and the original guards the same
/// call the same way (`runtime-auth-service.ts:422`). Everywhere else the file
/// is the whole story.
pub(crate) fn write_keychain_service(service: &str, credentials: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // One writer at a time (`KEYCHAIN_WRITES`): two panes resuming one
        // account's conversations on a restart each ran this pair within
        // 300 ms, and the second add met the first's item — "security 종료
        // 45", and a launch refused for it (2026-09-13).
        let _one_writer = KEYCHAIN_WRITES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let user = keychain_account(keychain_user())?;
        let delete = || {
            security_command(
                &["delete-generic-password", "-s", service, "-a", &user],
                None,
            )
        };
        let add = || {
            security_command(
                &[
                    "add-generic-password",
                    "-A",
                    "-s",
                    service,
                    "-a",
                    &user,
                    "-w",
                    credentials,
                ],
                None,
            )
        };
        let refused =
            |error: String| format!("키체인에 자격증명을 쓰지 못했습니다: {service} — {error}");
        // Delete, then add — deliberately not `-U`. An update in place keeps
        // the OLD item's access list, and the reason every write goes through
        // `security -A` is to leave an item any signer can read (see
        // `security_command`). A missing item is the ordinary first write;
        // any other refusal of this delete is diagnosed by the add below.
        let _ = delete();
        match add() {
            Ok(_) => Ok(()),
            Err(error) if error.contains(DUPLICATE_ITEM) => {
                // The item was still standing: the delete above was refused,
                // or a writer this process cannot see landed between the two
                // runs. Once more — and this time a delete that refuses is
                // the answer, in the tool's words, instead of a duplicate
                // nobody can explain.
                match delete() {
                    Ok(_) => {}
                    Err(word) if word.contains(NO_SUCH_ITEM) => {}
                    Err(word) => {
                        return Err(format!(
                            "키체인의 기존 항목을 지우지 못해 자격증명을 쓰지 못했습니다: {service} — {word}"
                        ));
                    }
                }
                add().map(|_| ()).map_err(refused)
            }
            Err(error) => Err(refused(error)),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, credentials);
        Ok(())
    }
}

#[allow(dead_code)]
pub(crate) fn read_keychain_service(service: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let user = keychain_account(keychain_user())?;
        security_command(
            &["find-generic-password", "-s", service, "-a", &user, "-w"],
            None,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = service;
        Err("keychain is macOS only".into())
    }
}

pub(crate) fn delete_keychain_service(service: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let user = keychain_account(keychain_user())?;
        match security_command(
            &["delete-generic-password", "-s", service, "-a", &user],
            None,
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.contains(NO_SUCH_ITEM) => Ok(()),
            Err(error) => Err(error),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = service;
        Ok(())
    }
}

/// Absence is different from a locked or unreadable keychain. A transaction
/// must not overwrite a key it could not back up.
pub(crate) fn read_keychain_service_if_present(service: &str) -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        match read_keychain_service(service) {
            Ok(key) => Ok(Some(key)),
            Err(error) if error.contains(NO_SUCH_ITEM) => Ok(None),
            Err(error) => Err(error),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = service;
        Ok(None)
    }
}

/// Put the credentials into the scoped service this home answers to.
fn write_keychain(home: &Path, credentials: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        for service in keychain_services(home) {
            write_keychain_service(&service, credentials)?;
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (home, credentials);
        Ok(())
    }
}

/// Is the one home ALREADY this account, down to the bytes?
///
/// Every clause is a way the answer could be "no" while the file alone looks
/// right, and each one has to be asked or the skip becomes a way to strand
/// somebody half-switched:
///
/// - the record has to name this account, and the bytes we last wrote have to be
///   these bytes — otherwise the CLI rotated the token since and step 1 owes it
///   a copy;
/// - the home has to actually hold them now;
/// - the settings file has to already name this login, which is the half a
///   person SEES (a home carrying one account's token under another account's
///   name is exactly the "고른 계정과 다른 계정" they reported);
/// - the conversations have to have come home already, because that walk
///   runs once and skipping the road would mean it never runs at all;
/// - and the KEYCHAIN must not contradict all of that. On macOS it is the half
///   the CLI reads, and `on_disk` falls back to the file when the keychain has
///   nothing to say — so a home whose file is perfect and whose keychain holds a
///   signed-out document satisfied every clause above and launched
///   `Please run /login`. That exact state was on this machine at 17:10 and
///   healed only because `gathered` happened to be false at the time.
///
///   Only a keychain that ANSWERED counts against the skip. A refusal or a
///   locked keychain teaches nothing, and refusing the skip on silence would
///   put a keychain write on every launch — the dialog storm this clause exists
///   to prevent in the first place.
fn already_materialized(
    state: &RuntimeAuth,
    account: &ClaudeAccount,
    on_disk: Option<&str>,
    on_file: Option<&str>,
    says: &KeychainSays,
    settings: &Path,
    store_dir: &Path,
) -> bool {
    state.gathered
        && state.account.as_deref() == Some(account.id.as_str())
        && state.written.as_deref() == on_disk
        && on_disk.is_some()
        && state.written.as_deref() == on_file
        && !says.contradicts_a_login()
        && oauth_block(settings) == stored_identity(store_dir)
}

/// Whose drawer the login currently in the one home belongs in.
///
/// The home's own settings file names who is signed in there, and that name —
/// not our bookkeeping — is what decides. They agree whenever the CLI merely
/// refreshed a token, and they disagree exactly when the person logged in
/// themselves, which is the case that misfiles credentials if bookkeeping wins.
///
/// Falls back to the recorded owner only when the home names nobody at all: a
/// keychain-only login carries no address, and for that shape the record is the
/// best answer there is. When the home names somebody this window does not hold,
/// the answer is `None` and the bytes are left alone.
fn home_owner_dir(config_root: &Path, settings: &Path, recorded: Option<&str>) -> Option<PathBuf> {
    let Some(named) = oauth_block(settings) else {
        return recorded.and_then(|owner| owner_dir(config_root, owner));
    };
    read_store(config_root)
        .accounts
        .into_iter()
        .find(|held| {
            let identity = ClaudeIdentity::from_credentials(
                &serde_json::json!({"oauthAccount": named}).to_string(),
            );
            !held.identity_changed(&identity)
                && held.pending.is_none()
                && stored_identity(Path::new(&held.config_dir)).as_ref() == Some(&named)
        })
        .map(|held| PathBuf::from(held.config_dir))
}

/// Put the selected account's login into the one home the CLI reads.
///
/// The four invariants, each one a way to log somebody out if it is dropped:
///
/// 1. **Read back before overwriting.** The CLI writes refreshed tokens into the
///    runtime credentials file. If what is there is not what we last wrote, it is
///    newer than ours — and it goes back to the account the HOME names, which is
///    not always the one we recorded (see `home_owner_dir`) (`:355-360`).
/// 2. **Do not write what is already there.** Same bytes means no write at all —
///    the original skips for Windows `EPERM` contention (`:1737-1748`), and here
///    it also means a launch of the already-selected account never reaches the
///    keychain, which is the one call that can put a dialog on a screen.
/// 3. **Undo the file if the keychain refuses.** A file naming one account and a
///    keychain naming another is worse than either alone, because which one a
///    build reads is not ours to decide (`:426-432`).
/// 4. **Refuse rather than guess.** An account with no readable credentials is
///    left alone entirely; another terminal's login is never substituted.
pub(crate) fn materialize(config_root: &Path, account: &ClaudeAccount) -> Result<(), String> {
    let home = runtime_home(config_root);
    materialize_into(config_root, account, &home)
}

/// The same, with the home named — so this is testable without a process-wide
/// `HOME`, which is the one thing a suite running in parallel cannot borrow.
fn materialize_into(
    config_root: &Path,
    account: &ClaudeAccount,
    home: &Path,
) -> Result<(), String> {
    require_unchanged_identity(config_root, account)?;
    let store_dir = Path::new(&account.config_dir);
    seed_scoped_keychain(store_dir)?;
    let live = home.join(CREDENTIALS_FILE);
    let settings = home.join(CLAUDE_SETTINGS_FILE);
    // What stands in the one home now — the file when there is one, and the
    // keychain when there is not, for the same reason the account's own read
    // has to look in both.
    let on_file = std::fs::read_to_string(&live)
        .ok()
        .filter(|text| holds_login(text));
    // One read, two facts: what the home holds, and whether the keychain is the
    // half that said so. Asking twice would be a second chance for a dialog on
    // a road that is walked on every launch.
    let (on_disk, says) = login_at(home);
    let named_home = home.to_string_lossy().into_owned();
    let mut state = read_runtime(config_root);
    if state.version != RUNTIME_AUTH_VERSION || state.home.as_deref() != Some(named_home.as_str()) {
        state = RuntimeAuth {
            version: RUNTIME_AUTH_VERSION,
            home: Some(named_home),
            ..RuntimeAuth::default()
        };
    }

    // 0. Already this account, already these bytes: touch NOTHING.
    //
    //    This is invariant 3 applied where it actually bites. It used to guard
    //    the file alone, so every launch of the already-selected account still
    //    went to the keychain — and a keychain write is not free the way an
    //    identical file write is: it is the one call on this road that can put a
    //    password dialog on somebody's screen. Launching agents all morning
    //    meant asking all morning ("키체인이 계속"). The selected account
    //    changing is a deliberate, rare act, and it is the only thing that has
    //    any business reaching the keychain.
    if already_materialized(
        &state,
        account,
        on_disk.as_deref(),
        on_file.as_deref(),
        &says,
        &settings,
        store_dir,
    ) {
        return Ok(());
    }

    // Only a real switch needs the account store. In particular, do not open a
    // per-account keychain item on every launch merely to prove again what the
    // app-owned runtime file and our durable record already prove.
    let mut credentials = credentials_at(store_dir)
        .ok_or_else(|| "이 계정의 자격증명을 읽지 못했습니다".to_string())?;

    // 1. Whatever the CLI rotated goes home before we write over it — TO THE
    //    ACCOUNT IT ACTUALLY NAMES, which is not always the one we last wrote.
    //
    //    The original files it under whoever was last materialized
    //    (`:355-360`), and that is right for the case it was written for: a
    //    token REFRESH keeps the same identity, so the newer bytes belong to the
    //    same account. But the person can also just log in themselves, and then
    //    the home holds a DIFFERENT login — new token and new identity both. On
    //    this machine that was the live state: the record said one account while
    //    `~/.claude.json` named another, because the person had run `/login` an
    //    hour earlier. Filing those bytes under the recorded owner would put one
    //    person's credentials in another person's drawer, and the next switch to
    //    that account would hand out the wrong login entirely.
    //
    //    So the identity in the home decides. When it names an account we hold,
    //    the bytes go there. When it names nobody we know, they stay where they
    //    are — a login this window cannot place is not a login it may file.
    if state.account.is_some()
        && state.written.is_some()
        && let Some(disk) = on_disk.as_deref()
        && state.written.as_deref() != Some(disk)
        && holds_login(disk)
        && let Some(dir) = home_owner_dir(config_root, &settings, state.account.as_deref())
    {
        // A seeded store now reads its scoped keychain first too. Keep both
        // halves current, or the next switch resurrects its pre-refresh token.
        write_private(&dir.join(CREDENTIALS_FILE), disk)?;
        write_keychain(&dir, disk)?;
        // When the runtime rotated the account we are materializing, that
        // read-back is now the newest managed credential too. Rewriting the
        // older value we loaded above would persist the refresh and immediately
        // undo it in the live runtime.
        if dir == store_dir {
            credentials = disk.to_string();
        }
    }

    // 2. Nothing to do when it is already there — but the bookkeeping still is,
    //    because the file may have been ours all along and unrecorded.
    let already_on_file = on_file.as_deref() == Some(credentials.as_str());
    if !already_on_file {
        write_private(&live, &credentials)?;
    }

    // 3. And the keychain, or the file goes back.
    if let Err(error) = write_keychain(home, &credentials) {
        // The undo restores the FILE, which is the only half this window wrote
        // before the keychain refused.
        if !already_on_file {
            match on_file.as_deref() {
                Some(previous) => {
                    let _ = write_private(&live, previous);
                }
                None => {
                    let _ = std::fs::remove_file(&live);
                }
            }
        }
        return Err(error);
    }

    carry_first_run_into(&settings, &store_dir.join(CLAUDE_SETTINGS_FILE));
    set_oauth_block(&settings, stored_identity(store_dir).as_ref());

    // And every account's past comes home — all of them, not only this one.
    // The person has several logins and one history, which is the whole point
    // of the change this rides with.
    if !state.gathered {
        let mut moved = 0usize;
        for held in read_store(config_root).accounts {
            moved += gather_conversations(Path::new(&held.config_dir), home);
        }
        let copied = external_runtime_home()
            .filter(|external| external != home)
            .map_or(0, |external| copy_conversations(&external, home));
        if moved > 0 {
            eprintln!("zerocode-shell: {moved} conversations moved into the one Claude home");
        }
        if copied > 0 {
            eprintln!(
                "zerocode-shell: {copied} conversations copied into the isolated Claude home"
            );
        }
        state.gathered = true;
    }

    state.account = Some(account.id.clone());
    state.written = Some(credentials);
    write_runtime(config_root, &state);
    Ok(())
}

/// The OAuth identity block this account keeps, out of wherever it keeps it.
///
/// The original's store holds it in a file of its own whose whole contents ARE
/// the block (`managed-auth-path.ts:60-63`). Ours is still a full settings file
/// with the block under a key, so both are read and the dedicated one wins —
/// which is also what makes moving to that layout a change of one directory
/// rather than a change of this reader.
fn stored_identity(store_dir: &Path) -> Option<serde_json::Value> {
    if let Ok(text) = std::fs::read_to_string(store_dir.join(OAUTH_ACCOUNT_FILE))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
        && value.is_object()
    {
        return Some(value);
    }
    oauth_block(&store_dir.join(CLAUDE_SETTINGS_FILE))
}

/// Bring the conversations an account directory is still holding into the one
/// home, so the person keeps every past they already had.
///
/// **Not optional, and not cleanup.** While an account WAS a config directory,
/// the CLI wrote that account's transcripts inside it, and selecting the
/// account is what made them readable. Now that a launch reads the one home,
/// those conversations would simply stop opening — the change that fixes
/// switching would, on its own, take the history it was meant to protect. So
/// the move travels with it.
///
/// Careful in the two ways that matter for somebody's own record:
///
/// - **Nothing is overwritten.** A destination that exists is left exactly as
///   it is and the source is left beside it. Session ids are UUIDs, so a
///   collision means the same conversation is already there.
/// - **Nothing is deleted.** A file that could not be moved stays where it was;
///   the worst outcome is a conversation that is still only visible the old
///   way, which is where it was before this ran.
///
/// Answers how many conversations moved, so a boot can say so once and never
/// again — a second run finds nothing and costs a directory listing.
fn gather_conversations(store_dir: &Path, home: &Path) -> usize {
    let from = store_dir.join("projects");
    let into = home.join("projects");
    transfer_conversations(&from, &into, true)
}

/// Seed the isolated runtime from the terminal home without changing that
/// terminal's history. This is the bridge for conversations created before the
/// runtime became app-owned; afterwards the two stores evolve independently.
fn copy_conversations(external_home: &Path, home: &Path) -> usize {
    let from = external_home.join("projects");
    let into = home.join("projects");
    transfer_conversations(&from, &into, false)
}

fn transfer_conversations(from: &Path, into: &Path, remove_source: bool) -> usize {
    if !from.is_dir() {
        return 0;
    }
    let mut transferred = 0usize;
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Symlinks are not descended and not moved: a link out of this
            // directory is not this directory's to carry anywhere.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(under) = path.strip_prefix(from) else {
                continue;
            };
            let target = into.join(under);
            if target.exists() {
                continue;
            }
            if let Some(parent) = target.parent()
                && std::fs::create_dir_all(parent).is_err()
            {
                continue;
            }
            if remove_source && std::fs::rename(&path, &target).is_ok() {
                transferred += 1;
            } else if std::fs::copy(&path, &target).is_ok() {
                if remove_source {
                    let _ = std::fs::remove_file(&path);
                }
                transferred += 1;
            }
        }
    }
    transferred
}

/// Where the account with this id keeps its own copy.
fn owner_dir(config_root: &Path, id: &str) -> Option<PathBuf> {
    read_store(config_root)
        .accounts
        .into_iter()
        .find(|account| account.id == id)
        .map(|account| PathBuf::from(account.config_dir))
}

/// Run the CLI's own login against a fresh directory and return who arrived.
///
/// `program` is the command the agent catalogue found — not hardcoded, because a
/// machine may have `claude` under another name and the catalogue already knows
/// which.
///
/// **`auth login --claudeai`, not the REPL.** The first version ran `/login`,
/// which starts the interactive TUI — and a TUI needs a terminal, which a GUI
/// app does not have. It could never work from the Dock, and from a terminal it
/// would have fought the terminal that launched the app. Orca runs the
/// subcommand (`runClaudeLoginAndCapture`, out/main/index.js:209870, 1.4.164):
/// it opens the browser flow itself and exits when the person finishes, no
/// terminal involved. Stdin is held open for it (Orca's `keepStdinOpen`) and
/// both output pipes are drained — a pipe nobody reads fills, and then the
/// login blocks on a `println` and times out looking innocent
/// (`runClaudeCommand`, :210156, pipes all three and reads).
///
/// Blocking and long: the caller puts it on a pool. The timeout is minutes
/// because a browser is waiting for a human.
fn run_login(program: &str, dir: &Path) -> Result<(), String> {
    use std::io::Read;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    seed_scoped_keychain(dir)?;
    let mut command = crate::proc::quiet_command(program);
    // The shell's PATH, when it has answered. `Command` resolves a bare program
    // name against the CHILD's PATH on unix, and a Finder-launched app's own
    // PATH has no homebrew and no npm on it — without this line, the add button
    // works from a terminal and fails from the Dock, which is exactly how the
    // detection bug shipped. See `shell_path`.
    if let Some(path) = crate::shell_path::hydrated() {
        command.env("PATH", path);
    }
    let mut child = command
        .args(["auth", "login", "--claudeai"])
        .env(zerocode_core::CONFIG_DIR_VAR, dir)
        .env(zerocode_core::SECURE_STORAGE_CONFIG_DIR_VAR, dir)
        // The overriding variables are cleared here too, for the same reason
        // they are cleared at launch: an ambient API key makes the CLI ignore
        // the directory, and then the login would write nothing here and
        // "succeed".
        .envs(
            zerocode_core::OVERRIDING_AUTH_VARS
                .iter()
                .map(|name| (*name, "")),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("{program}을(를) 실행할 수 없습니다: {error}"))?;
    // Held, not dropped: dropping the handle closes the CLI's stdin, and the
    // login treats a closed stdin as a cancelled session.
    let stdin = child.stdin.take();
    // Both pipes are drained on their own threads; the stderr tail is the only
    // place the CLI says WHY a login failed.
    let stdout = child.stdout.take();
    let _stdout_drain = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stdout {
            let _ = stream.read_to_string(&mut text);
        }
    });
    let stderr = child.stderr.take();
    let stderr_drain = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stderr {
            let _ = stream.read_to_string(&mut text);
        }
        text
    });

    let began = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                drop(stdin);
                return Ok(());
            }
            Ok(Some(_)) => {
                drop(stdin);
                let why = stderr_drain
                    .join()
                    .ok()
                    .map(|text| text.trim().lines().last().unwrap_or_default().to_string())
                    .filter(|line| !line.is_empty());
                return Err(match why {
                    Some(line) => format!("로그인이 실패했습니다: {line}"),
                    None => "로그인이 실패했습니다".into(),
                });
            }
            Ok(None) => {
                if began.elapsed() >= LOGIN_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("로그인이 완료되지 않았습니다".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}
/// Add an account: make a directory, log in against it, and keep it if a real
/// identity came back.
///
/// The directory is removed again on every failure path. A half-made account is
/// worse than none — it would sit in the list with no name and switch a launch
/// to credentials that do not exist.
pub fn add_account(
    config_root: &Path,
    local_data_root: &Path,
    program: &str,
    now: i64,
) -> Result<AccountStore, String> {
    let id = new_id(now);
    let dir = account_dir(local_data_root, &id).ok_or("계정을 저장할 위치를 찾지 못했습니다")?;
    let result = add_into(config_root, program, &id, &dir, now);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn add_into(
    config_root: &Path,
    program: &str,
    id: &str,
    dir: &Path,
    now: i64,
) -> Result<AccountStore, String> {
    run_login(program, dir)?;
    // One check rather than two. "The login left nothing" and "the login left
    // something we cannot name" are the same outcome for a list of accounts —
    // and asking after the NAME is the question that survives the platform
    // difference, since on macOS the file the first question looked for is not
    // written at all.
    let identity = identity_in(dir);
    let Some(email) = identity.email.clone().filter(|one| !one.trim().is_empty()) else {
        return Err("로그인이 계정 정보를 남기지 않았습니다".into());
    };
    let mut store = read_store(config_root);
    if let Some(held) = duplicate_of(&store.accounts, &identity) {
        return Err(format!("{}은(는) 이미 추가된 계정입니다", held.label()));
    }
    store.accounts.push(ClaudeAccount {
        id: id.to_string(),
        email,
        organization_uuid: identity.organization_uuid,
        organization_name: identity.organization_name,
        account_uuid: identity.account_uuid,
        organization_type: identity.organization_type,
        pending: None,
        config_dir: dir.to_string_lossy().into_owned(),
        added_at: now,
    });
    // The first account added becomes the selected one; a later one does not
    // steal the selection out from under a running window.
    if store.selection.active.is_none() {
        store.selection.active = Some(id.to_string());
    }
    write_store(config_root, &store)?;
    Ok(store)
}

/// Whether a directory's login still WORKS, asked of the CLI itself.
///
/// `signed_in` can only ask who the directory names — the name lives in a
/// file, the token in the macOS keychain, and the two die separately (the
/// 2.1.232 update killed every stored token on this machine while every
/// name survived, live report 2026-08-14: "여러 계정이 연결됐음" over four
/// dead logins). `claude auth status` is the one honest oracle, so the
/// settings panel asks it per row, off the paint path.
///
/// The environment matches `run_login`'s: the directory chosen, the
/// overriding variables cleared — an ambient API key would make every dead
/// login look alive.
///
/// THREE outcomes, not two. `None` is "the question was not answered", and it
/// is not a synonym for `Some(false)`: the row this marks wears 「로그인이
/// 만료되었습니다 — 다시 로그인하세요」, which is a diagnosis, and the only
/// evidence for it is the CLI's own `loggedIn: false`. A probe that could not
/// start, exited badly, or printed something that is not its JSON has learned
/// nothing about the credential — and this used to fold all three into
/// `false` and accuse a working login (live report 2026-08-25: "지금 로그인
/// 계정인데 잘사용하고있는데 로그인 하라고 표시되는 버그"). It is the same
/// discipline `usage::SIGNED_OUT_STATUS` already states for the scan: "could
/// not read" must not mark anybody.
pub fn login_alive(program: &str, dir: &Path) -> Option<bool> {
    probe_identity(program, dir).0
}

fn login_alive_in(program: &str, config_dir: &Path, secure_storage_dir: &Path) -> Option<bool> {
    if config_dir == secure_storage_dir {
        return login_alive(program, config_dir);
    }
    login_status_in(program, config_dir, secure_storage_dir)?
        .get("loggedIn")
        .and_then(serde_json::Value::as_bool)
}

fn login_status_in(
    program: &str,
    config_dir: &Path,
    secure_storage_dir: &Path,
) -> Option<serde_json::Value> {
    seed_scoped_keychain(secure_storage_dir).ok()?;
    let mut command = crate::proc::quiet_command(program);
    if let Some(path) = crate::shell_path::hydrated() {
        command.env("PATH", path);
    }
    let mut child = command
        .args(["auth", "status"])
        .env(zerocode_core::CONFIG_DIR_VAR, config_dir)
        .env(
            zerocode_core::SECURE_STORAGE_CONFIG_DIR_VAR,
            secure_storage_dir,
        )
        .envs(
            zerocode_core::OVERRIDING_AUTH_VARS
                .iter()
                .map(|name| (*name, "")),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // Read WHILE waiting, or a child that fills the pipe buffer never exits
    // and the deadline below turns a healthy answer into a kill.
    let drained = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut pipe, &mut said);
            said
        })
    });
    let deadline = std::time::Instant::now() + LOGIN_PROBE_DEADLINE;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let said = drained
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if !ended.is_some_and(|status| status.success()) {
        return None;
    }
    serde_json::from_slice::<serde_json::Value>(&said).ok()
}

/// Probe a store and retain the identity the CLI actually reported.
pub fn probe_identity(program: &str, dir: &Path) -> (Option<bool>, ClaudeIdentity) {
    let Some(status) = login_status_in(program, dir, dir) else {
        return (None, ClaudeIdentity::default());
    };
    let alive = status.get("loggedIn").and_then(serde_json::Value::as_bool);
    let identity = if alive == Some(true) {
        let mut reported = ClaudeIdentity::from_credentials(&status.to_string());
        let profile = identity_in(dir);
        if !reported.is_namable() {
            profile
        } else {
            if reported.organization_uuid.is_none()
                || reported.organization_uuid == profile.organization_uuid
            {
                reported.account_uuid = reported.account_uuid.or(profile.account_uuid);
                reported.organization_uuid =
                    reported.organization_uuid.or(profile.organization_uuid);
                reported.organization_name =
                    reported.organization_name.or(profile.organization_name);
                reported.organization_type =
                    reported.organization_type.or(profile.organization_type);
            }
            reported
        }
    } else {
        ClaudeIdentity::default()
    };
    (alive, identity)
}

/// Merge completed probes against the current store, ignoring stale observations.
pub fn observe_identities(
    config_root: &Path,
    observations: &[(ClaudeAccount, ClaudeIdentity)],
) -> Result<(), String> {
    let mut store = read_store(config_root);
    let mut changed = false;
    for (asked, identity) in observations {
        if let Some(held) = store.accounts.iter_mut().find(|held| *held == asked) {
            held.observe_identity(identity.clone());
            changed |= held != asked;
        }
    }
    if changed {
        write_store(config_root, &store)?;
    }
    Ok(())
}

fn require_unchanged_identity(config_root: &Path, account: &ClaudeAccount) -> Result<(), String> {
    let observed = identity_in(Path::new(&account.config_dir));
    observe_identities(config_root, &[(account.clone(), observed.clone())])?;
    let held = read_store(config_root)
        .accounts
        .into_iter()
        .find(|held| held.id == account.id);
    if account.identity_changed(&observed)
        || account.pending.is_some()
        || held.is_some_and(|held| held.pending.is_some())
    {
        return Err(
            "로그인이 바뀌었습니다. 설정에서 새 계정으로 추가하거나 이 행에 적용하세요".into(),
        );
    }
    Ok(())
}

/// How long one row's probe may take before it is abandoned as unanswered.
///
/// Every probe is a CLI start, and the settings panel runs one per account at
/// once. `Command::output()` waits forever, so one wedged start held the whole
/// verification — and a verification that never returns leaves whatever guess
/// was on the rows standing.
const LOGIN_PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// Log in again into the directory an account already has.
///
/// The repair for a token that died while the directory kept its name: the
/// CLI's banner still says who the account is (it reads the same file this
/// window reads), and every terminal it opens says `Please run /login`. The
/// old road out was 지우기 + 계정 추가, which throws away the directory's
/// settings and its place in the list for a credential problem (live report
/// 2026-08-14, 1-g15).
///
/// The same `run_login` the add road uses — one login mechanism, so the
/// PATH hydration, the cleared auth variables and the drained pipes cannot
/// come to differ between adding and repairing. Afterwards the identity is
/// re-read, because a repair may land on a DIFFERENT person: the CLI asks
/// which account to use and nobody promised it would be the same one.
pub fn relogin_account(
    config_root: &Path,
    program: &str,
    id: &str,
) -> Result<AccountStore, String> {
    let store = read_store(config_root);
    let original = store
        .accounts
        .iter()
        .find(|account| account.id == id)
        .ok_or("그런 계정이 없습니다")?;
    let dir = PathBuf::from(&original.config_dir);
    // The CLI owns this attempt; the previous login remains usable until the
    // person chooses what a different organization should mean for the row.
    let attempt = dir.join(PENDING_LOGIN_DIR);
    std::fs::create_dir_all(&attempt).map_err(|error| error.to_string())?;
    run_login(program, &attempt)?;
    let identity = identity_in(&attempt);
    if !identity.is_namable() {
        return Err("로그인이 계정 정보를 남기지 않았습니다".into());
    }
    let mut current = read_store(config_root);
    let held = current
        .accounts
        .iter_mut()
        .find(|held| *held == original)
        .ok_or("로그인 중 계정이 바뀌었습니다. 계정 목록을 새로 확인하세요")?;
    if held.identity_changed(&identity) {
        held.pending = Some(identity);
        write_store(config_root, &current)?;
        return Ok(current);
    }
    copy_login(&attempt, &dir)?;
    held.accept_identity(identity);
    let refreshed = held.clone();
    let selected = current.selection.active.as_deref() == Some(id);
    write_store(config_root, &current)?;
    if selected {
        invalidate_materialized_account(config_root, id);
        materialize(config_root, &refreshed)?;
    }
    clear_login(&attempt)?;
    let _ = std::fs::remove_dir_all(&attempt);
    Ok(current)
}

const PENDING_LOGIN_DIR: &str = "pending-login";

/// Copy only the login, retaining the target's conversations and settings.
fn copy_login(from: &Path, into: &Path) -> Result<(), String> {
    let credentials = credentials_at(from).ok_or("이 계정의 자격증명을 읽지 못했습니다")?;
    let identity = oauth_block(&from.join(CLAUDE_SETTINGS_FILE))
        .or_else(|| stored_identity(from))
        .ok_or("로그인이 계정 정보를 남기지 않았습니다")?;
    write_private(&into.join(CREDENTIALS_FILE), &credentials)?;
    write_keychain(into, &credentials)?;
    write_private(&into.join(OAUTH_ACCOUNT_FILE), &identity.to_string())?;
    set_oauth_block(&into.join(CLAUDE_SETTINGS_FILE), Some(&identity));
    Ok(())
}

/// Retire a staged or reassigned login, never its settings or conversations.
fn clear_login(dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Resolved once, and before the first delete: a keychain this machine
        // cannot address is not a keychain with nothing in it, and a clear that
        // "succeeded" without deleting anything would leave a live item behind
        // a login we believe we retired.
        let user = keychain_account(keychain_user())?;
        for service in keychain_services(dir) {
            if let Err(error) = security_command(
                &["delete-generic-password", "-s", &service, "-a", &user],
                None,
            ) && !error.contains(NO_SUCH_ITEM)
            {
                return Err(error);
            }
        }
    }
    for name in [CREDENTIALS_FILE, OAUTH_ACCOUNT_FILE] {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    for name in [CLAUDE_SETTINGS_FILE, CLAUDE_CONFIG_FILE] {
        if dir.join(name).is_file() {
            set_oauth_block(&dir.join(name), None);
        }
    }
    Ok(())
}

/// Resolve the changed login explicitly, preserving the old row when adding.
pub fn resolve_identity(
    config_root: &Path,
    local_data_root: &Path,
    id: &str,
    choice: &str,
    now: i64,
) -> Result<AccountStore, String> {
    if !matches!(choice, "add" | "apply") {
        return Err("알 수 없는 계정 선택입니다".into());
    }
    let mut store = read_store(config_root);
    let at = store
        .accounts
        .iter()
        .position(|held| held.id == id)
        .ok_or("그런 계정이 없습니다")?;
    let held = &store.accounts[at];
    let pending = held
        .pending
        .clone()
        .ok_or("확인할 로그인 변경이 없습니다")?;
    let dir = PathBuf::from(&held.config_dir);
    let attempt = dir.join(PENDING_LOGIN_DIR);
    let staged = identity_in(&attempt);
    let source = if staged.is_namable()
        && staged.organization_uuid == pending.organization_uuid
        && staged.account_uuid == pending.account_uuid
    {
        &attempt
    } else {
        &dir
    };
    let observed = identity_in(source);
    if observed.organization_uuid != pending.organization_uuid
        || observed.account_uuid != pending.account_uuid
    {
        return Err("로그인이 다시 바뀌었습니다. 계정 목록을 새로 확인하세요".into());
    }
    if choice == "apply" {
        copy_login(source, &dir)?;
        store.accounts[at].accept_identity(pending);
    } else {
        let new_id = new_id(now);
        let target =
            account_dir(local_data_root, &new_id).ok_or("계정을 저장할 위치를 찾지 못했습니다")?;
        copy_login(source, &target)?;
        let mut added = ClaudeAccount {
            id: new_id,
            config_dir: target.to_string_lossy().into_owned(),
            added_at: now,
            ..ClaudeAccount::default()
        };
        added.accept_identity(pending);
        store.accounts.push(added);
        // A probe can discover a login already replaced outside the app. There
        // is no old credential to recover then; retain the old row as signed out.
        if source == &dir {
            clear_login(&dir)?;
        }
        store.accounts[at].pending = None;
    }
    write_store(config_root, &store)?;
    if source == &attempt {
        clear_login(&attempt)?;
        let _ = std::fs::remove_dir_all(&attempt);
    }
    if choice == "apply" && store.selection.active.as_deref() == Some(id) {
        invalidate_materialized_account(config_root, id);
        materialize(config_root, &store.accounts[at])?;
    }
    Ok(store)
}

/// An id from the clock plus a counter, so two adds in one millisecond cannot
/// collide. No randomness: this crate has none, and a monotonic suffix is
/// enough for a list a person edits by hand.
fn new_id(now: i64) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("a{now}-{seq}")
}

/// Go back to this machine's own Claude login — the original's 「시스템 기본값」.
///
/// A row, not an absence. The original lists it first and calls the managed
/// accounts 선택 사항: "Orca는 일반적인 Claude 로그인을 사용할 수 있습니다. …
/// 빠르게 전환하려는 경우에만 계정을 추가하세요." So "we are not involved" is a
/// state the person has to be able to CHOOSE — rather than reach by deleting
/// every account until subtraction arrives at it, which is how it behaved.
///
/// Stored as a flag of its own, not as an empty selection: an empty selection
/// means "nothing chosen yet" and still names our home, for the reason
/// `no_account_selected_still_names_the_app_owned_home` records.
///
/// And choosing it means this window touches nothing at all: no environment
/// override, no materialization, no keychain write. That is the whole safety of
/// it — the person's own login keeps working because nobody wrote to it. Every
/// harm in this file's history came from writing that home, never from leaving
/// a launch in it.
pub fn use_system_default(config_root: &Path) -> Result<AccountStore, String> {
    let mut store = read_store(config_root);
    store.selection.active = None;
    store.selection.system_default = true;
    write_store(config_root, &store)?;
    // The record stops claiming an account is materialized, so the next switch
    // does the full write rather than taking the unchanged-runtime fast path
    // against a home nobody is pointing at any more.
    let mut state = read_runtime(config_root);
    if state.account.is_some() {
        state.account = None;
        state.written = None;
        write_runtime(config_root, &state);
    }
    Ok(store)
}

/// Select one managed account only after both ends of the switch agree.
///
/// A non-empty credential document is not proof that Claude can still use it:
/// an expired OAuth document keeps all of its fields and therefore passes the
/// cheap materialization guard.  The settings action is rare and explicit, so
/// it pays for the CLI's authoritative probe before reporting success.  That
/// probe may also refresh the account store; invalidating our runtime receipt
/// afterwards ensures those refreshed bytes reach the shared runtime now,
/// rather than on some later launch.
pub fn select_account(config_root: &Path, program: &str, id: &str) -> Result<AccountStore, String> {
    let secure_storage = owner_dir(config_root, id);
    let asked = read_store(config_root)
        .accounts
        .into_iter()
        .find(|row| row.id == id);
    select_account_with_probe(config_root, id, |config_dir| {
        let dir = secure_storage.as_deref().unwrap_or(config_dir);
        if dir == config_dir {
            let (alive, identity) = probe_identity(program, dir);
            if let Some(asked) = &asked {
                observe_identities(config_root, &[(asked.clone(), identity)]).ok()?;
            }
            alive
        } else {
            login_alive_in(program, config_dir, dir)
        }
    })
}

fn require_live_account(
    account: &ClaudeAccount,
    probe: &mut impl FnMut(&Path) -> Option<bool>,
) -> Result<(), String> {
    match probe(Path::new(&account.config_dir)) {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "선택한 Claude 계정 {}의 로그인이 만료되었습니다",
            account.email
        )),
        None => Err(format!(
            "선택한 Claude 계정 {}의 로그인 상태를 확인하지 못했습니다",
            account.email
        )),
    }
}

fn require_live_runtime(
    account: &ClaudeAccount,
    status: Option<bool>,
    action: &str,
) -> Result<(), String> {
    match status {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "선택한 Claude 계정 {}을 앱 런타임에 {action}하지 못했습니다",
            account.email
        )),
        None => Err(format!(
            "선택한 Claude 계정 {}의 앱 런타임 상태를 확인하지 못했습니다",
            account.email
        )),
    }
}

fn restore_previous_runtime(
    config_root: &Path,
    previous: Option<&ClaudeAccount>,
    attempted: &ClaudeAccount,
) -> Result<(), String> {
    let Some(previous) = previous.filter(|held| held.id != attempted.id) else {
        return Ok(());
    };
    invalidate_materialized_account(config_root, &previous.id);
    materialize(config_root, previous)
}

fn select_account_with_probe(
    config_root: &Path,
    id: &str,
    mut probe: impl FnMut(&Path) -> Option<bool>,
) -> Result<AccountStore, String> {
    let mut store = read_store(config_root);
    let account = store
        .accounts
        .iter()
        .find(|account| account.id == id)
        .cloned()
        .ok_or("그런 계정이 없습니다")?;
    require_live_account(&account, &mut probe)?;
    let previous = (!store.selection.system_default)
        .then(|| zerocode_core::active_account(&store.accounts, &store.selection))
        .flatten()
        .cloned();
    // A switch is not selected until its login is really in the app-owned
    // runtime. Reporting success first stranded the UI on an account whose
    // credentials had just been refused.
    //
    // Even selecting the same row is a switch action.  Force this pass because
    // the CLI probe above may have refreshed an expired token while our
    // receipt still matches the old, non-empty runtime document.
    invalidate_materialized_account(config_root, &account.id);
    materialize(config_root, &account)?;
    if let Err(error) = require_live_runtime(&account, probe(&runtime_home(config_root)), "적용")
    {
        if let Err(rollback) = restore_previous_runtime(config_root, previous.as_ref(), &account) {
            return Err(format!(
                "{error}; 이전 계정 런타임 복구도 실패했습니다: {rollback}"
            ));
        }
        return Err(error);
    }
    store = read_store(config_root);
    store.selection.active = Some(id.to_string());
    store.selection.system_default = false;
    write_store(config_root, &store)?;
    Ok(store)
}

/// Remove an account, and its credentials with it.
///
/// The directory goes too — leaving it behind would keep a login this window
/// has forgotten, which is the opposite of what "remove" means to the person who
/// asked. The selection is cleared when it pointed here, so the fallback in
/// `active_account` picks a real one rather than a stale name.
pub fn remove_account(
    config_root: &Path,
    local_data_root: &Path,
    id: &str,
) -> Result<AccountStore, String> {
    let mut store = read_store(config_root);
    let Some(at) = store.accounts.iter().position(|account| account.id == id) else {
        return Err("그런 계정이 없습니다".into());
    };
    let gone = store.accounts.remove(at);
    // Only under our own directory, checked rather than trusted: the stored
    // path came off disk and a path outside is not ours to delete.
    if let Some(mine) = account_dir(local_data_root, &gone.id)
        && Path::new(&gone.config_dir) == mine
    {
        clear_login(&mine.join(PENDING_LOGIN_DIR))?;
        clear_login(&mine)?;
        let _ = std::fs::remove_dir_all(&mine);
    }
    if store.selection.active.as_deref() == Some(id) {
        store.selection.active = None;
    }
    write_store(config_root, &store)?;
    Ok(store)
}

/// The environment a launch of `agent` gets from the selected account, or empty
/// when accounts do not apply.
/// Which home a Claude command should read, and the account that owns it.
///
/// The half both doors share. It decides nothing and writes nothing.
fn runtime_env_for(
    config_root: &Path,
    agent: &str,
) -> (Vec<(String, String)>, Option<ClaudeAccount>) {
    if !zerocode_core::account::providers_for(agent)
        .contains(&zerocode_core::account::Provider::Anthropic)
    {
        return (Vec::new(), None);
    }
    // The isolated home is named FIRST, and named even when no account is
    // selected. That last clause is the whole of a defect the person met as a
    // password prompt they could not get rid of.
    //
    // With no selection this used to return nothing at all, so the launch
    // inherited the ambient environment and the agent ran against the person's
    // own `~/.claude`. On macOS that means the CLI reads the person's own
    // keychain item — one this window never created and has no business
    // touching — and the reader gets an authorization dialog
    // ("이게 뜨면 안돼"). An empty answer here is not neutral: it is a decision
    // to run in somebody else's home.
    //
    // Nothing is lost by naming it: the directory exists whether or not an
    // account is chosen, an agent with no login there simply asks the person to
    // log in, and a person who set `CLAUDE_CONFIG_DIR` themselves still wins —
    // `zerocode_core::launch_env` prefers theirs over ours.
    let home = runtime_home(config_root);
    let store = read_store(config_root);
    // The system default, and ONLY it, hands back nothing: the person's own
    // Claude login, in the person's own home, with this window staying out of
    // it. An override here would put an agent in a home they did not pick, and
    // `launch_env_for` has nothing to materialize.
    //
    // Safe only because of the other half: we never WRITE that home. Every
    // injury in this file's history came from writing it (a login overwritten
    // every launch, a keychain access list rewritten and the person's own CLI
    // dropped off it), and none from letting a launch read it.
    //
    // Read from the FLAG rather than from "no account resolved", because those
    // are different answers and giving them the same one is a live injury in
    // both directions. A store with nothing added and nothing chosen still
    // names our home — an empty answer there is not neutral, it is a decision
    // to run in somebody else's, which is what
    // `no_account_selected_still_names_the_app_owned_home` was written for.
    if store.selection.system_default {
        return (Vec::new(), None);
    }
    let account = zerocode_core::active_account(&store.accounts, &store.selection).cloned();
    let secure_storage = account
        .as_ref()
        .map(|account| account.config_dir.as_str())
        .or_else(|| home.to_str());
    let headers = std::env::var(zerocode_core::account::CUSTOM_HEADERS_VAR).ok();
    let isolated = zerocode_core::launch_env(headers.as_deref(), home.to_str(), secure_storage);
    (isolated, account)
}

/// The same home, for a LOOK rather than a launch. Nothing is written.
///
/// The callers are the ones with nothing to install: the two usage scans and the
/// commit-message run. They only need to know which home to read, and having
/// them materialize on the way was how a person who touched nothing got their
/// credentials rewritten on a timer — `askEveryProviderUsage` fires every
/// fifteen minutes (`ui/shell.js`, `USAGE_AMBIENT_MS`), and a credential write is
/// the one call on this road that can put a keychain dialog on a screen. So the
/// reported drumbeat was never "every agent launch"; it was the clock.
///
/// The original draws the same line: its usage supplement runs its pty under an
/// `envPatch` and `stripAuthEnv` and installs nothing
/// (`fetchManagedUsagePanelSupplement`, out/main/index.js:209490-209516).
pub fn reading_env_for(config_root: &Path, agent: &str) -> Vec<(String, String)> {
    runtime_env_for(config_root, agent).0
}

/// Where a usage read found the login it asks the OAuth endpoint with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginFrom {
    /// The keychain item scoped to the store `CLAUDE_SECURESTORAGE_CONFIG_DIR`
    /// names — the selected account's own directory, or the runtime home when
    /// no account is selected. The CLI refreshes its token HERE.
    Keychain,
    /// A credentials file: the runtime home's `.credentials.json` — the copy
    /// [`materialize`] wrote at the last switch, and the whole story on a
    /// platform with no keychain — or Codex's `auth.json`.
    File,
}

impl LoginFrom {
    /// The source's word on a usage read's log line.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::File => "file",
        }
    }
}

/// The order a Claude usage read looks for its login in: the store the CLI
/// refreshes first, the copy this window wrote last. The one statement of it —
/// [`usage_login`] walks this and nothing else.
pub(crate) const USAGE_LOGIN_ORDER: [LoginFrom; 2] = [LoginFrom::Keychain, LoginFrom::File];

/// The login a Claude usage read asks with, and where it was found — read out
/// of `env`, the environment [`reading_env_for`] hands the CLI, so it is the
/// login the CLI itself is using.
///
/// **Why the keychain comes first.** The runtime home's `.credentials.json` is
/// the copy [`materialize`] wrote when the account was last switched to; the
/// CLI never writes it back, because on macOS it keeps its login in the item
/// scoped to its secure-storage directory and refreshes it there. So the file
/// goes stale at the CLI's first refresh, eight hours at most. Measured on the
/// machine this was written for (t-6583, 2026-09-24): the runtime file, the
/// account directory's file and the runtime home's scoped item all held a
/// token that had expired some 260 minutes earlier, and the endpoint answered
/// 401 — while the item scoped to the selected account's directory answered
/// 200 in about 400 ms.
///
/// A LOOK, and only ever one: nothing here writes, seeds or copies — a read on
/// a timer that wrote is what rewrote a person's credentials once already (see
/// [`reading_env_for`]). The keychain half IS [`keychain_says`], the read the
/// scan's [`prepare_selected_store`] and the readiness probe already make, of
/// the item scoped to a directory this window made. The unsuffixed item, the
/// person's own terminal login, is never asked about; and a system-default
/// selection hands this an empty environment, so it answers `None` and the
/// person's own login stays theirs.
pub(crate) fn usage_login(env: &[(String, String)]) -> Option<(String, LoginFrom)> {
    let named = |var: &str| {
        env.iter()
            .rev()
            .find(|(key, _)| key == var)
            .map(|(_, value)| PathBuf::from(value))
    };
    USAGE_LOGIN_ORDER.into_iter().find_map(|from| {
        let login = match from {
            LoginFrom::Keychain => named(zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR)
                .and_then(|store| keychain_says(&store).login().map(str::to_string)),
            LoginFrom::File => named(zerocode_core::account::CONFIG_DIR_VAR)
                .and_then(|home| std::fs::read_to_string(home.join(CREDENTIALS_FILE)).ok())
                .filter(|text| holds_login(text)),
        };
        login.map(|text| (text, from))
    })
}

/// Which home a LOOK at the Claude login reads (t-3996), and whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoginLook {
    /// The app-owned runtime home: the person has not chosen the machine's
    /// own login, so this is what a launch reads — and the one home whose
    /// keychain item this window may ask about, because it seeded it.
    Managed(PathBuf),
    /// The person's own login, in the person's own home; `None` where there
    /// is no such home to read (a process with no `HOME`, or a test — see
    /// [`external_runtime_home`]). Its keychain item is never asked about.
    System(Option<PathBuf>),
}

/// The home the readiness probe reads the Claude login from. Nothing is
/// written: the same line [`reading_env_for`] draws, said as a place.
pub(crate) fn login_look(config_root: &Path) -> LoginLook {
    if read_store(config_root).selection.system_default {
        LoginLook::System(external_runtime_home())
    } else {
        LoginLook::Managed(runtime_home(config_root))
    }
}

pub fn launch_env_for(config_root: &Path, agent: &str) -> Result<Vec<(String, String)>, String> {
    let (isolated, account) = runtime_env_for(config_root, agent);
    // No account chosen is not a reason to run in the person's home. The
    // isolated environment still travels; there is simply nothing to
    // materialize into it.
    let Some(account) = account else {
        return Ok(isolated);
    };
    // A selected account whose directory has gone (deleted by hand, or a state
    // directory restored from a backup) cannot be materialized, but it must not
    // send the launch back to an external terminal home either.
    let dir = Path::new(&account.config_dir);
    if !signed_in(dir) {
        return Err(format!(
            "선택한 Claude 계정 {}의 로그인이 유효하지 않습니다",
            account.email
        ));
    }
    // Its login into the one home, before the launch reads that home. On the
    // launch road and not only on the switch, because the directories that need
    // it most are the ones that already exist: a window that selected an
    // account before this road was built has a selection nobody ever
    // materialized, and a repair nobody triggers is not a repair. It costs
    // nothing when the login is already there — same bytes, no write.
    materialize(config_root, &account)?;
    let home = runtime_home(config_root);
    if !signed_in(&home) {
        return Err(format!(
            "선택한 Claude 계정 {}을 앱 런타임에 준비하지 못했습니다",
            account.email
        ));
    }
    Ok(isolated)
}

/// Refuse an unattended launch that would stop on the provider's login UI.
///
/// A system-default launch has no managed config directory and remains the
/// person's own provider setup. When this account mechanism supplied a home,
/// that home must already name a login before an orchestration worker can be
/// called ready; otherwise its briefing would be pasted into the login screen.
pub fn require_unattended_login(
    config_root: &Path,
    program: &str,
    agent: &str,
    env: &[(String, String)],
) -> Result<(), String> {
    let secure_storage = env
        .iter()
        .rev()
        .find(|(key, _)| key == zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR)
        .map(|(_, value)| Path::new(value));
    require_unattended_login_with_probe(config_root, agent, env, |config_dir| {
        login_alive_in(program, config_dir, secure_storage.unwrap_or(config_dir))
    })
}

fn require_unattended_login_with_probe(
    config_root: &Path,
    agent: &str,
    env: &[(String, String)],
    mut probe: impl FnMut(&Path) -> Option<bool>,
) -> Result<(), String> {
    if !zerocode_core::account::providers_for(agent)
        .contains(&zerocode_core::account::Provider::Anthropic)
    {
        return Ok(());
    }
    let Some(config_dir) = env
        .iter()
        .rev()
        .find(|(key, _)| key == zerocode_core::account::CONFIG_DIR_VAR)
        .map(|(_, value)| value)
    else {
        return Ok(());
    };
    let runtime = Path::new(config_dir);
    match probe(runtime) {
        Some(true) => return Ok(()),
        // An unanswered probe is not a verdict (see `login_alive`): the CLI
        // may simply have missed its deadline under build load — measured
        // 2026-09-06 05:57, a zo child pane refused with "확인하지 못했습니다"
        // while the parent session in the same home was answering turns and
        // `claude auth status` said loggedIn:true a minute later. When the
        // home durably names a login, that evidence stands in for the answer;
        // a home that names nobody still goes through the repair road below.
        None if signed_in(runtime) => {
            eprintln!(
                "zerocode-shell: {agent}의 런타임 로그인 프로브가 답하지 않아 홈의 신원으로 판정했습니다"
            );
            return Ok(());
        }
        None => {
            return Err(format!(
                "{agent}의 선택된 런타임 로그인 상태를 확인하지 못했습니다"
            ));
        }
        Some(false) => {}
    }

    // The runtime can be expired while the selected account store is healthy.
    // This is the exact drift an account-row switch used to leave behind: both
    // files remained non-empty, so `already_materialized` skipped forever and
    // every unattended pane opened on `Please run /login`.  Ask the source
    // account before writing anything, then force one repair and verify the
    // destination with the same CLI oracle.
    let store = read_store(config_root);
    let Some(account) = zerocode_core::active_account(&store.accounts, &store.selection).cloned()
    else {
        return Err(format!(
            "{agent}의 선택된 런타임이 로그인되어 있지 않습니다"
        ));
    };
    require_live_account(&account, &mut probe)?;
    invalidate_materialized_account(config_root, &account.id);
    materialize(config_root, &account)?;
    require_live_runtime(&account, probe(runtime), "준비")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The account this machine files its keychain items under. A suite that
    /// cannot name it is not testing the keychain, so it says so rather than
    /// running against an account nobody has — which is the defect these tests
    /// were written for.
    #[cfg(target_os = "macos")]
    fn this_machines_account() -> String {
        keychain_user().expect("this machine names who is running the suite")
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn file_era_login_is_seeded_before_a_probe_even_if_the_cli_cannot_start() {
        let root = tempfile::tempdir().expect("temp store");
        let credentials = r#"{"claudeAiOauth":{"accessToken":"fixture-file-era"}}"#;
        write_private(&root.path().join(CREDENTIALS_FILE), credentials).unwrap();
        assert!(matches!(keychain_says(root.path()), KeychainSays::Missing));
        assert_eq!(login_alive("/no-such-t2848-cli", root.path()), None);
        assert_eq!(keychain_says(root.path()).login(), Some(credentials));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_plain_shell_seeds_the_store_before_somebody_types_claude() {
        let root = tempfile::tempdir().unwrap();
        let held = one_account(root.path(), "typed", "typed@example.test", "typed-fixture");
        let dir = Path::new(&held.config_dir);
        assert!(matches!(keychain_says(dir), KeychainSays::Missing));
        crate::shell_account_env(root.path()).expect("plain repair shell");
        assert!(keychain_says(dir).login().is_some());
    }

    /// MEASURED 2026-09-10 on this Mac: `security add-generic-password -w` with the
    /// secret on STDIN (the tool's "password data for new item:" prompt) stores
    /// the first 128 bytes and no more — a 562-byte fixture came back as 128,
    /// and every real Claude blob is 509–2040 bytes. The window had seeded
    /// every scoped item that way since 09-06, so each held a login cut off
    /// inside its access token. Passed as the `-w` argument the same fixture
    /// came back whole. The fake tool models both roads.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_full_size_login_survives_the_keychain_write() {
        let root = tempfile::tempdir().unwrap();
        let access = "A".repeat(180);
        let refresh = "R".repeat(180);
        let credentials = format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{access}","refreshToken":"{refresh}","expiresAt":1789059636237,"scopes":["user:inference","user:profile"],"subscriptionType":"max"}}}}"#
        );
        assert!(credentials.len() > SECURITY_PROMPT_BYTES);
        write_keychain(root.path(), &credentials).unwrap();
        let service = keychain_services(root.path()).remove(0);
        let held = security_command(
            &[
                "find-generic-password",
                "-s",
                &service,
                "-a",
                &this_machines_account(),
                "-w",
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            held.len(),
            credentials.len(),
            "the keychain holds a cut-off login"
        );
        assert_eq!(held, credentials);
        assert!(matches!(keychain_says(root.path()), KeychainSays::Login(_)));
    }

    /// The item this Mac held for the window's own home on 2026-09-10: 128 bytes,
    /// `{"claudeAiOauth":{"accessToken":"…` and no closing quote. Not a logout,
    /// not a login — a broken write, which a launch repairs from the file.
    #[cfg(target_os = "macos")]
    #[test]
    fn seeding_repairs_a_damaged_scoped_item() {
        let root = tempfile::tempdir().unwrap();
        let service = keychain_services(root.path()).remove(0);
        let cut = format!(r#"{{"claudeAiOauth":{{"accessToken":"{}"#, "A".repeat(95));
        // Planted through the prompt road the old writer used, which is how the
        // damage came to exist.
        security_command(
            &[
                "add-generic-password",
                "-A",
                "-s",
                &service,
                "-a",
                &this_machines_account(),
                "-w",
            ],
            Some(&cut),
        )
        .unwrap();
        assert!(matches!(keychain_says(root.path()), KeychainSays::Damaged));
        let credentials =
            r#"{"claudeAiOauth":{"accessToken":"fixture","refreshToken":"fixture-r"}}"#;
        write_private(&root.path().join(CREDENTIALS_FILE), credentials).unwrap();
        seed_scoped_keychain(root.path()).unwrap();
        assert!(
            matches!(keychain_says(root.path()), KeychainSays::Login(held) if held == credentials)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seeding_leaves_logouts_refusals_and_existing_logins_untouched() {
        let credentials = r#"{"claudeAiOauth":{"accessToken":"fixture"}}"#;
        for previous in [
            "{}",
            r#"{"claudeAiOauth":{"accessToken":"existing-fixture"}}"#,
        ] {
            let root = tempfile::tempdir().unwrap();
            write_keychain(root.path(), previous).unwrap();
            write_private(&root.path().join(CREDENTIALS_FILE), credentials).unwrap();
            seed_scoped_keychain(root.path()).unwrap();
            let service = keychain_services(root.path()).remove(0);
            assert_eq!(
                security_command(
                    &[
                        "find-generic-password",
                        "-s",
                        &service,
                        "-a",
                        &this_machines_account()
                    ],
                    None
                )
                .unwrap(),
                previous
            );
        }
        let refused = tempfile::tempdir().unwrap();
        write_private(&refused.path().join(CREDENTIALS_FILE), credentials).unwrap();
        let service = keychain_services(refused.path()).remove(0);
        security_command(
            &[
                "test-refuse-read",
                "-s",
                &service,
                "-a",
                &this_machines_account(),
            ],
            None,
        )
        .unwrap();
        seed_scoped_keychain(refused.path()).unwrap();
        assert!(matches!(
            keychain_says(refused.path()),
            KeychainSays::NoAnswer
        ));
        let root = tempfile::tempdir().unwrap();
        write_private(&root.path().join(CREDENTIALS_FILE), "{}").unwrap();
        seed_scoped_keychain(root.path()).unwrap();
        assert!(matches!(keychain_says(root.path()), KeychainSays::Missing));
    }

    #[cfg(unix)]
    #[test]
    fn relogin_keeps_the_old_organization_and_exposes_the_new_identity_as_pending() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let mut held = one_account(root.path(), "a-old", "same@example.test", "old");
        held.organization_uuid = Some("team-org".into());
        held.organization_name = Some("Work".into());
        write_store(
            root.path(),
            &AccountStore {
                accounts: vec![held.clone()],
                ..AccountStore::default()
            },
        )
        .unwrap();
        let cli = root.path().join("login-cli");
        std::fs::write(&cli, r##"#!/bin/sh
cat > "$CLAUDE_CONFIG_DIR/.claude.json" <<'JSON'
{"oauthAccount":{"emailAddress":"same@example.test","accountUuid":"same-person","organizationUuid":"max-org","organizationName":"Personal","organizationType":"claude_max"}}
JSON
"##).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = relogin_account(root.path(), &cli.to_string_lossy(), &held.id).unwrap();
        assert_eq!(result.accounts[0].organization_uuid, held.organization_uuid);
        let row = serde_json::to_value(&result.accounts[0]).unwrap();
        assert_eq!(row["pending"]["organization_uuid"], "max-org");
        assert_eq!(read_store(root.path()).accounts, result.accounts);
    }

    fn pending_account_fixture(root: &Path) -> ClaudeAccount {
        let mut held = one_account(root, "old", "same@example.test", "original-fixture");
        held.organization_uuid = Some("team".into());
        held.organization_name = Some("Work".into());
        held.account_uuid = Some("person".into());
        let profile = serde_json::json!({"emailAddress": held.email, "accountUuid":"person", "organizationUuid":"team", "organizationName":"Work", "organizationType":"claude_team"});
        set_oauth_block(
            &Path::new(&held.config_dir).join(CLAUDE_SETTINGS_FILE),
            Some(&profile),
        );
        let attempt = Path::new(&held.config_dir).join(PENDING_LOGIN_DIR);
        let new_profile = serde_json::json!({"emailAddress":held.email, "accountUuid":"person", "organizationUuid":"max", "organizationName":"Personal", "organizationType":"claude_max"});
        write_private(
            &attempt.join(CREDENTIALS_FILE),
            r#"{"claudeAiOauth":{"accessToken":"pending-fixture"}}"#,
        )
        .unwrap();
        set_oauth_block(&attempt.join(CLAUDE_SETTINGS_FILE), Some(&new_profile));
        held.pending = Some(identity_in(&attempt));
        write_store(
            root,
            &AccountStore {
                accounts: vec![held.clone()],
                ..AccountStore::default()
            },
        )
        .unwrap();
        held
    }

    #[test]
    fn pending_choices_add_without_replacing_the_old_login_or_apply_in_place() {
        for choice in ["add", "apply"] {
            let root = tempfile::tempdir().unwrap();
            let held = pending_account_fixture(root.path());
            let old_login = credentials_at(Path::new(&held.config_dir));
            let resolved =
                resolve_identity(root.path(), root.path(), &held.id, choice, 42).unwrap();
            assert_eq!(resolved.accounts.len(), if choice == "add" { 2 } else { 1 });
            assert!(resolved.accounts.iter().all(|row| row.pending.is_none()));
            if choice == "add" {
                assert_eq!(
                    resolved.accounts[0].organization_uuid,
                    held.organization_uuid
                );
                assert_eq!(credentials_at(Path::new(&held.config_dir)), old_login);
                assert_ne!(
                    resolved.accounts[0].config_dir,
                    resolved.accounts[1].config_dir
                );
            }
            let new_login = resolved.accounts.last().unwrap();
            assert_eq!(new_login.organization_uuid.as_deref(), Some("max"));
            assert!(
                credentials_at(Path::new(&new_login.config_dir))
                    .unwrap()
                    .contains("pending-fixture")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn cli_probe_reports_a_changed_organization_as_pending_without_overwriting_the_record() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let mut held = pending_account_fixture(root.path());
        held.pending = None;
        write_store(
            root.path(),
            &AccountStore {
                accounts: vec![held.clone()],
                ..AccountStore::default()
            },
        )
        .unwrap();
        let cli = root.path().join("probe-cli");
        std::fs::write(&cli, "#!/bin/sh\nprintf '%s' '{\"loggedIn\":true,\"email\":\"same@example.test\",\"accountUuid\":\"person\",\"orgId\":\"max\",\"orgName\":\"Personal\",\"organizationType\":\"claude_max\"}'\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (alive, identity) = probe_identity(&cli.to_string_lossy(), Path::new(&held.config_dir));
        assert_eq!(alive, Some(true));
        observe_identities(root.path(), &[(held.clone(), identity.clone())]).unwrap();
        let mut observed = read_store(root.path()).accounts.remove(0);
        assert_eq!(observed.pending.take(), Some(identity));
        assert_eq!(observed, held);
        assert!(materialize(root.path(), &held).is_err());
    }

    #[test]
    fn adding_a_login_changed_outside_the_app_keeps_the_old_row_signed_out() {
        let root = tempfile::tempdir().unwrap();
        let held = pending_account_fixture(root.path());
        let dir = Path::new(&held.config_dir);
        let attempt = dir.join(PENDING_LOGIN_DIR);
        copy_login(&attempt, dir).unwrap();
        clear_login(&attempt).unwrap();
        let stale = serde_json::json!({"emailAddress":held.email, "accountUuid":"person", "organizationUuid":"team"});
        write_private(&dir.join(OAUTH_ACCOUNT_FILE), &stale.to_string()).unwrap();
        let result = resolve_identity(root.path(), root.path(), &held.id, "add", 42).unwrap();
        assert_eq!(result.accounts[0].organization_uuid, held.organization_uuid);
        assert!(!signed_in(dir));
        assert!(credentials_at(dir).is_none());
        let added = &result.accounts[1];
        assert_eq!(
            identity_in(Path::new(&added.config_dir)).organization_uuid,
            added.organization_uuid
        );
        assert!(credentials_at(Path::new(&added.config_dir)).is_some());
    }

    #[test]
    fn a_stale_probe_cannot_overwrite_an_explicit_identity_choice() {
        let root = tempfile::tempdir().unwrap();
        let asked = pending_account_fixture(root.path());
        let resolved = resolve_identity(root.path(), root.path(), &asked.id, "apply", 42).unwrap();
        observe_identities(
            root.path(),
            &[(
                asked,
                ClaudeIdentity {
                    email: Some("stale@example.test".into()),
                    organization_uuid: Some("third".into()),
                    ..ClaudeIdentity::default()
                },
            )],
        )
        .unwrap();
        assert_eq!(read_store(root.path()).accounts, resolved.accounts);
    }

    #[test]
    fn unattended_launches_refuse_a_login_screen_without_gating_other_agents() {
        let config = tempfile::tempdir().expect("config root");
        let root = tempfile::tempdir().expect("runtime home");
        let env = vec![(
            zerocode_core::account::CONFIG_DIR_VAR.to_string(),
            root.path().to_string_lossy().into_owned(),
        )];
        assert!(
            require_unattended_login_with_probe(config.path(), "claude", &env, |_| Some(false))
                .is_err(),
            "an unsigned managed home was accepted for unattended work"
        );
        assert!(
            require_unattended_login_with_probe(config.path(), "codex", &env, |_| Some(false))
                .is_ok()
        );
        assert!(
            require_unattended_login_with_probe(config.path(), "claude", &[], |_| Some(false))
                .is_ok()
        );

        std::fs::write(
            root.path().join(CLAUDE_SETTINGS_FILE),
            r#"{"oauthAccount":{"emailAddress":"worker@example.test"}}"#,
        )
        .expect("signed-in identity");
        assert!(
            require_unattended_login_with_probe(config.path(), "claude", &env, |_| Some(true))
                .is_ok()
        );
    }

    /// A probe that did not answer is not a refusal when the home names a
    /// login: the child of a session that is answering turns must not be
    /// turned away because `claude auth status` missed its deadline under
    /// load. A home naming nobody still cannot pass on silence.
    #[test]
    fn an_unanswered_probe_defers_to_the_homes_own_identity() {
        let config = tempfile::tempdir().expect("config root");
        let root = tempfile::tempdir().expect("runtime home");
        let env = vec![(
            zerocode_core::account::CONFIG_DIR_VAR.to_string(),
            root.path().to_string_lossy().into_owned(),
        )];
        assert!(
            require_unattended_login_with_probe(config.path(), "zo", &env, |_| None).is_err(),
            "silence over an empty home was accepted"
        );
        std::fs::write(
            root.path().join(CLAUDE_SETTINGS_FILE),
            r#"{"oauthAccount":{"emailAddress":"worker@example.test"}}"#,
        )
        .expect("signed-in identity");
        assert!(
            require_unattended_login_with_probe(config.path(), "zo", &env, |_| None).is_ok(),
            "silence over a home that names a login was refused"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unattended_preflight_reads_the_selected_secure_store() {
        use std::os::unix::fs::PermissionsExt as _;

        let config = tempfile::tempdir().expect("config root");
        let runtime = tempfile::tempdir().expect("runtime home");
        let secure = tempfile::tempdir().expect("secure store");
        let cli = config.path().join("status-cli");
        std::fs::write(
            &cli,
            "#!/bin/sh\n\
             if [ -n \"$CLAUDE_CONFIG_DIR\" ] && \
                [ -n \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" ] && \
                [ \"$CLAUDE_CONFIG_DIR\" != \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" ]; then\n\
               printf '{\"loggedIn\":true}'\n\
             else\n\
               printf '{\"loggedIn\":false}'\n\
             fi\n",
        )
        .expect("write fake cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake cli");
        let env = vec![
            (
                zerocode_core::account::CONFIG_DIR_VAR.to_string(),
                runtime.path().to_string_lossy().into_owned(),
            ),
            (
                zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR.to_string(),
                secure.path().to_string_lossy().into_owned(),
            ),
        ];

        require_unattended_login(config.path(), &cli.to_string_lossy(), "claude", &env)
            .expect("the selected secure store was ignored by preflight");
    }

    /// Only the CLI's own `loggedIn: false` may accuse a login.
    ///
    /// The row this marks says 「로그인이 만료되었습니다 — 다시 로그인하세요」,
    /// which is a diagnosis. A probe that could not start, exited badly, or
    /// printed something that is not its JSON has learned NOTHING — and all
    /// three used to answer `false`, which is how a working account was told
    /// to log in again.
    #[test]
    fn only_the_clis_own_no_accuses_a_login_and_everything_else_is_unknown() {
        let root = tempfile::tempdir().expect("no temp dir");
        let dir = root.path();
        let fake = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake cli");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod fake cli");
            }
            path.to_string_lossy().into_owned()
        };

        let said_yes = fake("yes-cli", "printf '{\"loggedIn\":true}'");
        assert_eq!(login_alive(&said_yes, dir), Some(true));

        let matched = fake(
            "matching-store-cli",
            "if [ \"$CLAUDE_CONFIG_DIR\" = \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" ]; then \
               printf '{\"loggedIn\":true}'; \
             else \
               printf '{\"loggedIn\":false}'; \
             fi",
        );
        assert_eq!(
            login_alive(&matched, dir),
            Some(true),
            "account status read a different secure store"
        );

        let said_no = fake("no-cli", "printf '{\"loggedIn\":false}'");
        assert_eq!(login_alive(&said_no, dir), Some(false));

        // Everything below is a question that was not answered.
        assert_eq!(
            login_alive(&dir.join("not-a-program").to_string_lossy(), dir),
            None
        );
        let rubble = fake("rubble-cli", "printf 'usage: claude [options]'");
        assert_eq!(
            login_alive(&rubble, dir),
            None,
            "unparsable stdout accused a login"
        );
        let angry = fake("angry-cli", "printf '{\"loggedIn\":false}'; exit 3");
        assert_eq!(
            login_alive(&angry, dir),
            None,
            "a failed exit accused a login"
        );
        let quiet = fake("quiet-cli", "printf '{\"other\":1}'");
        assert_eq!(
            login_alive(&quiet, dir),
            None,
            "a JSON without the field accused a login"
        );
    }

    #[cfg(unix)]
    #[test]
    fn account_login_writes_the_accounts_secure_store() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("no temp dir");
        let account = root.path().join("account");
        let cli = root.path().join("login-cli");
        std::fs::write(
            &cli,
            "#!/bin/sh\n\
             [ \"$CLAUDE_CONFIG_DIR\" = \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" ] || exit 3\n",
        )
        .expect("write fake cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake cli");

        run_login(&cli.to_string_lossy(), &account)
            .expect("login pointed its secret at another store");
    }

    #[cfg(unix)]
    #[test]
    fn selecting_an_account_keeps_its_secure_store_during_runtime_verification() {
        use std::os::unix::fs::PermissionsExt as _;

        let config = tempfile::tempdir().expect("no temp dir");
        one_account(config.path(), "a-1", "one@example.com", "tok-one");
        let cli = config.path().join("select-cli");
        std::fs::write(
            &cli,
            "#!/bin/sh\n\
             case \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" in\n\
               */stores/a-1) printf '{\"loggedIn\":true}' ;;\n\
               *) printf '{\"loggedIn\":false}' ;;\n\
             esac\n",
        )
        .expect("write fake cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake cli");

        select_account(config.path(), &cli.to_string_lossy(), "a-1")
            .expect("runtime verification forgot the selected secure store");
    }

    #[test]
    fn an_id_is_a_filesystem_safe_name_and_two_never_collide() {
        let root = tempfile::tempdir().expect("no temp dir");
        let one = new_id(1_700_000_000_000);
        let two = new_id(1_700_000_000_000);
        assert_ne!(one, two, "two adds in one millisecond collided");
        for id in [&one, &two] {
            assert!(
                id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "`{id}` is not safe as a directory name"
            );
            assert!(
                account_dir(root.path(), id).is_some(),
                "`{id}` was refused as a directory"
            );
        }
    }

    #[test]
    fn a_hostile_id_never_becomes_a_path() {
        let root = tempfile::tempdir().expect("no temp dir");
        // The ids are ours today; the guard is for the day one comes off disk.
        for hostile in ["..", "../../etc", "a/b", "", "a b", &"x".repeat(65)] {
            assert!(
                account_dir(root.path(), hostile).is_none(),
                "`{hostile}` was accepted as a directory name"
            );
        }
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
            account_dir(local_data.path(), "a1").expect("safe account id"),
            local_data.path().join(MANAGED_ACCOUNTS_DIR).join("a1")
        );
        assert_eq!(
            runtime_home(config.path()),
            config.path().join(CLAUDE_HOME_DIR),
            "the shared runtime escaped the app-owned config root"
        );
    }

    #[test]
    fn removal_deletes_only_the_home_owned_by_the_injected_local_data_root() {
        let config = tempfile::tempdir().expect("no config dir");
        let local_data = tempfile::tempdir().expect("no local data dir");
        let external = tempfile::tempdir().expect("no external dir");
        let foreign = external.path().join("foreign-home");
        std::fs::create_dir_all(&foreign).expect("make foreign home");

        let account = |id: &str, dir: &Path| ClaudeAccount {
            id: id.to_string(),
            email: format!("{id}@example.com"),
            organization_uuid: None,
            organization_name: None,
            config_dir: dir.to_string_lossy().into_owned(),
            added_at: 1,
            ..ClaudeAccount::default()
        };
        write_store(
            config.path(),
            &AccountStore {
                accounts: vec![account("a-foreign", &foreign)],
                selection: AccountSelection {
                    active: Some("a-foreign".into()),
                    system_default: false,
                },
            },
        )
        .expect("write store");
        remove_account(config.path(), local_data.path(), "a-foreign").expect("remove row");
        assert!(foreign.is_dir(), "an external agent home was deleted");

        let managed = account_dir(local_data.path(), "a-managed").expect("managed path");
        std::fs::create_dir_all(&managed).expect("make managed home");
        write_store(
            config.path(),
            &AccountStore {
                accounts: vec![account("a-managed", &managed)],
                selection: AccountSelection::default(),
            },
        )
        .expect("write store");
        remove_account(config.path(), local_data.path(), "a-managed").expect("remove account");
        assert!(!managed.exists(), "the app-owned account home was retained");
    }

    #[test]
    fn an_identity_is_the_only_thing_read_from_a_login() {
        let dir = tempfile::tempdir().expect("no temp dir");
        assert!(!signed_in(dir.path()));
        std::fs::write(
            dir.path().join(CREDENTIALS_FILE),
            r#"{"claudeAiOauth":{"accessToken":"secret-do-not-read",
                "emailAddress":"joe@example.com","organizationName":"Example"}}"#,
        )
        .expect("write");
        assert!(signed_in(dir.path()));
        let identity = identity_in(dir.path());
        assert_eq!(identity.email.as_deref(), Some("joe@example.com"));
        assert_eq!(identity.organization_name.as_deref(), Some("Example"));
        // The token is not part of the identity, and this test exists to keep
        // it that way — a future field that captured it would fail here.
        let json = serde_json::to_string(&identity).expect("serialize");
        assert!(
            !json.contains("secret-do-not-read"),
            "a token travelled inside an identity: {json}"
        );
    }

    /// A logged-OUT credentials document is not a login.
    ///
    /// The shape that caused this is the one a signed-out CLI actually writes,
    /// and it is not a stub: seven fields, a real expiry, a real subscription
    /// tier, and the two secrets set to the empty string. A test that asked only
    /// whether the object was present and non-empty called it a login — so
    /// invariant 1 "preserved" it into an account's own store on top of the token
    /// that was there, and logging in inside the app stopped sticking because the
    /// next trigger copied the empty document back over it.
    ///
    /// Measured on this machine's real shape rather than an invented one: the
    /// non-secret fields are populated here on purpose, because that is exactly
    /// what defeats every "is anything filled in?" heuristic.
    #[test]
    fn a_signed_out_document_is_not_mistaken_for_a_login() {
        let signed_out = r#"{"claudeAiOauth":{
            "accessToken":"","refreshToken":"",
            "expiresAt":1786169803544,"refreshTokenExpiresAt":1786169803544,
            "scopes":["user:inference","user:profile"],
            "subscriptionType":"max","rateLimitTier":"default_claude_max_20x"
        }}"#;
        assert!(
            !holds_login(signed_out),
            "a signed-out document passes as a login, which is how one gets \
             written over somebody's real token"
        );

        // And the same document with a token in it is, of course, a login.
        let signed_in = signed_out.replace(r#""accessToken":"""#, r#""accessToken":"tok-real""#);
        assert!(
            holds_login(&signed_in),
            "a real login stopped being recognised, which refuses every account \
             on the machine"
        );

        // The other half stays true too: a keychain payload carries no address,
        // and must still count.
        assert!(
            holds_login(r#"{"claudeAiOauth":{"refreshToken":"tok-only"}}"#),
            "a keychain-shaped payload stopped being a login"
        );
        assert!(!holds_login("{}"), "an empty object became a login");
        assert!(!holds_login("not json"), "unparseable bytes became a login");
    }

    /// The platform this app runs on, and the bug it caused.
    ///
    /// On macOS the CLI puts the token in the KEYCHAIN and leaves only its
    /// settings file in the config directory (`writeManagedCredentials`
    /// :210019, 1.4.164). A reader that knew only `.credentials.json` called
    /// every real login on every Mac "left no credentials" — the feature simply
    /// did not work on its primary platform, and no test noticed because a
    /// fixture can always write the file the reader was looking for.
    #[test]
    fn a_login_that_kept_its_token_in_the_keychain_is_still_a_login() {
        let dir = tempfile::tempdir().expect("no temp dir");
        std::fs::write(
            dir.path().join(CLAUDE_SETTINGS_FILE),
            r#"{"numStartups":3,"oauthAccount":{"emailAddress":"mac@example.com",
                "organizationUuid":"org-9","organizationName":"Example"}}"#,
        )
        .expect("write");
        assert!(
            signed_in(dir.path()),
            "a login whose token went to the keychain read as no login at all"
        );
        assert_eq!(
            identity_in(dir.path()).email.as_deref(),
            Some("mac@example.com")
        );
    }

    #[test]
    fn a_settings_file_with_no_account_in_it_is_not_a_login() {
        let dir = tempfile::tempdir().expect("no temp dir");
        // What an abandoned login leaves: the CLI ran, so its settings file
        // exists, but nobody signed in. Existence is why this is asked as "can
        // it name who" rather than "is there a file".
        std::fs::write(
            dir.path().join(CLAUDE_SETTINGS_FILE),
            r#"{"numStartups":1}"#,
        )
        .expect("write");
        assert!(!signed_in(dir.path()));
    }

    #[test]
    fn an_unreadable_file_yields_no_identity_rather_than_a_panic() {
        let dir = tempfile::tempdir().expect("no temp dir");
        std::fs::write(dir.path().join(CREDENTIALS_FILE), "not json at all").expect("write");
        assert!(!identity_in(dir.path()).is_namable());
        // And a later file still gets its turn — one corrupt file must not
        // hide a good one behind it.
        std::fs::write(
            dir.path().join(CLAUDE_CONFIG_FILE),
            r#"{"oauthAccount":{"email":"late@example.com"}}"#,
        )
        .expect("write");
        assert_eq!(
            identity_in(dir.path()).email.as_deref(),
            Some("late@example.com")
        );
    }

    #[test]
    fn an_agent_with_no_account_mechanism_gets_no_environment() {
        let config = tempfile::tempdir().expect("no config dir");
        // Reads no state: the guard is the agent id, so this holds on any
        // machine whatever it has stored.
        assert!(
            launch_env_for(config.path(), "codex")
                .expect("codex has no account mechanism")
                .is_empty()
        );
    }

    /// A store with one account in it, and the account's own directory.
    fn one_account(config_root: &Path, id: &str, email: &str, token: &str) -> ClaudeAccount {
        let dir = config_root.join("stores").join(id);
        std::fs::create_dir_all(&dir).expect("store dir");
        std::fs::write(
            dir.join(CREDENTIALS_FILE),
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}},"oauthAccount":{{"emailAddress":"{email}"}}}}"#),
        )
        .expect("credentials");
        std::fs::write(
            dir.join(CLAUDE_SETTINGS_FILE),
            format!(r#"{{"oauthAccount":{{"emailAddress":"{email}"}}}}"#),
        )
        .expect("settings");
        let account = ClaudeAccount {
            id: id.to_string(),
            email: email.to_string(),
            organization_uuid: None,
            organization_name: None,
            config_dir: dir.to_string_lossy().into_owned(),
            added_at: 1_700_000_000_000,
            ..ClaudeAccount::default()
        };
        let mut store = read_store(config_root);
        store.accounts.push(account.clone());
        write_store(config_root, &store).expect("store");
        account
    }

    fn cli_writes_credentials(dir: &Path, credentials: &str) {
        #[cfg(target_os = "macos")]
        {
            let service = keychain_services(dir).remove(0);
            let user = this_machines_account();
            security_command(
                &[
                    "add-generic-password",
                    "-U",
                    "-A",
                    "-s",
                    &service,
                    "-a",
                    &user,
                    "-w",
                ],
                Some(credentials),
            )
            .expect("CLI keychain write");
        }
        #[cfg(not(target_os = "macos"))]
        std::fs::write(dir.join(CREDENTIALS_FILE), credentials).expect("CLI credential write");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_macos_account_prefers_its_complete_keychain_login_over_a_stale_file() {
        let config = tempfile::tempdir().expect("no config dir");
        let account = one_account(config.path(), "a-1", "one@example.com", "stale");
        let dir = Path::new(&account.config_dir);
        let service = keychain_services(dir).remove(0);
        let user = this_machines_account();
        let complete = r#"{"claudeAiOauth":{"accessToken":"fresh","refreshToken":"refresh","expiresAt":1999999999999}}"#;
        security_command(
            &[
                "add-generic-password",
                "-U",
                "-A",
                "-s",
                &service,
                "-a",
                &user,
                "-w",
            ],
            Some(complete),
        )
        .expect("seed keychain");

        assert_eq!(credentials_at(dir).as_deref(), Some(complete));
    }

    /// A usage read asks with the login the CLI refreshed — the item scoped to
    /// the store the reading environment names — ahead of the copy the runtime
    /// home was handed at the last switch.
    ///
    /// The live state this was written from (t-6583, 2026-09-24): the runtime
    /// home's file and its own scoped item held a token that had expired some
    /// 260 minutes earlier and the endpoint answered 401 on every read, while
    /// the item scoped to the selected account's directory — where the CLI
    /// keeps refreshing — answered 200.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_usage_read_asks_with_the_login_the_cli_refreshed() {
        let written =
            r#"{"claudeAiOauth":{"accessToken":"written-at-switch","refreshToken":"r-1"}}"#;
        let refreshed =
            r#"{"claudeAiOauth":{"accessToken":"refreshed-by-the-cli","refreshToken":"r-2"}}"#;
        let config = tempfile::tempdir().expect("no config dir");
        let account = one_account(config.path(), "u-1", "usage@example.com", "stored-at-add");
        let store = PathBuf::from(&account.config_dir);
        let runtime = runtime_home(config.path());
        // What the last switch left: the runtime home's file and its own item.
        write_private(&runtime.join(CREDENTIALS_FILE), written).expect("runtime file");
        write_keychain(&runtime, written).expect("runtime item");
        // And where the CLI has refreshed since: the selected store's item.
        write_keychain(&store, refreshed).expect("store item");

        let env = reading_env_for(config.path(), "claude");
        assert_eq!(
            usage_login(&env),
            Some((refreshed.to_string(), LoginFrom::Keychain)),
            "the usage read asked with the copy the last switch wrote"
        );

        // A store whose item says nothing — never seeded, or a keychain that
        // did not answer — reads the runtime file, and the look writes nothing
        // on the way: the item is as missing afterwards as before.
        let bare_config = tempfile::tempdir().expect("no config dir");
        let bare = one_account(bare_config.path(), "u-2", "bare@example.com", "stored");
        write_private(
            &runtime_home(bare_config.path()).join(CREDENTIALS_FILE),
            written,
        )
        .expect("runtime file");
        let bare_store = Path::new(&bare.config_dir);
        assert!(matches!(keychain_says(bare_store), KeychainSays::Missing));
        assert_eq!(
            usage_login(&reading_env_for(bare_config.path(), "claude")),
            Some((written.to_string(), LoginFrom::File))
        );
        assert!(
            matches!(keychain_says(bare_store), KeychainSays::Missing),
            "a usage look seeded the store's keychain item"
        );
    }

    /// The person's own login is never what a usage read asks with: not the
    /// unsuffixed item their terminal's CLI keeps, and not anything at all
    /// once they chose the system default — whatever this window's own homes
    /// hold (t-6583, condition 2).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_usage_read_never_asks_with_the_persons_own_login() {
        let own = r#"{"claudeAiOauth":{"accessToken":"the-persons-own","refreshToken":"r-own"}}"#;
        security_command(
            &[
                "add-generic-password",
                "-U",
                "-s",
                "Claude Code-credentials",
                "-a",
                &this_machines_account(),
                "-w",
                own,
            ],
            None,
        )
        .expect("the person's own item");
        // A store and a runtime home with nothing of their own do not borrow it.
        let config = tempfile::tempdir().expect("no config dir");
        let _account = one_account(config.path(), "p-1", "person@example.com", "stored");
        assert_eq!(usage_login(&reading_env_for(config.path(), "claude")), None);

        // And the system default reads nothing, even with a live login in the
        // store and a copy in the runtime home.
        let chosen = tempfile::tempdir().expect("no config dir");
        let held = one_account(chosen.path(), "p-2", "chosen@example.com", "stored");
        write_keychain(Path::new(&held.config_dir), own).expect("store item");
        write_private(&runtime_home(chosen.path()).join(CREDENTIALS_FILE), own)
            .expect("runtime file");
        assert!(usage_login(&reading_env_for(chosen.path(), "claude")).is_some());
        use_system_default(chosen.path()).expect("system default");
        let env = reading_env_for(chosen.path(), "claude");
        assert!(env.is_empty(), "the system default named a home: {env:?}");
        assert_eq!(usage_login(&env), None);
    }

    #[test]
    fn the_shared_runtime_inherits_completed_first_run_answers() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no runtime home");
        let account = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        std::fs::write(
            Path::new(&account.config_dir).join(CLAUDE_SETTINGS_FILE),
            r#"{"oauthAccount":{"emailAddress":"one@example.com"},"theme":"dark-daltonized","hasCompletedOnboarding":true,"lastOnboardingVersion":"2.1.222"}"#,
        )
        .expect("source settings");

        materialize_into(config.path(), &account, home.path()).expect("materialize");
        let runtime: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(CLAUDE_SETTINGS_FILE))
                .expect("runtime settings"),
        )
        .expect("runtime json");
        assert_eq!(runtime["theme"], "dark-daltonized");
        assert_eq!(runtime["hasCompletedOnboarding"], true);
        assert_eq!(runtime["lastOnboardingVersion"], "2.1.222");
    }

    #[test]
    fn an_old_runtime_record_cannot_bless_stale_credentials() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no runtime home");
        let account = one_account(config.path(), "a-1", "one@example.com", "fresh");
        let stale = r#"{"claudeAiOauth":{"accessToken":"stale"}}"#;
        std::fs::write(home.path().join(CREDENTIALS_FILE), stale).expect("stale runtime");
        set_oauth_block(
            &home.path().join(CLAUDE_SETTINGS_FILE),
            stored_identity(Path::new(&account.config_dir)).as_ref(),
        );
        write_runtime(
            config.path(),
            &RuntimeAuth {
                home: Some(home.path().to_string_lossy().into_owned()),
                account: Some(account.id.clone()),
                written: Some(stale.to_string()),
                gathered: true,
                ..RuntimeAuth::default()
            },
        );

        materialize_into(config.path(), &account, home.path()).expect("repair old runtime");
        let repaired =
            std::fs::read_to_string(home.path().join(CREDENTIALS_FILE)).expect("runtime");
        assert!(
            repaired.contains("fresh"),
            "a legacy record treated stale runtime credentials as current: {repaired}"
        );
    }

    /// No account selected still means OUR home, never the person's.
    ///
    /// An empty answer here is not neutral — it is a decision to run in somebody
    /// else's home. With nothing selected this used to return no environment at
    /// all, so the launch inherited the ambient one and the agent ran against
    /// `~/.claude`: the person's own conversations, and on macOS the person's own
    /// keychain item, which this window never created. That is how an
    /// authorization dialog arrived and kept arriving even with the app closed —
    /// the CLI reads that item through `security`, and our earlier writes had
    /// taken `security` off its access list.
    ///
    /// The reading door and the launch door are both checked, because they
    /// return by different roads and only one of them was obviously wrong.
    ///
    /// This is NOT the system default below. Nothing chosen yet and 「이 기기의
    /// 로그인을 쓴다」 are different statements, and the whole reason that row
    /// carries a flag of its own is that they were being given one answer.
    #[test]
    fn no_account_selected_still_names_the_app_owned_home() {
        let config = tempfile::tempdir().expect("no config dir");
        // Deliberately empty: a store with no accounts and no selection.
        write_store(config.path(), &AccountStore::default()).expect("store");
        let expected = runtime_home(config.path()).to_string_lossy().into_owned();

        for (road, env) in [
            (
                "launch",
                launch_env_for(config.path(), "claude").expect("empty managed home"),
            ),
            ("reading", reading_env_for(config.path(), "claude")),
        ] {
            for variable in [
                zerocode_core::account::CONFIG_DIR_VAR,
                zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR,
            ] {
                let named = env
                    .iter()
                    .find(|(key, _)| key == variable)
                    .map(|(_, value)| value.clone());
                assert_eq!(
                    named.as_deref(),
                    Some(expected.as_str()),
                    "the {road} door left `{variable}` pointing at the person's Claude home"
                );
            }
        }

        // And an agent with no account mechanism is still left alone entirely.
        assert!(reading_env_for(config.path(), "codex").is_empty());
    }

    /// Conversations stay in the one app-owned home while credentials stay in
    /// the selected account's own store.  Giving both jobs to
    /// `CLAUDE_CONFIG_DIR` makes an in-session `/login` land in the shared home;
    /// the next launch then materializes the selected account over it and the
    /// fresh login appears to expire on restart.
    #[test]
    fn a_selected_account_separates_conversations_from_secure_storage() {
        let config = tempfile::tempdir().expect("no config dir");
        let account = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        select_account_with_probe(config.path(), "a-1", |_| Some(true)).expect("select");

        let env = launch_env_for(config.path(), "claude").expect("selected account");
        let value = |name: &str| {
            env.iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(
            value(zerocode_core::account::CONFIG_DIR_VAR),
            runtime_home(config.path()).to_str(),
            "the shared conversation home changed with the account"
        );
        assert_eq!(
            value(zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR),
            Some(account.config_dir.as_str()),
            "the selected credential store was not handed to Claude"
        );
    }

    /// The system default is a CHOICE, and choosing it makes this window
    /// stop taking part.
    ///
    /// The original lists it first and calls the managed accounts 선택 사항 —
    /// "Orca는 일반적인 Claude 로그인을 사용할 수 있습니다 … 빠르게 전환하려는
    /// 경우에만 계정을 추가하세요". So there has to be a way to SAY it, and it
    /// cannot be the same answer as the test above: nothing added yet is a gap,
    /// while this is the person saying "use mine, stay out".
    ///
    /// The load-bearing half is the SECOND assertion. Leaving a launch in that
    /// home is harmless — every injury this feature has caused came from
    /// WRITING it: a login overwritten on every launch, and a keychain access
    /// list rewritten so the person's own CLI was dropped off it and asked for
    /// a password forever after. So this measures that nothing is written.
    #[test]
    fn the_system_default_is_chosen_by_a_row_and_writes_nothing() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let account = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        materialize_into(config.path(), &account, home.path()).expect("first");
        select_account_with_probe(config.path(), "a-1", |_| Some(true)).expect("select");

        let live = home.path().join(CREDENTIALS_FILE);
        let settings = home.path().join(CLAUDE_SETTINGS_FILE);
        let before = (
            std::fs::read_to_string(&live).expect("live"),
            std::fs::read_to_string(&settings).expect("settings"),
        );

        // Back to the machine's own login.
        let store = use_system_default(config.path()).expect("system default");
        assert_eq!(
            store.selection.active, None,
            "the selection was not cleared"
        );

        // 1. No environment travels — an override would put an agent in a home
        //    the person did not pick.
        for (road, env) in [
            (
                "launch",
                launch_env_for(config.path(), "claude").expect("system default"),
            ),
            ("reading", reading_env_for(config.path(), "claude")),
        ] {
            assert!(
                env.is_empty(),
                "the {road} door still names a home after the system default \
                 was chosen: {env:?}"
            );
        }

        // 2. And nothing was written to the runtime home on the way.
        assert_eq!(
            before.0,
            std::fs::read_to_string(&live).expect("live"),
            "choosing the system default rewrote a credentials file"
        );
        assert_eq!(
            before.1,
            std::fs::read_to_string(&settings).expect("settings"),
            "choosing the system default rewrote an identity block"
        );

        // 3. And a real account still switches afterwards, so the row is a
        //    choice rather than a one-way door.
        select_account_with_probe(config.path(), "a-1", |_| Some(true)).expect("select again");
        assert!(
            launch_env_for(config.path(), "claude")
                .expect("selected account")
                .iter()
                .any(|(key, _)| key == zerocode_core::account::CONFIG_DIR_VAR),
            "a managed account no longer names the app-owned home"
        );
    }

    #[test]
    fn every_selected_account_launches_in_the_same_app_owned_home() {
        let config = tempfile::tempdir().expect("no config dir");
        one_account(config.path(), "a-1", "one@example.com", "tok-one");
        one_account(config.path(), "a-2", "two@example.com", "tok-two");
        let expected = runtime_home(config.path()).to_string_lossy().into_owned();

        for id in ["a-1", "a-2"] {
            select_account_with_probe(config.path(), id, |_| Some(true)).expect("switch");
            let env = launch_env_for(config.path(), "claude").expect("selected account");
            assert_eq!(
                env.iter()
                    .find(|(key, _)| key == zerocode_core::account::CONFIG_DIR_VAR)
                    .map(|(_, value)| value.as_str()),
                Some(expected.as_str()),
                "{id} opened a different conversation home"
            );
        }
    }

    #[test]
    fn selecting_the_same_row_applies_a_cli_refreshed_login_before_success() {
        let config = tempfile::tempdir().expect("no config dir");
        let account = one_account(config.path(), "a-1", "one@example.com", "old-login");
        select_account_with_probe(config.path(), "a-1", |_| Some(true)).expect("first select");

        let refreshed = r#"{"claudeAiOauth":{"accessToken":"refreshed-login"}}"#;
        cli_writes_credentials(Path::new(&account.config_dir), refreshed);
        let runtime = runtime_home(config.path());
        assert!(
            !std::fs::read_to_string(runtime.join(CREDENTIALS_FILE))
                .expect("old runtime")
                .contains("refreshed-login"),
            "the fixture no longer carries a stale but non-empty runtime"
        );

        let selected_dir = PathBuf::from(&account.config_dir);
        let runtime_for_probe = runtime.clone();
        let store = select_account_with_probe(config.path(), "a-1", |dir| {
            if dir == selected_dir {
                return Some(true);
            }
            if dir == runtime_for_probe {
                return Some(
                    std::fs::read_to_string(runtime_for_probe.join(CREDENTIALS_FILE))
                        .ok()
                        .is_some_and(|text| text.contains("refreshed-login")),
                );
            }
            None
        })
        .expect("live switch");

        assert_eq!(store.selection.active.as_deref(), Some("a-1"));
        assert!(
            std::fs::read_to_string(runtime.join(CREDENTIALS_FILE))
                .expect("refreshed runtime")
                .contains("refreshed-login"),
            "selection was reported before the refreshed login reached the runtime"
        );
    }

    #[test]
    fn a_failed_live_switch_restores_the_previous_runtime_and_selection() {
        let config = tempfile::tempdir().expect("no config dir");
        one_account(config.path(), "a-1", "one@example.com", "login-one");
        let second = one_account(config.path(), "a-2", "two@example.com", "login-two");
        select_account_with_probe(config.path(), "a-1", |_| Some(true)).expect("first select");

        let second_dir = PathBuf::from(&second.config_dir);
        let runtime = runtime_home(config.path());
        let error = select_account_with_probe(config.path(), "a-2", |dir| {
            if dir == second_dir {
                Some(true)
            } else {
                Some(false)
            }
        })
        .expect_err("runtime verification should fail");

        assert!(error.contains("적용하지 못했습니다"), "{error}");
        assert_eq!(
            read_store(config.path()).selection.active.as_deref(),
            Some("a-1"),
            "a failed switch changed the durable selection"
        );
        assert!(
            std::fs::read_to_string(runtime.join(CREDENTIALS_FILE))
                .expect("restored runtime")
                .contains("login-one"),
            "a failed switch left the new account in the shared runtime"
        );
    }

    #[test]
    fn an_unattended_worker_repairs_runtime_drift_from_the_live_selected_account() {
        let config = tempfile::tempdir().expect("no config dir");
        let account = one_account(config.path(), "a-1", "one@example.com", "old-login");
        select_account_with_probe(config.path(), "a-1", |_| Some(true)).expect("first select");

        let refreshed = r#"{"claudeAiOauth":{"accessToken":"worker-refresh"}}"#;
        cli_writes_credentials(Path::new(&account.config_dir), refreshed);
        let env = launch_env_for(config.path(), "claude").expect("managed launch env");
        let runtime = runtime_home(config.path());
        let selected_dir = PathBuf::from(&account.config_dir);
        let runtime_for_probe = runtime.clone();

        require_unattended_login_with_probe(config.path(), "claude", &env, |dir| {
            if dir == selected_dir {
                return Some(true);
            }
            if dir == runtime_for_probe {
                return Some(
                    std::fs::read_to_string(runtime_for_probe.join(CREDENTIALS_FILE))
                        .ok()
                        .is_some_and(|text| text.contains("worker-refresh")),
                );
            }
            None
        })
        .expect("runtime repair");

        assert!(
            std::fs::read_to_string(runtime.join(CREDENTIALS_FILE))
                .expect("worker runtime")
                .contains("worker-refresh"),
            "the worker preflight accepted the expired shared runtime"
        );
    }

    #[test]
    fn a_refused_switch_is_neither_reported_nor_saved_as_selected() {
        let config = tempfile::tempdir().expect("no config dir");
        let dir = config.path().join("stores").join("a-empty");
        std::fs::create_dir_all(&dir).expect("store dir");
        std::fs::write(
            dir.join(CLAUDE_SETTINGS_FILE),
            r#"{"oauthAccount":{"emailAddress":"empty@example.com"}}"#,
        )
        .expect("settings");
        write_store(
            config.path(),
            &AccountStore {
                accounts: vec![ClaudeAccount {
                    id: "a-empty".to_string(),
                    email: "empty@example.com".to_string(),
                    organization_uuid: None,
                    organization_name: None,
                    config_dir: dir.to_string_lossy().into_owned(),
                    added_at: 1,
                    ..ClaudeAccount::default()
                }],
                selection: AccountSelection::default(),
            },
        )
        .expect("store");

        assert!(
            select_account_with_probe(config.path(), "a-empty", |_| Some(true)).is_err(),
            "a switch with no readable login was reported as successful"
        );
        assert_eq!(
            read_store(config.path()).selection.active,
            None,
            "a failed switch was saved as the active account"
        );
        assert!(
            launch_env_for(config.path(), "claude").is_err(),
            "a damaged managed account was launched in an unauthenticated home"
        );
    }

    /// A login the person made THEMSELVES is filed under whoever it names.
    ///
    /// Invariant 1 preserves what the CLI wrote before overwriting it, and the
    /// original files that under whichever account was last materialized — right
    /// for a token refresh, which keeps the same identity. But somebody can also
    /// run `/login` and end up signed in as a different account of their own, and
    /// then the recorded owner is simply the wrong drawer. This machine was in
    /// exactly that state: the record said one account while `~/.claude.json`
    /// named another. Filing by the record would have handed the second person's
    /// credentials out under the first person's name on the next switch.
    #[test]
    fn a_login_somebody_made_themselves_is_filed_under_the_account_it_names() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let first = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        let second = one_account(config.path(), "a-2", "two@example.com", "tok-two");

        materialize_into(config.path(), &first, home.path()).expect("first");

        // The person logs in as the OTHER account by hand: new token, and the
        // settings file names them now.
        cli_writes_credentials(
            home.path(),
            r#"{"claudeAiOauth":{"accessToken":"fresh-by-hand"}}"#,
        );
        set_oauth_block(
            &home.path().join(CLAUDE_SETTINGS_FILE),
            stored_identity(Path::new(&second.config_dir)).as_ref(),
        );

        // Now a switch back to the first account runs.
        materialize_into(config.path(), &first, home.path()).expect("switch back");

        let filed_under_second =
            std::fs::read_to_string(Path::new(&second.config_dir).join(CREDENTIALS_FILE))
                .unwrap_or_default();
        assert!(
            filed_under_second.contains("fresh-by-hand"),
            "the hand-made login was not kept for the account it names: \
             {filed_under_second}"
        );
        let first_store =
            std::fs::read_to_string(Path::new(&first.config_dir).join(CREDENTIALS_FILE))
                .unwrap_or_default();
        assert!(
            !first_store.contains("fresh-by-hand"),
            "one person's login was filed in another person's drawer: {first_store}"
        );
    }

    /// Materializing the account that is ALREADY there writes nothing.
    ///
    /// Not an optimization. Every write on this road goes to the keychain too,
    /// and a keychain write is the one call here that can put a password dialog
    /// on somebody's screen — so an unguarded repeat means the person is asked
    /// again every time an agent launches ("키체인이 계속"). Measured at the
    /// files rather than at the code, because the code has looked right through
    /// two versions of this bug.
    #[test]
    fn the_account_already_in_the_home_is_left_completely_alone() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let only = one_account(config.path(), "a-1", "one@example.com", "tok-one");

        materialize_into(config.path(), &only, home.path()).expect("first");
        let live = home.path().join(CREDENTIALS_FILE);
        let settings = home.path().join(CLAUDE_SETTINGS_FILE);
        let stamped = (
            std::fs::metadata(&live)
                .expect("live")
                .modified()
                .expect("t"),
            std::fs::metadata(&settings)
                .expect("settings")
                .modified()
                .expect("t"),
        );

        // A launch, then another. Neither is a switch, so neither may write.
        materialize_into(config.path(), &only, home.path()).expect("again");
        materialize_into(config.path(), &only, home.path()).expect("and again");

        assert_eq!(
            stamped.0,
            std::fs::metadata(&live)
                .expect("live")
                .modified()
                .expect("t"),
            "the unchanged login was written again"
        );
        assert_eq!(
            stamped.1,
            std::fs::metadata(&settings)
                .expect("settings")
                .modified()
                .expect("t"),
            "the unchanged identity block was written again"
        );

        // And a REAL switch still takes, which is what makes the skip a skip
        // rather than a wall.
        let other = one_account(config.path(), "a-2", "two@example.com", "tok-two");
        materialize_into(config.path(), &other, home.path()).expect("switch");
        let now = std::fs::read_to_string(&live).expect("read");
        assert!(now.contains("tok-two"), "the switch did not take: {now}");
    }

    #[test]
    fn an_unchanged_runtime_does_not_reopen_the_account_store() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let only = one_account(config.path(), "a-1", "one@example.com", "tok-one");

        materialize_into(config.path(), &only, home.path()).expect("first");
        std::fs::remove_file(Path::new(&only.config_dir).join(CREDENTIALS_FILE))
            .expect("remove source credentials");

        materialize_into(config.path(), &only, home.path())
            .expect("an already-materialized launch should not need to reopen the account store");
        let live = std::fs::read_to_string(home.path().join(CREDENTIALS_FILE)).expect("runtime");
        assert!(
            live.contains("tok-one"),
            "the runtime login changed: {live}"
        );
    }

    #[test]
    fn relogin_invalidates_only_the_matching_materialized_account() {
        let config = tempfile::tempdir().expect("no config dir");
        write_runtime(
            config.path(),
            &RuntimeAuth {
                home: Some("/app/.claude".to_string()),
                account: Some("a-1".to_string()),
                written: Some("old-token".to_string()),
                gathered: true,
                ..RuntimeAuth::default()
            },
        );

        invalidate_materialized_account(config.path(), "a-2");
        assert_eq!(read_runtime(config.path()).account.as_deref(), Some("a-1"));

        invalidate_materialized_account(config.path(), "a-1");
        let state = read_runtime(config.path());
        assert!(state.account.is_none());
        assert!(state.written.is_none());
        assert!(
            state.gathered,
            "relogin repeated the conversation migration"
        );
    }

    /// Switching accounts moves the LOGIN, and leaves the conversations alone.
    ///
    /// The reported defect, at the layer that caused it: an account was a whole
    /// `CLAUDE_CONFIG_DIR`, so switching switched the drawer the person's
    /// history was read from and their conversations vanished. The person's
    /// need is the plain one — "세션이 끊기면 계정만 바꿔서 이어가져야" — and it
    /// only works if the history stays where it is while the login moves.
    #[test]
    fn switching_an_account_moves_the_login_and_never_the_conversations() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let first = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        let second = one_account(config.path(), "a-2", "two@example.com", "tok-two");

        // A pre-existing runtime login, and a conversation already in the one home.
        std::fs::write(home.path().join(CREDENTIALS_FILE), r#"{"mine":true}"#).expect("write");
        let talk = home.path().join("projects").join("-w-a").join("s-1.jsonl");
        std::fs::create_dir_all(talk.parent().expect("parent")).expect("projects");
        std::fs::write(&talk, "{\"kind\":\"user\"}\n").expect("write");

        materialize_into(config.path(), &first, home.path()).expect("first");
        let now = std::fs::read_to_string(home.path().join(CREDENTIALS_FILE)).expect("read");
        assert!(now.contains("tok-one"), "the login did not move: {now}");
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(CLAUDE_SETTINGS_FILE)).expect("read"),
        )
        .expect("json");
        assert_eq!(settings["oauthAccount"]["emailAddress"], "one@example.com");

        materialize_into(config.path(), &second, home.path()).expect("second");
        let now = std::fs::read_to_string(home.path().join(CREDENTIALS_FILE)).expect("read");
        assert!(now.contains("tok-two"), "the switch did not take: {now}");

        // And the whole point: the conversation is still there, under the same
        // name, after two switches. This is the assertion the defect fails.
        assert!(talk.is_file(), "switching accounts took the conversations");
    }

    /// What the CLI rotated goes back to the account it belongs to.
    ///
    /// The token is single-use. If a refresh the CLI performed is overwritten
    /// by the copy in our store, that account is holding a token that has
    /// already been spent — which is a login that fails the next time it is
    /// picked, with nothing on screen to say why.
    #[test]
    fn a_token_the_cli_rotated_is_kept_before_the_next_login_is_written() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let first = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        let second = one_account(config.path(), "a-2", "two@example.com", "tok-two");

        materialize_into(config.path(), &first, home.path()).expect("first");
        // The CLI refreshes, in the one home, behind our back.
        cli_writes_credentials(
            home.path(),
            r#"{"claudeAiOauth":{"accessToken":"tok-one-refreshed"},"oauthAccount":{"emailAddress":"one@example.com"}}"#,
        );
        materialize_into(config.path(), &second, home.path()).expect("second");

        let kept = std::fs::read_to_string(Path::new(&first.config_dir).join(CREDENTIALS_FILE))
            .expect("read");
        assert!(
            kept.contains("tok-one-refreshed"),
            "the refreshed token was thrown away: {kept}"
        );
    }

    #[test]
    fn a_refresh_of_the_selected_account_remains_the_live_login() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let account = one_account(config.path(), "a-1", "one@example.com", "tok-one");

        materialize_into(config.path(), &account, home.path()).expect("first");
        cli_writes_credentials(
            home.path(),
            r#"{"claudeAiOauth":{"accessToken":"tok-one-refreshed"},"oauthAccount":{"emailAddress":"one@example.com"}}"#,
        );

        materialize_into(config.path(), &account, home.path()).expect("read back refresh");
        let live = std::fs::read_to_string(home.path().join(CREDENTIALS_FILE)).expect("runtime");
        assert!(
            live.contains("tok-one-refreshed"),
            "read-back persisted the refresh and then replaced it with the stale token: {live}"
        );
        let kept = credentials_at(Path::new(&account.config_dir)).expect("store login");
        assert!(
            kept.contains("tok-one-refreshed"),
            "the scoped store kept the pre-refresh login"
        );
    }

    /// An account with nothing readable in it is refused, and changes nothing.
    #[test]
    fn an_account_with_no_login_is_refused_and_leaves_the_home_alone() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        std::fs::write(home.path().join(CREDENTIALS_FILE), r#"{"mine":true}"#).expect("write");
        let empty = ClaudeAccount {
            id: "a-9".to_string(),
            email: "gone@example.com".to_string(),
            organization_uuid: None,
            organization_name: None,
            config_dir: config.path().join("nowhere").to_string_lossy().into_owned(),
            added_at: 1_700_000_000_000,
            ..ClaudeAccount::default()
        };
        assert!(materialize_into(config.path(), &empty, home.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(home.path().join(CREDENTIALS_FILE)).expect("read"),
            r#"{"mine":true}"#,
            "a refusal still touched the one home"
        );
    }

    /// Every account's past comes home, and nothing already there is touched.
    ///
    /// The clause that makes the person's own need work — "세션이 끊기면 계정만
    /// 바꿔서 이어가져야" — because a conversation recorded while an account was
    /// a config directory is one a launch reading the single home would
    /// otherwise never find again.
    #[test]
    fn every_accounts_conversations_come_home_and_none_is_overwritten() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let first = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        let second = one_account(config.path(), "a-2", "two@example.com", "tok-two");

        let put = |dir: &str, slug: &str, name: &str, text: &str| {
            let at = Path::new(dir).join("projects").join(slug).join(name);
            std::fs::create_dir_all(at.parent().expect("parent")).expect("dirs");
            std::fs::write(&at, text).expect("write");
            at
        };
        let moved_one = put(&first.config_dir, "-w-a", "s-1.jsonl", "one\n");
        let moved_two = put(&second.config_dir, "-w-b", "s-2.jsonl", "two\n");
        // And one the person already has in the one home under the same name.
        let clash = put(&first.config_dir, "-w-a", "s-3.jsonl", "the old copy\n");
        let standing = home.path().join("projects").join("-w-a").join("s-3.jsonl");
        std::fs::create_dir_all(standing.parent().expect("parent")).expect("dirs");
        std::fs::write(&standing, "the one already here\n").expect("write");

        materialize_into(config.path(), &first, home.path()).expect("materialize");

        for (was, slug, name, text) in [
            (&moved_one, "-w-a", "s-1.jsonl", "one\n"),
            (&moved_two, "-w-b", "s-2.jsonl", "two\n"),
        ] {
            let now = home.path().join("projects").join(slug).join(name);
            assert_eq!(
                std::fs::read_to_string(&now).ok().as_deref(),
                Some(text),
                "{name} did not come home"
            );
            assert!(!was.exists(), "{name} was left behind as a second copy");
        }
        // The one already there is untouched, and its twin is left beside it
        // rather than thrown away — a collision means the same conversation.
        assert_eq!(
            std::fs::read_to_string(&standing).expect("read"),
            "the one already here\n"
        );
        assert!(clash.exists(), "a file that could not move was deleted");

        // Once. A second materialization finds nothing left to do, and must not
        // start moving the file it just declined to overwrite.
        std::fs::write(&standing, "edited since\n").expect("write");
        materialize_into(config.path(), &second, home.path()).expect("again");
        assert_eq!(
            std::fs::read_to_string(&standing).expect("read"),
            "edited since\n"
        );
    }

    #[test]
    fn the_terminal_history_is_seeded_without_becoming_app_owned() {
        let external = tempfile::tempdir().expect("external home");
        let runtime = tempfile::tempdir().expect("runtime home");
        let source = external
            .path()
            .join("projects")
            .join("-w-a")
            .join("s-1.jsonl");
        std::fs::create_dir_all(source.parent().expect("parent")).expect("projects");
        std::fs::write(&source, "external\n").expect("source");

        assert_eq!(copy_conversations(external.path(), runtime.path()), 1);
        let copied = runtime
            .path()
            .join("projects")
            .join("-w-a")
            .join("s-1.jsonl");
        assert_eq!(
            std::fs::read_to_string(&copied).expect("copied history"),
            "external\n"
        );
        assert!(
            source.is_file(),
            "seeding removed the terminal's own history"
        );

        std::fs::write(&source, "terminal moved on\n").expect("change external");
        assert_eq!(
            std::fs::read_to_string(&copied).expect("copied history"),
            "external\n",
            "the two histories remained linked after the one-time seed"
        );
    }

    /// A Zerocode-owned home answers only to its scoped service. The
    /// unsuffixed service belongs to a person's ordinary terminal and writing
    /// it would make an in-app account switch change that terminal too.
    #[test]
    fn a_managed_home_never_answers_to_the_global_service() {
        let named = keychain_services(Path::new("/Users/j/zerocode/.claude"));
        assert_eq!(named.len(), 1);
        assert!(
            named[0].starts_with("Claude Code-credentials-") && named[0].len() == 32,
            "the scoped service is not eight hex after the name: {named:?}"
        );
        assert!(
            !named
                .iter()
                .any(|service| service == "Claude Code-credentials")
        );
        // Same directory, same name — a switch must reach the entry the last
        // one wrote.
        assert_eq!(
            named,
            keychain_services(Path::new("/Users/j/zerocode/.claude"))
        );
        assert_ne!(named[0], keychain_services(Path::new("/Users/j/.other"))[0]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_keychain_write_supplies_the_noninteractive_w_value() {
        let home = tempfile::tempdir().expect("runtime home");
        let credentials = r#"{"claudeAiOauth":{"accessToken":"fixture"}}"#;

        write_keychain(home.path(), credentials).expect("noninteractive keychain write");
        assert_eq!(keychain_says(home.path()).login(), Some(credentials));
    }

    /// 2026-09-13, "security 종료 45": the item was already standing when the
    /// add ran. A write over an existing item replaces it — that is what the
    /// delete before the add is for, and the fake tool now refuses a bare
    /// second add the way the real one does.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_write_over_an_existing_item_replaces_it() {
        let service = "zerocode-test-duplicate-item";
        write_keychain_service(service, "first").expect("first write");
        write_keychain_service(service, "second").expect("write over the existing item");
        assert_eq!(read_keychain_service(service).as_deref(), Ok("second"));
    }

    /// The delete before the add can itself be refused — a locked keychain, an
    /// access list that does not name `security`. The add then meets the old
    /// item (exit 45); one more delete-and-add lands the login, and it is the
    /// delete's own refusal, not "duplicate", that a second refusal would name.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_write_whose_delete_was_refused_once_still_lands() {
        let service = "zerocode-test-pinned-item";
        let user = this_machines_account();
        write_keychain_service(service, "stale").expect("seed");
        security_command(&["test-pin-item", "-s", service, "-a", &user], None).expect("pin");
        write_keychain_service(service, "fresh").expect("the retry lands the write");
        assert_eq!(read_keychain_service(service).as_deref(), Ok("fresh"));
    }

    /// Writers from several threads — a restart resuming several panes of one
    /// account at once — all land, and none is refused as a duplicate.
    #[cfg(target_os = "macos")]
    #[test]
    fn concurrent_writers_of_one_service_are_never_refused_as_duplicates() {
        let service = "zerocode-test-concurrent-writers";
        let hands: Vec<_> = (0..8)
            .map(|hand| {
                std::thread::spawn(move || {
                    (0..25)
                        .map(|turn| write_keychain_service(service, &format!("hand-{hand}-{turn}")))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut refused = Vec::new();
        for hand in hands {
            for outcome in hand.join().expect("writer thread") {
                if let Err(error) = outcome {
                    refused.push(error);
                }
            }
        }
        assert!(refused.is_empty(), "{refused:?}");
        let held = read_keychain_service(service).expect("one item stands");
        assert!(held.starts_with("hand-"), "{held}");
    }

    /// A real Claude blob is 509–2040 bytes and a router table can be longer.
    /// The write takes the argument road, so nothing is cut at the prompt's
    /// 128 bytes (measured 2026-09-10).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_long_credential_round_trips_whole() {
        let service = "zerocode-test-long-credential";
        let long = format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{}"}}}}"#,
            "T".repeat(4096)
        );
        write_keychain_service(service, &long).expect("write");
        assert_eq!(read_keychain_service(service).as_deref(), Ok(long.as_str()));
    }

    /// A refusal names the tool's exit AND its sentence, so "종료 45" reads as a
    /// duplicate, a refused delete or a locked keychain — and never the secret,
    /// which `security` does not print.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_refused_security_run_is_named_by_code_and_sentence() {
        let duplicate = security_failure_word(
            Some(45),
            "security: SecKeychainItemCreateFromContent (<default>): The specified item already exists.\n",
        );
        assert!(
            duplicate.contains(DUPLICATE_ITEM) && duplicate.contains("already exists"),
            "{duplicate}"
        );
        assert_eq!(security_failure_word(Some(44), ""), NO_SUCH_ITEM);
        assert_eq!(
            security_failure_word(
                Some(36),
                "\nsecurity: SecKeychainSearchCopyNext: The user name or passphrase you entered is not correct.\n",
            ),
            "종료 36 — security: SecKeychainSearchCopyNext: The user name or passphrase you entered is not correct."
        );
        assert_eq!(security_failure_word(None, ""), "신호로 끝남");
    }

    /// A keychain holding a signed-out document is repaired, even when the file
    /// and our own record agree perfectly.
    ///
    /// The live state on this machine at 17:10 (`3823354`'s boot log): the home's
    /// item held a document with no token in it. Every clause of the skip was
    /// about the FILE or about our bookkeeping, and `on_disk` falls back to the
    /// file when the keychain has nothing to give — so the home looked
    /// materialized while the half the CLI actually reads on macOS was
    /// signed out. What the person sees is `Please run /login` in a window whose
    /// account panel says they are signed in.
    ///
    /// Both halves are measured, because the first is worthless without the
    /// second: the skip must refuse, AND the repair must put the login back.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_keychain_holding_a_signed_out_document_is_repaired_however_good_the_file_is() {
        let config = tempfile::tempdir().expect("no config dir");
        let home = tempfile::tempdir().expect("no home");
        let account = one_account(config.path(), "a-1", "one@example.com", "tok-one");
        materialize_into(config.path(), &account, home.path()).expect("first");

        // Exactly what a signed-out CLI leaves: the whole document, no token.
        let signed_out = r#"{"claudeAiOauth":{"accessToken":"","refreshToken":""}}"#;
        write_keychain(home.path(), signed_out).expect("poison");
        assert!(
            !holds_login(signed_out),
            "the fixture stopped being a signed-out document"
        );

        // The file and the record are untouched and still agree — this is the
        // shape that made the skip fire.
        let state = read_runtime(config.path());
        let on_file = std::fs::read_to_string(home.path().join(CREDENTIALS_FILE)).expect("file");
        assert_eq!(
            state.written.as_deref(),
            Some(on_file.as_str()),
            "the setup no longer reproduces a home that LOOKS materialized"
        );
        let (on_disk, says) = login_at(home.path());
        assert_eq!(
            on_disk.as_deref(),
            Some(on_file.as_str()),
            "the read stopped falling back to the file, so this measures nothing"
        );
        assert!(
            !already_materialized(
                &state,
                &account,
                on_disk.as_deref(),
                Some(on_file.as_str()),
                &says,
                &home.path().join(CLAUDE_SETTINGS_FILE),
                Path::new(&account.config_dir),
            ),
            "a home whose keychain is signed out was called already materialized"
        );

        materialize_into(config.path(), &account, home.path()).expect("repair");
        assert_eq!(
            keychain_says(home.path()).login(),
            Some(on_file.as_str()),
            "the repair did not put the login back into the half the CLI reads"
        );
    }

    /// And a keychain that says NOTHING is not evidence of anything.
    ///
    /// The opposite injury, and the reason this is a classification rather than
    /// a boolean: a locked or refusing keychain teaches us nothing about what it
    /// holds, and treating silence as a contradiction would put a keychain write
    /// on every single launch — the dialog storm the skip exists to prevent
    /// ("키체인이 계속").
    #[test]
    fn a_keychain_that_could_not_be_read_does_not_contradict_anything() {
        assert!(!KeychainSays::NoAnswer.contradicts_a_login());
        assert!(KeychainSays::NotALogin.contradicts_a_login());
        assert!(KeychainSays::Missing.contradicts_a_login());
        assert!(!KeychainSays::Login("x".into()).contradicts_a_login());
    }

    /// `$USER` and `$USERNAME` are not the only things that know who is running
    /// this, and the uid is the one that cannot be empty.
    ///
    /// MEASURED 2026-09-20 (w-4837): an exec'd child of the window had no
    /// `USER` at all, `LOGNAME=root`, and `joe` as the uid's own account.
    /// `LOGNAME` is deliberately not in the chain for exactly that reason — it
    /// was the variable that DID answer, and it answered with the wrong
    /// account. A third guess is not a third chance at the truth.
    #[test]
    fn an_environment_that_names_nobody_still_asks_this_uids_own_account() {
        let uid_account = account_of_this_uid().expect("this uid has an account of its own");
        assert!(
            !uid_account.is_empty(),
            "the uid road answered with nothing"
        );
        assert_eq!(
            keychain_user_from(|_| None, account_of_this_uid),
            Some(uid_account.clone()),
            "an environment that names nobody never reached the uid"
        );
        // The environment still wins while it names somebody: these items are
        // the CLI's, and the CLI files them under `$USER` (`keychain.ts:80-82`),
        // so a uid consulted ahead of it would ask about a different account.
        assert_eq!(
            keychain_user_from(
                |name| (name == "USER").then(|| "from-the-environment".to_string()),
                account_of_this_uid
            ),
            Some("from-the-environment".to_string())
        );
        // A variable set to nothing is not a name either — `-a ""` is as
        // invented an account as `-a user` is.
        assert_eq!(
            keychain_user_from(
                |name| (name == "USER").then(String::new),
                account_of_this_uid
            ),
            Some(uid_account)
        );
    }

    /// And when no source answers, the answer is that no source answered.
    ///
    /// The version before this one returned the literal `"user"` here, so the
    /// app asked the keychain about an account nobody has, was told there is no
    /// such item, and reported that the person has no key while the key sat
    /// right there.
    #[test]
    fn a_machine_that_names_nobody_at_all_says_so_instead_of_inventing_a_name() {
        let answer = keychain_user_from(|_| None, || None);
        assert_ne!(
            answer.as_deref(),
            Some("user"),
            "a made-up account name is back"
        );
        assert_eq!(answer, None, "not knowing has to stay a question");
    }

    /// The word the doors refuse in is about US, and must not be foldable into
    /// a word about the person's keychain.
    ///
    /// `read_keychain_service_if_present` turns the tool's "no such item" into
    /// `Ok(None)`, and every caller above it reads that as "this person has no
    /// key" — `no_key` on the router and Jev roads. An unaddressable keychain
    /// has to come out the other door, or the lie simply moves up a level.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_door_with_no_account_to_name_refuses_as_an_unknown_user_never_as_a_missing_item() {
        let refusal = unknown_user_refusal();
        assert!(
            refusal.contains(UNKNOWN_USER),
            "the refusal does not name its own door: {refusal}"
        );
        assert!(
            !refusal.contains(NO_SUCH_ITEM),
            "an unaddressable keychain would be folded into `no key`: {refusal}"
        );
        assert_eq!(keychain_account(None), Err(unknown_user_refusal()));
        assert_eq!(
            keychain_account(Some("somebody".into())),
            Ok("somebody".to_string())
        );
    }

    /// An account we could not even name is not a keychain missing an item, so
    /// it neither accuses the login nor triggers a write.
    ///
    /// This is the same distinction the rest of this enum exists for: a write
    /// on an answer we did not get is the dialog storm ("키체인이 계속") with
    /// one more cause behind it.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_unknown_account_neither_contradicts_a_login_nor_reseeds() {
        assert!(!KeychainSays::UnknownUser.contradicts_a_login());
        assert!(!KeychainSays::UnknownUser.is_signed_out_document());
        assert_eq!(KeychainSays::UnknownUser.word(), UNKNOWN_USER);
        let store = tempfile::tempdir().expect("temp store");
        let credentials = r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"b"}}"#;
        seed_scoped_keychain_if_missing(store.path(), credentials, &KeychainSays::UnknownUser)
            .expect("an unknown account is not a repair");
        assert!(
            matches!(keychain_says(store.path()), KeychainSays::Missing),
            "a keychain we could not address was written to anyway"
        );
    }
}
