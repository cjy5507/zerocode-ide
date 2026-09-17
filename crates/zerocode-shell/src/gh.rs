//! GitHub, reached the way Orca reaches it: through the `gh` CLI.
//!
//! Orca never holds a token of its own. Every GitHub read in it is
//! `ghExecFileAsync(["api", …])` — the user's own authenticated `gh`, with
//! `--cache 60s` so a panel that repaints does not re-spend the rate limit
//! (`getPRChecksViaRestFallback`, out/main/index.js:59708-59760). That is the
//! right shape to copy and not only the convenient one: the credential stays
//! in the tool that owns it, and a machine with no `gh` gets an honest "not
//! signed in" instead of a login form this window would have to be trusted
//! with.
//!
//! CI and check details normalize the REST responses into the shared checks
//! vocabulary. The background observer adds a bounded GraphQL review query
//! because thread resolution is needed to avoid nudging resolved feedback.
//! Its conditional REST transport lives in `gh_observer.rs` and reuses the
//! same check parsers, while requiring every fact page to be complete.
//!
//! Everything that shapes JSON into our types is a free function over `&str`
//! so it can be tested without a network, a token, or a repository.

#[path = "gh_observer.rs"]
pub(crate) mod observer;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use zerocode_core::checks::{
    CheckAnnotation, CheckConclusion, CheckDetails, CheckJob, CheckRun, CheckStatus, CheckStep,
    actions_run_id,
};

use crate::vendor_cli::{
    CliCall, CliError, CliOutput, CliRunner, ProcessRunner, VendorCli,
    json::{array, boolean, number, text},
};

/// How long `gh` may reuse its own answer. Orca's own `--cache 60s`; a checks
/// panel refreshes on a timer and on focus, and without this each of those is
/// three requests against a 5,000/hour budget shared with everything else the
/// person's `gh` does.
const CACHE_FOR: &str = "60s";
const CACHE_LIFETIME: Duration = Duration::from_secs(60);
const CONNECTION_TEST_BUDGET: Duration = Duration::from_secs(15);

/// The binary name is declared once; the shared runner owns how it is spawned.
const GH: VendorCli = VendorCli::new("gh");

/// GitHub auth mutations are process-global because `gh`'s selected account is
/// process-external, host-global state. Keep the mutation and its canonical
/// re-read together so two windows cannot each receive the other's answer.
static AUTH_LIFECYCLE: RwLock<()> = RwLock::new(());
static AUTH_CACHE_BYPASS_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Run one GitHub call through the shared process boundary.
fn call_gh(
    runner: &(impl CliRunner + ?Sized),
    cwd: Option<&Path>,
    args: &[String],
    stdin: Option<&[u8]>,
    budget: Option<Duration>,
) -> Result<CliOutput, GhError> {
    runner
        .run(
            GH,
            &CliCall {
                cwd,
                args,
                stdin,
                budget,
            },
        )
        .map_err(Into::into)
}

fn run_gh(cwd: Option<&Path>, args: &[String], stdin: Option<&[u8]>) -> Result<CliOutput, GhError> {
    let _auth = AUTH_LIFECYCLE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    call_gh(&ProcessRunner, cwd, args, stdin, None)
}

fn checked(output: CliOutput) -> Result<String, GhError> {
    if output.success {
        return Ok(output.stdout);
    }
    Err(GhError::Refused(output.stderr.trim().to_string()))
}

fn bypass_old_account_cache() {
    *AUTH_CACHE_BYPASS_UNTIL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now() + CACHE_LIFETIME);
}

fn may_use_auth_cache() -> bool {
    AUTH_CACHE_BYPASS_UNTIL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_none_or(|until| Instant::now() >= until)
}

fn api_args(path: &str, use_cache: bool) -> Vec<String> {
    let mut args = vec!["api".into()];
    if use_cache {
        args.extend(["--cache".into(), CACHE_FOR.into()]);
    }
    args.push(path.into());
    args
}

/// Ceilings the wire carries, measured: 100 checks, 20 annotations, 100 jobs
/// (the `per_page` values in the same three calls). The window says so when a
/// list is at its ceiling rather than quietly showing a prefix.
pub const CHECKS_PER_PAGE: usize = 100;
pub const ANNOTATIONS_PER_PAGE: usize = 20;
pub const JOBS_PER_PAGE: usize = 100;

/// Why a GitHub read could not be answered. The window shows these; they are
/// not log lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhError {
    /// `gh` is not on PATH.
    Missing,
    /// `gh` ran and refused — not signed in, no access, no such repository.
    Refused(String),
    /// `gh` answered something that is not the JSON its API returns.
    Unreadable(String),
}

impl From<CliError> for GhError {
    fn from(error: CliError) -> Self {
        match error {
            CliError::Missing => Self::Missing,
            CliError::Refused(message) => Self::Refused(message),
        }
    }
}

impl GhError {
    /// The key the window looks up to say this in the reader's language. The
    /// message travels beside it for the cases where `gh` said something
    /// specific worth showing verbatim.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Refused(_) => "refused",
            Self::Unreadable(_) => "unreadable",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Refused(text) | Self::Unreadable(text) => Some(text.as_str()),
        }
    }
}

const ACCOUNT_ID_CHARS: usize = 24;

/// One non-secret account row reported by `gh auth status --json hosts`.
///
/// Raw `tokenSource`, scopes, and tokens are deliberately absent. The renderer
/// only needs identity, health, selection, and whether `gh` can actually
/// mutate this row. Environment-provided tokens are effective credentials but
/// cannot be switched or removed by editing `gh`'s own store — for those, the
/// variable's name (only) crosses as `env_credential`, so the card can say
/// what to unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubAccount {
    pub id: String,
    pub host: String,
    pub login: String,
    pub active: bool,
    pub standing: &'static str,
    pub selectable: bool,
    pub disconnectable: bool,
    pub credential_source: &'static str,
    /// The NAME of the environment variable supplying this credential
    /// (`GH_TOKEN`, `GITHUB_TOKEN`, …) — never its value. The settings card
    /// needs the name to say which variable hides the keyring login, because
    /// `gh auth refresh` cannot modify an env-supplied token.
    pub env_credential: Option<String>,
    /// Required scopes this signed-in account's token is missing — names from
    /// `REQUIRED_SCOPES`, never the token's own scope list. Empty when the
    /// token qualifies, when the account cannot stand anyway, or when `gh`
    /// reported no scopes at all (an absent report is not an empty one).
    pub scope_gaps: Vec<String>,
}

/// Canonical, secret-free GitHub integration state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubStatus {
    /// `available` or `missing`. An unreadable response is an error rather than
    /// a fabricated availability state.
    pub availability: &'static str,
    pub connected: bool,
    /// The application never owns a GitHub token; `gh` is the credential
    /// authority even when it chooses an OS keyring underneath.
    pub credential_protection: &'static str,
    pub repo_host: Option<String>,
    pub repo_standing: &'static str,
    pub active_account_id: Option<String>,
    pub accounts: Vec<GithubAccount>,
}

impl GithubStatus {
    fn missing(repo_host: Option<String>) -> Self {
        Self {
            availability: "missing",
            connected: false,
            credential_protection: "external_cli",
            repo_host,
            repo_standing: "unavailable",
            active_account_id: None,
            accounts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubTestReport {
    pub account_id: String,
    pub standing: &'static str,
}

fn normalized_host(raw: &str) -> Option<String> {
    let host = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && !host.ends_with('-')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    .then_some(host)
}

/// Hostname of an ordinary HTTPS/SSH/scp-style git remote, with credentials
/// and ports discarded before anything can cross IPC.
#[must_use]
pub fn remote_host(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let authority = if let Some((_, rest)) = remote.split_once("://") {
        rest.split(['/', '?', '#']).next().unwrap_or_default()
    } else if let Some((left, _)) = remote.split_once(':') {
        // scp form: `git@host:owner/repo`. A local `../repo` has no colon and
        // a Windows drive has no `@`, so neither can become a host.
        if !left.contains('@') {
            return None;
        }
        left
    } else {
        return None;
    };
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host_port.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        _ => host_port,
    };
    normalized_host(host)
}

fn github_account_id(host: &str, login: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(host.as_bytes());
    digest.update(b"\n");
    digest.update(login.trim().to_ascii_lowercase().as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(digest.finalize())
        .chars()
        .take(ACCOUNT_ID_CHARS)
        .collect()
}

/// Every scope this window's own `gh` reads require. Work-item search and
/// seed reads run against repositories, and nothing here touches Projects —
/// Orca asks for `project` and `read:org` because its Projects board does.
const REQUIRED_SCOPES: &[&str] = &["repo"];

/// The scopes a `gh auth status` row reports, in either shape the CLI has
/// used: a comma-separated string (current) or an array of strings. `None`
/// when the row carries no scope report at all — fine-grained tokens do not
/// list classic scopes, and an absent report must not be read as an empty one.
fn account_scopes(value: &Value) -> Option<Vec<String>> {
    let raw = value.get("scopes")?;
    let held: Vec<String> = if let Some(joined) = raw.as_str() {
        joined
            .split(',')
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    } else if let Some(items) = raw.as_array() {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    } else {
        return None;
    };
    Some(held)
}

fn cli_managed_source(source: Option<&str>) -> bool {
    matches!(
        source
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("keyring" | "oauth_token" | "config" | "config file")
    )
}

/// The token source, when it is the NAME of an environment variable. `gh`
/// reports env-supplied credentials by the variable that carries them
/// (`GH_TOKEN`, `GITHUB_TOKEN`, host-scoped variants); anything shaped
/// otherwise is dropped rather than guessed at — a name may cross IPC,
/// a value never could.
fn env_var_name(source: &str) -> Option<String> {
    let name = source.trim();
    let named = name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    named.then(|| name.to_string())
}

fn read_auth_status(raw: &str, repo_host: Option<String>) -> Result<GithubStatus, GhError> {
    let body = parse_json(raw)?;
    let hosts = body
        .get("hosts")
        .and_then(Value::as_object)
        .ok_or_else(|| GhError::Unreadable("GitHub CLI auth JSON has no hosts object".into()))?;
    let mut accounts = Vec::new();
    for (host, values) in hosts {
        let Some(host) = normalized_host(host) else {
            continue;
        };
        let Some(values) = values.as_array() else {
            continue;
        };
        for value in values {
            let Some(login) = text(value, "login") else {
                continue;
            };
            let standing = if value
                .get("state")
                .and_then(Value::as_str)
                .is_some_and(|state| state.eq_ignore_ascii_case("success"))
            {
                "connected"
            } else {
                "auth_error"
            };
            let source = value.get("tokenSource").and_then(Value::as_str);
            let managed = cli_managed_source(source);
            // Scope gaps are judged here, against this window's own needs —
            // the token's scope list itself never crosses IPC. An account
            // that cannot stand gets no scope verdict: login comes first.
            let scope_gaps = if standing == "connected" {
                account_scopes(value).map_or_else(Vec::new, |held| {
                    REQUIRED_SCOPES
                        .iter()
                        .filter(|need| !held.iter().any(|scope| scope == *need))
                        .map(ToString::to_string)
                        .collect()
                })
            } else {
                Vec::new()
            };
            accounts.push(GithubAccount {
                id: github_account_id(&host, &login),
                host: host.clone(),
                login,
                active: boolean(value, "active").unwrap_or(false),
                standing,
                selectable: managed,
                disconnectable: managed,
                credential_source: if managed { "gh" } else { "environment" },
                env_credential: if managed {
                    None
                } else {
                    source.and_then(env_var_name)
                },
                scope_gaps,
            });
        }
    }
    accounts.sort_by(|left, right| {
        left.host
            .cmp(&right.host)
            .then_with(|| right.active.cmp(&left.active))
            .then_with(|| {
                left.login
                    .to_ascii_lowercase()
                    .cmp(&right.login.to_ascii_lowercase())
            })
    });
    let active_account_id = repo_host.as_deref().and_then(|repo_host| {
        accounts
            .iter()
            .find(|account| account.host == repo_host && account.active)
            .map(|account| account.id.clone())
    });
    let repo_standing = match repo_host.as_deref() {
        None => "not_repository",
        Some(host) => accounts
            .iter()
            .find(|account| account.host == host && account.active)
            .map_or("auth_error", |account| account.standing),
    };
    Ok(GithubStatus {
        availability: "available",
        connected: accounts
            .iter()
            .any(|account| account.standing == "connected"),
        credential_protection: "external_cli",
        repo_host,
        repo_standing,
        active_account_id,
        accounts,
    })
}

fn integration_status_with(
    runner: &impl CliRunner,
    cwd: &Path,
    remote: Option<&str>,
    probe_repository: bool,
) -> Result<GithubStatus, GhError> {
    let repo_host = remote.and_then(remote_host);
    let args = [
        "auth".into(),
        "status".into(),
        "--json".into(),
        "hosts".into(),
    ];
    let output = match call_gh(runner, Some(cwd), &args, None, None) {
        Err(GhError::Missing) => return Ok(GithubStatus::missing(repo_host)),
        other => other?,
    };
    // `gh auth status --json` deliberately keeps the useful per-account JSON
    // on stdout when one account is unhealthy, while still returning non-zero.
    // Parse that canonical snapshot first. Only fall back to the process
    // refusal when stdout is absent or not the documented JSON shape.
    let mut status = match read_auth_status(&output.stdout, repo_host.clone()) {
        Ok(status) => status,
        Err(error) if output.success => return Err(error),
        Err(_) => return Err(GhError::Refused(output.stderr.trim().to_string())),
    };
    if probe_repository
        && status.repo_standing == "connected"
        && !call_gh(
            runner,
            Some(cwd),
            &[
                "repo".into(),
                "view".into(),
                "--json".into(),
                "nameWithOwner".into(),
            ],
            None,
            None,
        )?
        .success
    {
        status.repo_standing = "unavailable";
    }
    Ok(status)
}

/// Re-read installed accounts, optionally re-hydrating the login shell PATH
/// first for the explicit "check again" gesture.
pub fn integration_status(
    cwd: &Path,
    remote: Option<&str>,
    force_path_refresh: bool,
) -> Result<GithubStatus, GhError> {
    let _lifecycle = AUTH_LIFECYCLE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if force_path_refresh {
        let _ = crate::shell_path::hydrate(true);
    }
    integration_status_with(&ProcessRunner, cwd, remote, true)
}

/// Can `gh` act on this repository's host right now?
///
/// The narrow question the review ladder asks, and only that: the account
/// listing WITHOUT the repository probe the settings card also spends, since
/// "is this repository reachable" is a different question from "is there a
/// signed-in account for its host" and costs a second process to answer.
///
/// `gh` missing, a host with no active account, and an account whose token
/// has expired all read the same way here, which is the original's own
/// posture — one `auth_required` with one next step (`isGitHubAuthenticated`,
/// hosted-review-creation.ts:66-91).
#[must_use]
pub fn is_authenticated(cwd: &Path, remote: Option<&str>) -> bool {
    let _lifecycle = AUTH_LIFECYCLE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    integration_status_with(&ProcessRunner, cwd, remote, false)
        .is_ok_and(|status| status.repo_standing == "connected")
}

fn account_mutation_with(
    runner: &impl CliRunner,
    cwd: &Path,
    remote: Option<&str>,
    account_id: &str,
    verb: &str,
    committed: impl FnOnce(),
) -> Result<GithubStatus, GhError> {
    let before = integration_status_with(runner, cwd, remote, false)?;
    let account = before
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| GhError::Refused("GitHub account is no longer available".into()))?;
    let allowed = match verb {
        "switch" => account.selectable,
        "logout" => account.disconnectable,
        _ => false,
    };
    if !allowed {
        return Err(GhError::Refused(
            "An environment credential cannot be changed by this app".into(),
        ));
    }
    let args = [
        "auth".into(),
        verb.into(),
        "--hostname".into(),
        account.host.clone(),
        "--user".into(),
        account.login.clone(),
    ];
    checked(call_gh(runner, Some(cwd), &args, None, None)?)?;
    committed();
    integration_status_with(runner, cwd, remote, true)
}

/// Select one account for its host and return the state `gh` reports after the
/// mutation. The opaque id is resolved against a fresh status, never trusted
/// as a hostname or login supplied by the renderer.
pub fn select_account(
    cwd: &Path,
    remote: Option<&str>,
    account_id: &str,
) -> Result<GithubStatus, GhError> {
    let _lifecycle = AUTH_LIFECYCLE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    account_mutation_with(
        &ProcessRunner,
        cwd,
        remote,
        account_id,
        "switch",
        bypass_old_account_cache,
    )
}

/// Remove one account from `gh` on this device and return its canonical state.
/// `gh auth logout` is local removal; it does not claim to revoke the token at
/// GitHub.
pub fn disconnect_account(
    cwd: &Path,
    remote: Option<&str>,
    account_id: &str,
) -> Result<GithubStatus, GhError> {
    let _lifecycle = AUTH_LIFECYCLE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    account_mutation_with(
        &ProcessRunner,
        cwd,
        remote,
        account_id,
        "logout",
        bypass_old_account_cache,
    )
}

pub fn test_connection(
    cwd: &Path,
    remote: Option<&str>,
    account_id: &str,
) -> Result<GithubTestReport, GhError> {
    let _lifecycle = AUTH_LIFECYCLE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    test_connection_with(&ProcessRunner, cwd, remote, account_id)
}

fn test_connection_with(
    runner: &impl CliRunner,
    cwd: &Path,
    remote: Option<&str>,
    account_id: &str,
) -> Result<GithubTestReport, GhError> {
    let status = integration_status_with(runner, cwd, remote, false)?;
    let account = status
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| GhError::Refused("GitHub account is no longer available".into()))?;
    if !account.active {
        return Err(GhError::Refused(
            "Select this GitHub account before testing it".into(),
        ));
    }
    let args = [
        "api".into(),
        "--hostname".into(),
        account.host.clone(),
        "user".into(),
    ];
    let output = call_gh(runner, Some(cwd), &args, None, Some(CONNECTION_TEST_BUDGET))?;
    if !output.success {
        // stderr may contain credential diagnostics owned by `gh`; lifecycle
        // IPC exposes a fixed message from `main`, never this process output.
        return Err(GhError::Refused(
            "GitHub rejected the connection test".into(),
        ));
    }
    let user = parse_json(&output.stdout)?;
    let login = user
        .get("login")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|login| !login.is_empty())
        .ok_or_else(|| GhError::Unreadable("GitHub user response has no login".into()))?;
    if !login.eq_ignore_ascii_case(&account.login) {
        return Err(GhError::Refused(
            "GitHub CLI answered for a different active account".into(),
        ));
    }
    Ok(GithubTestReport {
        account_id: account.id.clone(),
        standing: "connected",
    })
}

/// A safe, non-secret shell command for the explicit login button. The host is
/// derived from the active checkout rather than accepted from the renderer.
#[must_use]
pub fn login_command(remote: Option<&str>) -> String {
    match remote.and_then(remote_host) {
        Some(host) => format!("gh auth login --hostname {host} --web"),
        None => "gh auth login --web".to_string(),
    }
}

/// One `gh api` call, run in a repository. Returns stdout on success.
///
/// The `cwd` matters: `gh` resolves which repository it is talking about from
/// the checkout it runs in, which is also how it picks the right host for an
/// enterprise remote. Passing the path is what lets a window with four
/// worktrees open ask about the right one.
fn gh_api(cwd: &Path, path: &str) -> Result<String, GhError> {
    checked(run_gh(
        Some(cwd),
        &api_args(path, may_use_auth_cache()),
        None,
    )?)
}

fn parse_json(raw: &str) -> Result<Value, GhError> {
    serde_json::from_str(raw).map_err(|error| GhError::Unreadable(error.to_string()))
}

/// The check runs GitHub lists for a commit (`mapRestCheckRun`, :59639).
///
/// A run's page is `details_url` when it has one — that is the CI provider's
/// own page — and `html_url` otherwise, which is GitHub's. Orca prefers the
/// former because a person clicking a failed check wants the log, not the
/// summary that links to it.
pub fn read_check_runs(raw: &str) -> Result<Vec<CheckRun>, GhError> {
    let body = parse_json(raw)?;
    Ok(array(&body, "check_runs")
        .iter()
        .filter_map(|run| {
            let name = text(run, "name")?;
            let status = run.get("status").and_then(Value::as_str).unwrap_or("");
            let url = text(run, "details_url").or_else(|| text(run, "html_url"));
            Some(CheckRun {
                name,
                status: CheckStatus::from_check_run(status),
                conclusion: CheckConclusion::from_check_run(
                    status,
                    run.get("conclusion").and_then(Value::as_str),
                ),
                workflow_run_id: url.as_deref().and_then(actions_run_id).or_else(|| {
                    // The nested form the details call also reads
                    // (`getWorkflowRunIdFromCheckRun`, :59967) — present on the
                    // list response too, and it survives a provider whose
                    // details URL is not an Actions URL.
                    number(run.get("check_suite")?.get("workflow_run")?, "id")
                }),
                check_run_id: number(run, "id"),
                url,
            })
        })
        .collect())
}

/// The legacy commit statuses, minus any whose name a check run already used
/// (`mapRestCommitStatus` + the `checkRunNames` filter, :59649/:59735).
///
/// Both exist on the same commit for the same CI in the middle of a
/// migration, and showing both is showing one check twice.
pub fn read_commit_statuses(raw: &str, taken: &[CheckRun]) -> Result<Vec<CheckRun>, GhError> {
    let body = parse_json(raw)?;
    Ok(array(&body, "statuses")
        .iter()
        .filter_map(|status| {
            let name = text(status, "context")?;
            if taken.iter().any(|check| check.name == name) {
                return None;
            }
            let state = status.get("state").and_then(Value::as_str).unwrap_or("");
            let url = text(status, "target_url");
            Some(CheckRun {
                name,
                status: CheckStatus::from_commit_status(state),
                conclusion: CheckConclusion::from_commit_status(state),
                workflow_run_id: url.as_deref().and_then(actions_run_id),
                check_run_id: None,
                url,
            })
        })
        .collect())
}

/// Suites that are blocked on a person and have no run to say so.
///
/// A first-time contributor's workflow sits at `action_required` on the SUITE,
/// with no check run underneath — so without this the panel shows "no checks"
/// on a PR that cannot merge. Orca fetches the suites for exactly this and
/// keeps only the ones whose runs it has not already listed
/// (:59741-59757).
pub fn read_blocked_suites(
    raw: &str,
    head_sha: &str,
    owner_repo: &str,
    taken: &[CheckRun],
) -> Result<Vec<CheckRun>, GhError> {
    let body = parse_json(raw)?;
    let claimed: Vec<u64> = taken
        .iter()
        .filter_map(|check| check.workflow_run_id)
        .collect();
    Ok(array(&body, "check_suites")
        .iter()
        .filter(|suite| {
            suite
                .get("conclusion")
                .and_then(Value::as_str)
                .map(|word| word.eq_ignore_ascii_case("action_required"))
                .unwrap_or(false)
        })
        .filter(|suite| match number(suite, "id") {
            Some(id) => !claimed.contains(&id),
            None => true,
        })
        .enumerate()
        .map(|(index, suite)| {
            let app = suite
                .get("app")
                .and_then(|app| text(app, "name"))
                .unwrap_or_else(|| format!("Check suite {}", index + 1));
            CheckRun {
                name: app,
                status: CheckStatus::Completed,
                conclusion: Some(CheckConclusion::ActionRequired),
                url: number(suite, "id").map(|id| {
                    format!("https://github.com/{owner_repo}/commit/{head_sha}/checks?check_suite_id={id}")
                }),
                check_run_id: None,
                workflow_run_id: None,
            }
        })
        .collect())
}

/// One check run's own page (`getPRCheckDetails`, :59979).
pub fn read_check_details(raw: &str, fallback_name: &str) -> Result<CheckDetails, GhError> {
    let body = parse_json(raw)?;
    let output = body.get("output");
    Ok(CheckDetails {
        name: text(&body, "name").unwrap_or_else(|| fallback_name.to_string()),
        status: text(&body, "status"),
        conclusion: text(&body, "conclusion"),
        url: text(&body, "html_url").or_else(|| text(&body, "details_url")),
        started_at: text(&body, "started_at"),
        completed_at: text(&body, "completed_at"),
        title: output.and_then(|out| text(out, "title")),
        summary: output.and_then(|out| text(out, "summary")),
        text: output.and_then(|out| text(out, "text")),
        annotations: Vec::new(),
        jobs: Vec::new(),
    })
}

/// Annotations — a path, a line, and what is wrong there
/// (`mapCheckAnnotations`, :59892). The response is a bare array, not an
/// object with a key.
pub fn read_annotations(raw: &str) -> Result<Vec<CheckAnnotation>, GhError> {
    let body = parse_json(raw)?;
    let Some(list) = body.as_array() else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .map(|annotation| CheckAnnotation {
            path: text(annotation, "path"),
            start_line: number(annotation, "start_line").map(|line| line as u32),
            end_line: number(annotation, "end_line").map(|line| line as u32),
            annotation_level: text(annotation, "annotation_level"),
            title: text(annotation, "title"),
            // The one field that is not optional: an annotation with no
            // message is an empty row, and Orca defaults it to "" for the
            // same reason.
            message: text(annotation, "message").unwrap_or_default(),
            raw_details: text(annotation, "raw_details"),
        })
        .collect())
}

/// The jobs of a workflow run (`mapWorkflowJobs`, :59906).
///
/// The name filter at the end is the subtle part: a workflow run holds every
/// job of every check in that workflow, so a details pane for "test (macos)"
/// would otherwise list the twelve jobs of the whole matrix. When a job's name
/// matches the check exactly, those are the only jobs shown; when none does —
/// the check is the workflow itself — all of them are.
pub fn read_jobs(raw: &str, check_name: Option<&str>) -> Result<Vec<CheckJob>, GhError> {
    let body = parse_json(raw)?;
    let jobs: Vec<CheckJob> = array(&body, "jobs")
        .iter()
        .map(|job| CheckJob {
            id: number(job, "id"),
            name: text(job, "name").unwrap_or_else(|| "Unnamed job".to_string()),
            status: text(job, "status"),
            conclusion: text(job, "conclusion"),
            url: text(job, "html_url"),
            log_tail: None,
            steps: array(job, "steps")
                .iter()
                .map(|step| CheckStep {
                    name: text(step, "name").unwrap_or_else(|| "Unnamed step".to_string()),
                    status: text(step, "status"),
                    conclusion: text(step, "conclusion"),
                })
                .collect(),
        })
        .collect();
    let Some(wanted) = check_name else {
        return Ok(jobs);
    };
    let exact: Vec<CheckJob> = jobs
        .iter()
        .filter(|job| job.name == wanted)
        .cloned()
        .collect();
    Ok(if exact.is_empty() { jobs } else { exact })
}

/// Everything the checks list needs for one commit, in Orca's own order:
/// check runs, then statuses it has no run for, then suites blocked on a
/// person.
///
/// All three endpoints must be fetched successfully. A missing status page
/// must not turn a failed check green or resolve an observer notification.
pub fn fetch_checks(
    cwd: &Path,
    owner_repo: &str,
    head_sha: &str,
) -> Result<Vec<CheckRun>, GhError> {
    let mut client = observer::Client::default();
    let mut budget = observer::Budget::new(&zerocode_core::checks::Limits::default());
    client
        .checks_for(cwd, owner_repo, head_sha, false, &mut budget)
        .map(|ci| ci.checks)
}

/// One row's details: the run, its annotations, and the jobs of its workflow.
///
/// Same tolerance as above — the run is the answer, and annotations or jobs
/// that cannot be read leave their sections empty rather than failing the
/// pane. Log tails are NOT fetched here: they are megabytes of text per job,
/// and the panel only ever says whether one exists.
pub fn fetch_check_details(
    cwd: &Path,
    owner_repo: &str,
    name: &str,
    check_run_id: Option<u64>,
    workflow_run_id: Option<u64>,
) -> Result<CheckDetails, GhError> {
    let mut details = match check_run_id {
        Some(id) => read_check_details(
            &gh_api(cwd, &format!("repos/{owner_repo}/check-runs/{id}"))?,
            name,
        )?,
        // A legacy status has no check run to open. The row still gets a pane,
        // carrying what the list already knew.
        None => CheckDetails {
            name: name.to_string(),
            ..CheckDetails::default()
        },
    };
    if let Some(id) = check_run_id
        && let Ok(raw) = gh_api(
            cwd,
            &format!(
                "repos/{owner_repo}/check-runs/{id}/annotations?per_page={ANNOTATIONS_PER_PAGE}"
            ),
        )
    {
        details.annotations = read_annotations(&raw).unwrap_or_default();
    }
    if let Some(run) = workflow_run_id
        && let Ok(raw) = gh_api(
            cwd,
            &format!("repos/{owner_repo}/actions/runs/{run}/jobs?per_page={JOBS_PER_PAGE}"),
        )
    {
        details.jobs = read_jobs(&raw, Some(name)).unwrap_or_default();
    }
    Ok(details)
}

/// The hosted review this checkout is on, as `gh` reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostedReview {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `OPEN`, `MERGED`, `CLOSED` — GitHub's own words, lowercased.
    pub state: String,
    pub draft: bool,
    pub head_sha: String,
    /// `owner/repo`, needed by every call above.
    pub owner_repo: String,
    /// GitHub's own `mergeable` word, lowercased: `mergeable`, `conflicting`
    /// or `unknown`. `None` when the field was absent — an older `gh`, or a
    /// host still calculating. Orca's ChecksPanel keys its conflict strip on
    /// exactly this value being `CONFLICTING`.
    pub mergeable: Option<String>,
    /// When the review last moved, as the ISO stamp `gh` hands over. Worn by
    /// the panel's metadata line (`ChecksPanelUpdatedAtMetadata`), never
    /// parsed here — the window's locale decides how a moment reads.
    pub updated_at: Option<String>,
    /// The branch this review merges into (`baseRefName`). The resolve
    /// prompt names it so an agent fetches the right tip.
    pub base_ref: Option<String>,
}

/// The PR URL names the base repository, including when the head lives in a fork.
fn pr_repository(value: &str, number: u64) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let parts: Vec<_> = url.path_segments()?.collect();
    (parts.len() == 4 && parts[2] == "pull" && parts[3].parse::<u64>().ok() == Some(number))
        .then(|| format!("{}/{}", parts[0], parts[1]))
}

pub fn read_pull_request(raw: &str) -> Result<Option<HostedReview>, GhError> {
    let body = parse_json(raw)?;
    let Some(number) = number(&body, "number") else {
        return Ok(None);
    };
    let repo = body.get("headRepository");
    let owner = body
        .get("headRepositoryOwner")
        .and_then(|owner| text(owner, "login"));
    let name = repo.and_then(|repo| text(repo, "name"));
    let owner_repo = text(&body, "url")
        .and_then(|url| pr_repository(&url, number))
        .or_else(|| Some(format!("{}/{}", owner?, name?)));
    let Some(owner_repo) = owner_repo else {
        return Ok(None);
    };
    Ok(Some(HostedReview {
        number,
        title: text(&body, "title").unwrap_or_default(),
        url: text(&body, "url").unwrap_or_default(),
        state: text(&body, "state")
            .unwrap_or_else(|| "open".into())
            .to_ascii_lowercase(),
        draft: boolean(&body, "isDraft").unwrap_or(false),
        head_sha: text(&body, "headRefOid").unwrap_or_default(),
        owner_repo,
        mergeable: text(&body, "mergeable").map(|word| word.to_ascii_lowercase()),
        updated_at: text(&body, "updatedAt"),
        base_ref: text(&body, "baseRefName"),
    }))
}

/// Ask `gh` which PR this checkout's branch is on.
///
/// `gh pr view` with no argument resolves the current branch, which is exactly
/// the question a workspace-per-branch window asks. Exit non-zero means "this
/// branch has no PR" as often as it means a real failure, so both answer
/// `None` — the panel says "no pull request" either way, and a checks panel is
/// not the surface to explain a git remote.
pub fn fetch_pull_request(cwd: &Path) -> Result<Option<HostedReview>, GhError> {
    let output = run_gh(
        Some(cwd),
        &[
            "pr".into(),
            "view".into(),
            "--json".into(),
            "number,title,url,state,isDraft,headRefOid,headRepository,headRepositoryOwner,mergeable,updatedAt,baseRefName"
                .into(),
        ],
        None,
    )?;
    if !output.success {
        return Ok(None);
    }
    read_pull_request(&output.stdout)
}

/// Where a pull request's head actually lives, and whether we may write there.
///
/// A port of the original's `getPullRequestPushTarget`
/// (`client/lookup/pull-request-push-target.ts:52-153`). The REST call is the
/// only road to it: `gh pr view --json` names the head repository but never
/// gives its clone or ssh URL, and for a fork those URLs are the whole point —
/// there is no remote configured here that points at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushHead {
    pub owner: String,
    pub repo: String,
    /// The branch name ON THAT REPOSITORY. For a fork it is the contributor's
    /// own spelling, without the `owner:` qualifier the compare page uses.
    pub branch: String,
    pub clone_url: String,
    pub ssh_url: String,
    /// `Some(false)` when the PR has "Allow edits from maintainers" off — the
    /// workspace can still be prepared, and a later push may still be refused
    /// by GitHub (`fork-push-warning.ts:6-7`). `None` when the field was
    /// absent, which is not the same as off.
    pub maintainer_can_modify: Option<bool>,
}

impl PushHead {
    /// `owner/repo`, lowercased — the identity two remotes are compared by.
    ///
    /// The original compares through `githubRepoIdentityKey`, which folds case
    /// for the same reason: GitHub treats `Acme/Tool` and `acme/tool` as one
    /// repository, and a remote spelled either way is the same remote.
    #[must_use]
    pub fn identity(&self) -> String {
        format!("{}/{}", self.owner, self.repo).to_ascii_lowercase()
    }
}

pub fn read_push_head(raw: &str) -> Result<Option<PushHead>, GhError> {
    let body = parse_json(raw)?;
    let head = body.get("head");
    let branch = head.and_then(|head| text(head, "ref"));
    let repo = head.and_then(|head| head.get("repo"));
    // A head repository that is `null` is a fork that has since been deleted.
    // GitHub still serves the pull request, and `refs/pull/<n>/head` still
    // resolves — but there is nowhere to push, which is the question here.
    let owner = repo.and_then(|repo| repo.get("owner").and_then(|it| text(it, "login")));
    let name = repo.and_then(|repo| {
        text(repo, "name").or_else(|| {
            text(repo, "full_name")
                .as_deref()
                .and_then(|full| full.split_once('/'))
                .map(|(_, name)| name.trim().to_string())
                .filter(|name| !name.is_empty())
        })
    });
    let clone_url = repo.and_then(|repo| text(repo, "clone_url"));
    let ssh_url = repo.and_then(|repo| text(repo, "ssh_url"));
    let (Some(owner), Some(repo), Some(branch), Some(clone_url), Some(ssh_url)) =
        (owner, name, branch, clone_url, ssh_url)
    else {
        return Ok(None);
    };
    Ok(Some(PushHead {
        owner,
        repo,
        branch,
        clone_url,
        ssh_url,
        maintainer_can_modify: boolean(&body, "maintainer_can_modify"),
    }))
}

/// Ask GitHub where one pull request's head lives.
///
/// `owner_repo` is the repository the pull request was OPENED ON, resolved
/// from this checkout's own remote rather than left to `gh`'s `{owner}/{repo}`
/// expansion — `gh` picks its own preferred remote, and a window that fetched
/// from one remote while asking about another would name a fork that has
/// nothing to do with the commit it is about to check out.
pub fn fetch_push_head(
    cwd: &Path,
    owner_repo: &str,
    number: u64,
) -> Result<Option<PushHead>, GhError> {
    read_push_head(&gh_api(cwd, &format!("repos/{owner_repo}/pulls/{number}"))?)
}

/// The name a fork's remote gets when this window has to add one.
///
/// The original's `sanitizeRemoteName` (`pull-request-push-target.ts:33-40`).
/// The `pr-` prefix is what makes these remotes recognisable later as ones a
/// tool added rather than ones a person did.
#[must_use]
pub fn sanitize_remote_name(owner: &str, repo: &str) -> String {
    let lowered = format!("{owner}-{repo}").to_ascii_lowercase();
    let mut slug = String::with_capacity(lowered.len());
    for one in lowered.chars() {
        // Runs of anything a ref component may not hold collapse to one dash,
        // and so do runs of dashes — `a//b` and `a--b` are one name.
        if one.is_ascii_alphanumeric() || one == '.' || one == '_' || one == '-' {
            if one == '-' && slug.ends_with('-') {
                continue;
            }
            slug.push(one);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let trimmed = slug.trim_matches(['.', '-']);
    if trimmed.is_empty() {
        "pr-head".to_string()
    } else {
        format!("pr-{trimmed}")
    }
}

/// Which of the fork's two URLs to add the remote with.
///
/// The original's `pickPushRemoteUrl` (`pull-request-push-target.ts:20-31`),
/// and its rule is "whatever the person already authenticates with here":
/// adding an https remote to a checkout that clones over ssh asks for a
/// password on the first push. GitHub Enterprise reaches ssh through
/// `ssh.<host>` on port 443, which is why the host is inspected and not just
/// the scheme.
#[must_use]
pub fn pick_push_url(origin_url: Option<&str>, clone_url: &str, ssh_url: &str) -> String {
    if origin_url.is_some_and(origin_speaks_ssh) {
        ssh_url.to_string()
    } else {
        clone_url.to_string()
    }
}

fn origin_speaks_ssh(origin: &str) -> bool {
    if origin.starts_with("git@") || origin.starts_with("ssh:") {
        return true;
    }
    let Some((_, rest)) = origin.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .split_once('@')
        .map_or(authority, |(_, host)| host);
    authority.starts_with("ssh.") || host.starts_with("ssh.")
}

/// A checkout's review, reduced to what a board card wears: a number and one
/// word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewMark {
    pub number: u64,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: &'static str,
}

/// Which of the four words a review wears.
///
/// **Draft is not a fourth state on the wire.** GitHub answers `OPEN` with a
/// separate `isDraft`, so a draft is an open pull request that says so — and
/// the two states that end a review are read first, because a merged review is
/// never a draft and reading the flag first would say it was.
///
/// Case-insensitive because GitHub speaks uppercase (`OPEN`, `MERGED`,
/// `CLOSED`) while `gh`'s own `--json` output has spelled it both ways.
/// A word this table has not met is an open review: something is there to link
/// to, and inventing a fifth tone for it would say more than is known.
pub fn review_word(state: &str, draft: bool) -> &'static str {
    match state.trim().to_ascii_lowercase().as_str() {
        "merged" => "merged",
        "closed" => "closed",
        _ if draft => "draft",
        _ => "open",
    }
}

/// What `gh pr view --json number,state,isDraft` says, as a card wears it.
///
/// A body with no number is a checkout with no review — `gh` answers `{}` for
/// a branch nobody has opened a pull request for.
pub fn read_review_state(raw: &str) -> Result<Option<ReviewMark>, GhError> {
    let body = parse_json(raw)?;
    let Some(number) = number(&body, "number") else {
        return Ok(None);
    };
    Ok(Some(ReviewMark {
        number,
        state: review_word(
            &text(&body, "state").unwrap_or_default(),
            boolean(&body, "isDraft").unwrap_or(false),
        ),
    }))
}

/// The review this checkout's own branch is on, for the board.
///
/// Three fields rather than [`fetch_pull_request`]'s eight: a card wears a
/// number and a word, and asking for the head repository as well would drop a
/// review whose fork has since been deleted — which is a checks-panel concern,
/// not a card's.
///
/// **A refusal is not an error here.** Exit non-zero means "no pull request"
/// far more often than it means anything else, and `gh` missing means a
/// machine that does not use GitHub. Both answer `None`: a board card simply
/// wears no review, and a kanban column is not the surface to explain a git
/// remote.
pub fn fetch_review_state(cwd: &Path) -> Option<ReviewMark> {
    let output = run_gh(
        Some(cwd),
        &[
            "pr".into(),
            "view".into(),
            "--json".into(),
            "number,state,isDraft".into(),
        ],
        None,
    )
    .ok()?;
    review_state_of(output)
}

/// What a finished `pr view` call means for a card — the exit standing is the
/// verdict, read BEFORE the body. `gh` prints error bodies on stdout at times,
/// and reading one as a review would dress a card in a state nobody is in.
/// A function of its own so that sentence is testable without a process.
fn review_state_of(output: CliOutput) -> Option<ReviewMark> {
    if !output.success {
        return None;
    }
    read_review_state(&output.stdout).ok().flatten()
}

/// Open a pull request, and answer with where it landed.
///
/// The body goes over STDIN (`--body-file -`), never argv: a generated
/// description is a page of Markdown and a repository's PR template can be
/// several, which is past what a command line will carry on any platform.
///
/// # Errors
///
/// [`GhError::Missing`] when `gh` is not on the machine; [`GhError::Refused`]
/// with what `gh` said otherwise — a PR that already exists, a branch with no
/// upstream, and a login that expired all arrive here, and each one is
/// something the person has to read rather than something to reduce to a word.
pub fn create_pull_request(
    cwd: &Path,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<String, GhError> {
    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--base".to_string(),
        base.to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body-file".to_string(),
        "-".to_string(),
    ];
    if draft {
        args.push("--draft".to_string());
    }
    let output = run_gh(Some(cwd), &args, Some(body.as_bytes()))?;
    if !output.success {
        let said = output.stderr.trim();
        return Err(GhError::Refused(if said.is_empty() {
            "gh pr create failed".to_string()
        } else {
            said.lines().last().unwrap_or(said).to_string()
        }));
    }
    // `gh` prints the URL of what it made. The last non-empty line, because it
    // also prints progress above it on a branch it had to push.
    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string())
}

/* ---- what the create dialog's GitHub tab lists ------------------------------
 *
 * The same road as everything above, and that is the point: Orca's work-item
 * list is `gh pr list --json …` and `gh api search/issues` through the very
 * same `ghExecFileAsync` the checks panel uses (`buildWorkItemListRequest`,
 * out/main/index.js:2407507; `listRecentWorkItems` :2409989;
 * `listQueriedWorkItems` :2412132). There is no second client and no token of
 * ours to keep.
 *
 * Two deliberate differences from the measured calls, both because this window
 * asks about ONE repository — the project the composer is standing in:
 *
 *   - No `--repo owner/repo`. `gh` reads the repository out of the directory it
 *     runs in, which is the same fact [`gh_api`] relies on, and asking for the
 *     slug first would be an extra round trip to tell `gh` what it already
 *     knows.
 *   - Issues go through `gh issue list --search` rather than
 *     `gh api search/issues?q=repo:…`. It is the same search API on the far
 *     side; the difference is that the qualifier naming the repository is the
 *     working directory instead of a string we would have to build.
 */

/// How many rows the list shows at once (`RESULT_LIMIT`,
/// App-BaqTRjaA.js@1319185).
pub const WORK_ITEM_LIMIT: usize = 12;

/// Past this a query is not a query, it is a paste
/// (`WORK_ITEM_LINK_QUERY_MAX_BYTES = 2 * 1024`,
/// work-item-link-query-bounds-CKph40_I.js). Orca answers an empty list rather
/// than sending it, and so does this.
pub const QUERY_MAX_BYTES: usize = 2 * 1024;

/// The order the list is asked for (`WORK_ITEM_NUMBER_SORT_QUALIFIER`,
/// :2394404).
const SORT_QUALIFIER: &str = "sort:created-desc";

/// What a pull request row needs. Orca asks for more
/// (`WORK_ITEM_PR_LIST_JSON_FIELDS`, :2394463 — `labels`, `reviewRequests`)
/// that no row here draws; a field we do not paint is bandwidth spent on a
/// guess about a screen that does not exist yet. `assignees` joined when the
/// table gained its 담당자 column (1-g77b).
const PR_FIELDS: &str = "number,title,state,url,updatedAt,author,assignees,isDraft,headRefName,baseRefName,headRepositoryOwner";

/// The issue half (`mapIssueWorkItem`, :2394943).
const ISSUE_FIELDS: &str = "number,title,state,url,updatedAt,author,assignees";

/// Which half of GitHub a search asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Pr,
    Issue,
}

impl ItemKind {
    /// The word the window sends, or nothing. An unknown word is REFUSED
    /// rather than guessed at — the rule `GlabKind::from_word` keeps, and for
    /// the same reason: what this selects is a `gh` noun and a half of the
    /// search grammar, so a guess is a call this file never agreed to make.
    #[must_use]
    pub fn from_word(said: &str) -> Option<Self> {
        Some(match said {
            "issue" => Self::Issue,
            "pr" => Self::Pr,
            _ => return None,
        })
    }

    /// The word the window switches on, and the `gh` noun. One function so the
    /// two cannot drift apart.
    const fn word(self) -> &'static str {
        match self {
            Self::Pr => "pr",
            Self::Issue => "issue",
        }
    }
}

/// What a search turned into: which lists to ask, and what to ask them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub asks: Vec<ItemKind>,
    pub query: String,
    /// The query was over [`QUERY_MAX_BYTES`]. Nothing is asked, and the tab
    /// says so rather than sending two kilobytes of pasted diff to GitHub.
    pub too_large: bool,
}

/// Which slice of a kind's work one preset chip asks for.
///
/// A chip is one word from the window and nothing more; an unknown one is
/// REFUSED, the same rule [`ItemKind::from_word`] keeps. Which words belong to
/// which kind is decided by [`preset_query`] alone — an issue has no reviewer
/// and no author chip, and a pair nobody offers is a question nobody can read
/// the empty answer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhPreset {
    /// Everything still open — the first chip of both kinds.
    Open,
    /// Issues assigned to the person (issues only).
    AssignedToMe,
    /// Pull requests the person opened (pull requests only).
    Mine,
    /// Pull requests waiting on the person's review (pull requests only).
    ReviewNeeded,
}

impl GhPreset {
    /// The word the window sends, or nothing.
    #[must_use]
    pub fn from_word(said: &str) -> Option<Self> {
        Some(match said {
            "open" => Self::Open,
            "assigned" => Self::AssignedToMe,
            "mine" => Self::Mine,
            "review" => Self::ReviewNeeded,
            _ => return None,
        })
    }
}

/// What one preset chip asks — and the only copy of that grammar.
///
/// Measured on the newer Orca (recon 2026-08-16, which outranks the 1.4.180
/// bundle): a chip click WRITES this string into the search box and runs it.
/// So the order is the order a person reads on screen — the kind, then the
/// state, then whose work — and the window can show the sentence without ever
/// spelling it (`github_preset_query` hands it over as data).
///
/// [`None`] for a pair that does not exist. That is a refusal rather than an
/// empty list: `is:issue … review-requested:@me` answers nothing, and nothing
/// reads as "GitHub has none of these" instead of "issues have no reviewers".
#[must_use]
pub fn preset_query(kind: ItemKind, preset: GhPreset) -> Option<&'static str> {
    Some(match (kind, preset) {
        (ItemKind::Issue, GhPreset::Open) => "is:issue is:open",
        (ItemKind::Issue, GhPreset::AssignedToMe) => "is:issue is:open assignee:@me",
        (ItemKind::Pr, GhPreset::Open) => "is:pr is:open",
        (ItemKind::Pr, GhPreset::Mine) => "is:pr is:open author:@me",
        (ItemKind::Pr, GhPreset::ReviewNeeded) => "is:pr is:open review-requested:@me",
        _ => return None,
    })
}

/// The same five questions under the composer's own names
/// (`getTaskPresetQuery`, native-chat-session-option-cache-Bqn9it4C.js@23579).
///
/// The composer picks a preset from a list of single words, while the tasks
/// page sends the kind and the chip (1-g77a). Two vocabularies, one table:
/// a second spelling of the qualifiers is how the picker and the chips start
/// answering different questions under the same label.
///
/// [`None`] for anything else — including the absence of a preset, which is
/// the composer's own first state rather than a sixth preset. Orca's task PAGE
/// defaults to `is:issue is:open`; its composer passes no preset at all and the
/// main process answers with both lists (`listRecentWorkItems` asks
/// `parseTaskQuery("is:open")` of issues and pull requests alike).
#[must_use]
pub fn composer_preset_query(preset: &str) -> Option<&'static str> {
    let (kind, chip) = match preset {
        "all" | "issues" => (ItemKind::Issue, GhPreset::Open),
        "my-issues" => (ItemKind::Issue, GhPreset::AssignedToMe),
        "prs" => (ItemKind::Pr, GhPreset::Open),
        "my-prs" => (ItemKind::Pr, GhPreset::Mine),
        "review" => (ItemKind::Pr, GhPreset::ReviewNeeded),
        _ => return None,
    };
    preset_query(kind, chip)
}

/// What the filter popover picked, as VALUES.
///
/// The window may not spell a qualifier — the search grammar has exactly one
/// copy and it is this file (`every_github_read_goes_through_one_door`). So
/// the popover sends what a person chose and [`WorkItemFilters::qualifiers`]
/// is the only place those choices become words GitHub reads.
///
/// `state` is the three words a work item can be in. Anything else — a stale
/// build's spelling, a hand-written invoke — is not a narrower search but an
/// unanswerable one, so it maps to no state at all rather than riding out to
/// GitHub as junk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WorkItemFilters {
    pub state: Option<String>,
    pub draft: bool,
    /// Who wrote it / who holds it — GitHub logins, picked off the bench the
    /// popover loads. Anything that is not a login (see [`valid_login`])
    /// narrows nothing, the state rule above.
    pub author: Option<String>,
    pub assignee: Option<String>,
}

/// Whether a string is a GitHub login at all: 1–39 characters of ASCII
/// alphanumerics and hyphens, neither end a hyphen. The question rides argv
/// as one `--search` sentence, so a "login" carrying spaces or quotes would
/// be this window smuggling grammar — the one thing it may never do.
fn valid_login(said: &str) -> bool {
    !said.is_empty()
        && said.len() <= 39
        && said
            .chars()
            .all(|one| one.is_ascii_alphanumeric() || one == '-')
        && !said.starts_with('-')
        && !said.ends_with('-')
}

impl WorkItemFilters {
    /// The qualifiers these values mean, in the order they join the question.
    ///
    /// `state:` is GitHub's own search grammar and reads the same on both
    /// halves; `draft:true` is a pull request's word, which is why the scope
    /// rule below has to see it.
    fn qualifiers(&self) -> Vec<String> {
        let mut said: Vec<String> = Vec::new();
        if let Some(state) = match self.state.as_deref() {
            Some("open") => Some("state:open"),
            Some("closed") => Some("state:closed"),
            Some("merged") => Some("state:merged"),
            _ => None,
        } {
            said.push(state.to_string());
        }
        if self.draft {
            said.push("draft:true".to_string());
        }
        for (name, held) in [("author", &self.author), ("assignee", &self.assignee)] {
            if let Some(login) = held.as_deref().filter(|login| valid_login(login)) {
                said.push(format!("{name}:{login}"));
            }
        }
        said
    }
}

/// Qualifiers only a pull request can answer (`hasPrOnlyFilter`, :2412132),
/// plus `is:pr` itself.
///
/// Asking the issue list for `review-requested:@me` is not a narrower search,
/// it is an empty one — and an empty half would look like "GitHub has nothing"
/// rather than "issues cannot have reviewers".
///
/// `state:merged` stands beside `is:merged` for the same reason: the filter
/// popover spells the state one, and an issue is never merged.
fn is_pr_only(query: &str) -> bool {
    [
        "is:pr",
        "is:merged",
        "state:merged",
        "draft:true",
        "review-requested:",
        "reviewed-by:",
    ]
    .iter()
    .any(|mark| query.contains(mark))
}

/// What to ask, from a preset, whatever was typed beside it, and whatever the
/// filter popover picked.
///
/// The scope rule is measured (`listQueriedWorkItems`): `is:issue` means the
/// issue list alone, a pull-request-only qualifier means the pull request list
/// alone, and anything else asks both and merges them — which is what an empty
/// search does, and why opening the tab shows the repository's recent work
/// rather than an empty box.
///
/// The filters join LAST. In Orca the query string is the single truth and the
/// popover edits it in place; here the window keeps the values and this is the
/// one hand that turns them into words — so their place in the sentence is
/// decided once, here, rather than by whichever surface wrote them.
#[must_use]
pub fn search_plan(preset: &str, typed: &str, filters: &WorkItemFilters) -> Plan {
    let typed = typed.trim();
    if typed.len() > QUERY_MAX_BYTES {
        return Plan {
            asks: Vec::new(),
            query: String::new(),
            too_large: true,
        };
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(preset) = composer_preset_query(preset) {
        parts.push(preset.to_string());
    }
    if !typed.is_empty() {
        parts.push(typed.to_string());
    }
    parts.extend(filters.qualifiers());
    // `is:open` when nobody has narrowed anything — Orca's own recent query
    // (`parseTaskQuery("is:open")`, :2410004). A filter IS a narrowing, so a
    // picked state stands alone rather than under a default that contradicts
    // it.
    let query = if parts.is_empty() {
        "is:open".to_string()
    } else {
        parts.join(" ")
    };
    let asks = if query.contains("is:issue") {
        vec![ItemKind::Issue]
    } else if is_pr_only(&query) {
        vec![ItemKind::Pr]
    } else {
        vec![ItemKind::Pr, ItemKind::Issue]
    };
    Plan {
        asks,
        query,
        too_large: false,
    }
}

/// One list request, as argv.
#[must_use]
pub fn work_item_args(kind: ItemKind, query: &str, limit: usize) -> Vec<String> {
    // `--state all` with the state in the SEARCH, which is how Orca spells it:
    // the flag and the qualifier are two filters, and leaving the flag at its
    // default would quietly intersect `is:merged` with "open".
    vec![
        kind.word().to_string(),
        "list".to_string(),
        "--limit".to_string(),
        limit.to_string(),
        "--state".to_string(),
        "all".to_string(),
        "--json".to_string(),
        match kind {
            ItemKind::Pr => PR_FIELDS,
            ItemKind::Issue => ISSUE_FIELDS,
        }
        .to_string(),
        "--search".to_string(),
        format!("{query} {SORT_QUALIFIER}"),
    ]
}

/// One row of the GitHub tab.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkItem {
    /// `pr` or `issue` — the window draws a different mark and a different
    /// create for each.
    pub kind: &'static str,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `open`, `draft`, `merged`, `closed` (`mapPullRequestWorkItem`,
    /// :2399600, in its own precedence).
    pub state: String,
    pub author: Option<String>,
    /// The pull request's head branch — the branch a checkout of it lands on.
    pub branch: Option<String>,
    /// The branch it is opened against, which is what the checkout is compared
    /// with afterwards.
    pub base: Option<String>,
    /// The head lives in somebody else's fork. Measured the way Orca measures
    /// it: the head repository's owner is not this repository's owner.
    pub cross_repo: bool,
    pub updated_at: Option<String>,
    /// Who the item is assigned to — the handles, in the API's order. The
    /// table's 담당자 column; empty is a fact the column prints as a dash,
    /// not an absence to hide.
    pub assignees: Vec<String>,
    /// Which project answered, when the read spanned several — the checkout
    /// path the window asked with, `None` on a single-project read so the
    /// table stays quiet about the only project there is.
    pub project: Option<String>,
}

/// The owner in `https://github.com/owner/repo/pull/7`.
///
/// A pull request's own URL is always the BASE repository's, which is what
/// makes it the cheapest true answer to "whose repository is this": Orca
/// resolves the same fact with a second `gh` call for the remote's slug
/// (`resolvePrWorkItemSource`, :2415000), and one of us is spending a request
/// on something already in hand.
fn owner_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let mut parts = rest.split('/');
    parts.next()?;
    let owner = parts.next()?;
    (!owner.is_empty()).then_some(owner)
}

/// Orca's precedence, and the order matters: a merged pull request that was
/// opened as a draft is merged, and a closed one is closed rather than
/// still-a-draft. One function so the list row and the opened item cannot
/// disagree about what state a thing is in.
fn work_item_state(item: &Value) -> &'static str {
    let said = text(item, "state").unwrap_or_default().to_ascii_lowercase();
    let draft = boolean(item, "isDraft").unwrap_or(false);
    if said == "merged" {
        "merged"
    } else if said == "closed" {
        "closed"
    } else if draft {
        "draft"
    } else {
        "open"
    }
}

fn read_work_item(item: &Value, kind: ItemKind) -> Option<WorkItem> {
    let number = number(item, "number")?;
    let url = text(item, "url").unwrap_or_default();
    let state = work_item_state(item);
    let head_owner = item
        .get("headRepositoryOwner")
        .and_then(|who| text(who, "login"));
    let cross_repo = match (head_owner.as_deref(), owner_of(&url)) {
        (Some(head), Some(base)) => !head.eq_ignore_ascii_case(base),
        // A pull request whose head repository is gone answers nothing here,
        // and calling that "same repository" is the answer that would try to
        // fetch a branch that is not there.
        _ => false,
    };
    Some(WorkItem {
        kind: kind.word(),
        number,
        title: text(item, "title").unwrap_or_default(),
        url,
        state: state.to_string(),
        author: item.get("author").and_then(|who| text(who, "login")),
        branch: text(item, "headRefName"),
        base: text(item, "baseRefName"),
        cross_repo,
        updated_at: text(item, "updatedAt"),
        assignees: array(item, "assignees")
            .iter()
            .filter_map(|who| text(who, "login"))
            .collect(),
        project: None,
    })
}

/// One `gh … list --json` answer, shaped into rows.
pub fn read_work_items(raw: &str, kind: ItemKind) -> Result<Vec<WorkItem>, GhError> {
    let body = parse_json(raw)?;
    let Some(list) = body.as_array() else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter_map(|item| read_work_item(item, kind))
        .collect())
}

/// One `gh` run in a repository, for the subcommands that are not `api`.
fn gh_run(cwd: &Path, args: &[String]) -> Result<String, GhError> {
    checked(run_gh(Some(cwd), args, None)?)
}

/// One voice under a work item — who, when, and their markdown, which the
/// window's renderer treats as text-by-construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkItemComment {
    pub author: Option<String>,
    pub written_at: Option<String>,
    pub body: String,
}

/// A work item opened on its own: the row's facts plus the body and the
/// conversation. Fetched with `gh <kind> view` in the repository, so the host
/// and the repo resolve exactly the way the list did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkItemDetail {
    pub kind: &'static str,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub author: Option<String>,
    pub opened_at: Option<String>,
    pub body: String,
    pub comments: Vec<WorkItemComment>,
    /// GitHub's own merge verdicts, PRs only and spelled exactly as the API
    /// spells them (`MERGEABLE`/`CONFLICTING`, `CLEAN`/`DIRTY`/`BLOCKED`/
    /// `BEHIND`/`UNSTABLE`, `REVIEW_REQUIRED`/`CHANGES_REQUESTED`). The
    /// window's ladder reads them verbatim — translating here would be a
    /// second copy of a table the renderer already owns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_state_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<String>,
    /* ---- appended (t-2733): the depth behind a PR's row. After the fields
     * above, never between them — the source-contract pin on this struct is
     * a contiguous block. Empty on an issue rather than absent, so the window
     * reads one shape. ---- */
    /// The changed files, with their ± and GitHub's own status word.
    pub files: Vec<PrFile>,
    /// The rollup's checks, through the same normaliser the panel uses.
    pub checks: PrChecks,
    /// Who was asked and where each stands.
    pub reviewers: Vec<PrReviewer>,
    /// Review comments, folded into threads by `in_reply_to_id`.
    pub conversation: Vec<ConversationThread>,
}

/// One changed file of a pull request (`GET pulls/{n}/files`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    /// GitHub's own word — `added`, `removed`, `modified`, `renamed`,
    /// `copied`, `changed`, `unchanged` — passed through, not translated.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
}

/// The rollup a PR view carries: one word for the whole and the rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrChecks {
    /// `none` · `pending` · `failing` · `passing`.
    pub state: String,
    pub rows: Vec<CheckRun>,
}

impl Default for PrChecks {
    fn default() -> Self {
        Self {
            state: "none".to_string(),
            rows: Vec::new(),
        }
    }
}

/// One reviewer: the login GitHub knows, and where they stand — `requested`
/// for somebody asked and silent, else their latest review's state
/// lowercased (`approved`, `changes_requested`, `commented`, `dismissed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrReviewer {
    pub login: String,
    pub state: String,
}

/// One voice under a review thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationReply {
    pub id: u64,
    pub author: Option<String>,
    pub body: String,
    pub at: Option<String>,
}

/// A review comment and the replies filed under it. A reply whose root the
/// page did not carry stands as a root of its own — a voice is never dropped
/// because its parent fell off the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationThread {
    pub id: u64,
    pub author: Option<String>,
    pub body: String,
    pub at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub replies: Vec<ConversationReply>,
}

/// One open review as the stack map reads it (`GET pulls?state=open`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackReview {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `open` · `draft` — through [`review_word`], so the map and the card
    /// wear the same word.
    pub state: String,
    pub head: String,
    pub base: String,
}

impl StackReview {
    /// The link the pure ordering reads.
    pub fn link(&self) -> zerocode_core::checks::StackPr {
        zerocode_core::checks::StackPr {
            number: self.number,
            head: self.head.clone(),
            base: self.base.clone(),
        }
    }
}

/// `GET repos/{owner}/{repo}/pulls/{n}/files`, one row per file.
pub fn read_pr_files(raw: &str) -> Result<Vec<PrFile>, GhError> {
    let body = parse_json(raw)?;
    Ok(body
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(PrFile {
                        path: text(row, "filename")?,
                        additions: number(row, "additions").unwrap_or(0),
                        deletions: number(row, "deletions").unwrap_or(0),
                        status: text(row, "status").unwrap_or_default(),
                        previous_path: text(row, "previous_filename"),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The rollup a `gh pr view --json statusCheckRollup` carries, through the
/// same two normalisers the checks panel uses: a `CheckRun` node speaks
/// status/conclusion, a `StatusContext` node speaks `state`. GraphQL spells
/// its words in uppercase; the normalisers lowercase.
fn read_rollup_checks(item: &Value) -> PrChecks {
    let rows: Vec<CheckRun> = array(item, "statusCheckRollup")
        .iter()
        .filter_map(|node| {
            if node.get("__typename").and_then(Value::as_str) == Some("StatusContext") {
                let state = node.get("state").and_then(Value::as_str).unwrap_or("");
                let url = text(node, "targetUrl");
                return Some(CheckRun {
                    name: text(node, "context")?,
                    status: CheckStatus::from_commit_status(state),
                    conclusion: CheckConclusion::from_commit_status(state),
                    workflow_run_id: url.as_deref().and_then(actions_run_id),
                    check_run_id: None,
                    url,
                });
            }
            let status = node.get("status").and_then(Value::as_str).unwrap_or("");
            let url = text(node, "detailsUrl");
            Some(CheckRun {
                name: text(node, "name")?,
                status: CheckStatus::from_check_run(status),
                conclusion: CheckConclusion::from_check_run(
                    status,
                    node.get("conclusion").and_then(Value::as_str),
                ),
                workflow_run_id: url.as_deref().and_then(actions_run_id),
                check_run_id: None,
                url,
            })
        })
        .collect();
    let tally = zerocode_core::checks::CheckTally::of(&rows);
    let state = if rows.is_empty() {
        "none"
    } else if tally.failing > 0 {
        "failing"
    } else if tally.pending > 0 {
        "pending"
    } else {
        "passing"
    };
    PrChecks {
        state: state.to_string(),
        rows,
    }
}

/// Who reviews: the requests still open, then everybody whose latest review
/// stands — GitHub's own two lists, in that order.
fn read_reviewers(item: &Value) -> Vec<PrReviewer> {
    let mut listed: Vec<PrReviewer> = array(item, "reviewRequests")
        .iter()
        .filter_map(|who| {
            Some(PrReviewer {
                login: text(who, "login")?,
                state: "requested".to_string(),
            })
        })
        .collect();
    for review in array(item, "latestReviews") {
        let Some(login) = review.get("author").and_then(|who| text(who, "login")) else {
            continue;
        };
        if listed.iter().any(|one| one.login == login) {
            continue;
        }
        listed.push(PrReviewer {
            login,
            state: text(review, "state")
                .unwrap_or_default()
                .to_ascii_lowercase(),
        });
    }
    listed
}

/// `GET pulls/{n}/comments`, folded into threads: a comment with no
/// `in_reply_to_id` is a root, a reply files under its root, and a reply
/// whose root is not on the page stands as a root of its own.
pub fn read_conversation(raw: &str) -> Result<Vec<ConversationThread>, GhError> {
    let body = parse_json(raw)?;
    let rows = body.as_array().cloned().unwrap_or_default();
    let mut threads: Vec<ConversationThread> = Vec::new();
    let roots: std::collections::HashSet<u64> = rows
        .iter()
        .filter(|row| row.get("in_reply_to_id").and_then(Value::as_u64).is_none())
        .filter_map(|row| number(row, "id"))
        .collect();
    for row in &rows {
        let Some(id) = number(row, "id") else {
            continue;
        };
        let author = row.get("user").and_then(|who| text(who, "login"));
        let said = row
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let at = text(row, "created_at");
        let parent = number(row, "in_reply_to_id").filter(|root| roots.contains(root));
        match parent.and_then(|root| threads.iter_mut().find(|thread| thread.id == root)) {
            Some(thread) => thread.replies.push(ConversationReply {
                id,
                author,
                body: said,
                at,
            }),
            None => threads.push(ConversationThread {
                id,
                author,
                body: said,
                at,
                path: text(row, "path"),
                line: number(row, "line"),
                replies: Vec::new(),
            }),
        }
    }
    Ok(threads)
}

/// The open reviews of a repository, as the stack map reads them.
pub fn read_stack_reviews(raw: &str) -> Result<Vec<StackReview>, GhError> {
    let body = parse_json(raw)?;
    Ok(body
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(StackReview {
                        number: number(row, "number")?,
                        title: text(row, "title").unwrap_or_default(),
                        url: text(row, "html_url").unwrap_or_default(),
                        state: review_word(
                            &text(row, "state").unwrap_or_default(),
                            boolean(row, "draft").unwrap_or(false),
                        )
                        .to_string(),
                        head: row.get("head").and_then(|head| text(head, "ref"))?,
                        base: row.get("base").and_then(|base| text(base, "ref"))?,
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The open reviews, one cached page, for the stack.
pub fn fetch_stack_reviews(
    cwd: &Path,
    owner_repo: &str,
    max: usize,
) -> Result<Vec<StackReview>, GhError> {
    read_stack_reviews(&gh_api(
        cwd,
        &format!("repos/{owner_repo}/pulls?state=open&per_page={max}"),
    )?)
}

/// The whole diff of one pull request, as `gh pr diff` prints it — read
/// once per PR and cut per file by `checks_runtime`.
pub fn fetch_pr_diff(cwd: &Path, item: u64) -> Result<String, GhError> {
    gh_run(
        cwd,
        &["pr".to_string(), "diff".to_string(), item.to_string()],
    )
}

/// Ask GitHub for (or take back) reviewers, the login array exactly as the
/// window sent it — one `reviewers[]` flag per login, in order, untrimmed
/// and uncased: a login is GitHub's identifier, and "helpfully" normalising
/// it is how `Kim-Lee` becomes somebody else. The verb is the only thing
/// this side decides. A refusal is `gh`'s first line, as a sentence.
pub fn set_reviewers(
    cwd: &Path,
    item: u64,
    logins: &[String],
    remove: bool,
) -> Result<(), GhError> {
    let _auth = AUTH_LIFECYCLE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    set_reviewers_with(&ProcessRunner, cwd, item, logins, remove)
}

fn set_reviewers_with(
    runner: &impl CliRunner,
    cwd: &Path,
    item: u64,
    logins: &[String],
    remove: bool,
) -> Result<(), GhError> {
    if logins.is_empty() {
        return Err(GhError::Refused("no reviewer was named".to_string()));
    }
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        if remove { "DELETE" } else { "POST" }.to_string(),
        format!("repos/{{owner}}/{{repo}}/pulls/{item}/requested_reviewers"),
    ];
    for login in logins {
        args.push("-f".to_string());
        args.push(format!("reviewers[]={login}"));
    }
    let output = call_gh(runner, Some(cwd), &args, None, None)?;
    if output.success {
        return Ok(());
    }
    Err(GhError::Refused(
        output
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_string(),
    ))
}

const ISSUE_DETAIL_FIELDS: &str = "number,title,state,author,createdAt,body,url,comments";
const PR_DETAIL_FIELDS: &str = "number,title,state,author,createdAt,body,url,comments,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,reviewRequests,latestReviews";

fn work_item_view_args(kind: ItemKind, item: u64) -> Vec<String> {
    vec![
        kind.word().to_string(),
        "view".to_string(),
        item.to_string(),
        "--json".to_string(),
        match kind {
            ItemKind::Pr => PR_DETAIL_FIELDS,
            ItemKind::Issue => ISSUE_DETAIL_FIELDS,
        }
        .to_string(),
    ]
}

/// One `gh … view --json` answer, shaped into the opened item.
pub fn read_work_item_detail(raw: &str, kind: ItemKind) -> Result<WorkItemDetail, GhError> {
    let item = parse_json(raw)?;
    let number = number(&item, "number")
        .ok_or_else(|| GhError::Unreadable("work item detail has no number".into()))?;
    let comments = array(&item, "comments")
        .iter()
        .map(|one| WorkItemComment {
            author: one.get("author").and_then(|who| text(who, "login")),
            written_at: text(one, "createdAt"),
            // Not `text()`: an emptied comment is still a comment, and trimming
            // markdown would move what the person wrote.
            body: one
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    Ok(WorkItemDetail {
        kind: kind.word(),
        number,
        title: text(&item, "title").unwrap_or_default(),
        url: text(&item, "url").unwrap_or_default(),
        state: work_item_state(&item).to_string(),
        author: item.get("author").and_then(|who| text(who, "login")),
        opened_at: text(&item, "createdAt"),
        body: item
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        comments,
        mergeable: text(&item, "mergeable"),
        merge_state_status: text(&item, "mergeStateStatus"),
        review_decision: text(&item, "reviewDecision"),
        // Files and threads are separate reads (`fetch_work_item_detail`
        // lays them in); the view itself carries the rollup and the reviewers.
        files: Vec::new(),
        checks: match kind {
            ItemKind::Pr => read_rollup_checks(&item),
            ItemKind::Issue => PrChecks::default(),
        },
        reviewers: match kind {
            ItemKind::Pr => read_reviewers(&item),
            ItemKind::Issue => Vec::new(),
        },
        conversation: Vec::new(),
    })
}

/// # Errors
///
/// [`GhError::Missing`] without `gh`, and whatever `gh` refused with —
/// an unknown number is `gh`'s own sentence, not an invented one.
pub fn fetch_work_item_detail(
    cwd: &Path,
    kind: ItemKind,
    item: u64,
    limits: &zerocode_core::checks::Limits,
) -> Result<WorkItemDetail, GhError> {
    let mut detail = gh_run(cwd, &work_item_view_args(kind, item))
        .and_then(|raw| read_work_item_detail(&raw, kind))?;
    if kind != ItemKind::Pr {
        return Ok(detail);
    }
    // The same tolerance `fetch_checks` keeps: the view IS the answer, and a
    // files page or a comments page that could not be read leaves its
    // section empty rather than failing the dialog over a third call.
    if let Ok(raw) = gh_api(
        cwd,
        &format!(
            "repos/{{owner}}/{{repo}}/pulls/{item}/files?per_page={}",
            limits.files_max
        ),
    ) {
        detail.files = read_pr_files(&raw).unwrap_or_default();
    }
    if let Ok(raw) = gh_api(
        cwd,
        &format!(
            "repos/{{owner}}/{{repo}}/pulls/{item}/comments?per_page={}",
            limits.conversation_max
        ),
    ) {
        detail.conversation = read_conversation(&raw).unwrap_or_default();
    }
    Ok(detail)
}

/// The three roads GitHub itself names, in Orca's measured menu order
/// (squash first). The enum is the door: whatever a window sends either IS
/// one of these flags or the merge never starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrMergeMethod {
    Squash,
    Merge,
    Rebase,
}

impl PrMergeMethod {
    /// # Errors
    ///
    /// The word back, when it is not one of the three roads.
    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "squash" => Ok(Self::Squash),
            "merge" => Ok(Self::Merge),
            "rebase" => Ok(Self::Rebase),
            other => Err(format!("unknown merge method: {other}")),
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Self::Squash => "--squash",
            Self::Merge => "--merge",
            Self::Rebase => "--rebase",
        }
    }
}

#[must_use]
pub fn merge_pr_args(item: u64, method: PrMergeMethod) -> Vec<String> {
    vec![
        "pr".to_string(),
        "merge".to_string(),
        item.to_string(),
        method.flag().to_string(),
    ]
}

/// # Errors
///
/// Whatever `gh` refused with — a branch protection rule's sentence is the
/// useful one, and nothing here replaces it.
pub fn merge_pr(cwd: &Path, item: u64, method: PrMergeMethod) -> Result<(), GhError> {
    gh_run(cwd, &merge_pr_args(item, method)).map(|_| ())
}

/// The comment travels on stdin (`--body-file -`), never argv — a process
/// list must not read what somebody wrote.
///
/// # Errors
///
/// Whatever `gh` refused with; nothing is invented here.
pub fn comment_work_item(cwd: &Path, kind: ItemKind, item: u64, body: &str) -> Result<(), GhError> {
    let _auth = AUTH_LIFECYCLE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    comment_work_item_with(&ProcessRunner, cwd, kind, item, body)
}

fn comment_work_item_with(
    runner: &impl CliRunner,
    cwd: &Path,
    kind: ItemKind,
    item: u64,
    body: &str,
) -> Result<(), GhError> {
    checked(call_gh(
        runner,
        Some(cwd),
        &[
            kind.word().to_string(),
            "comment".to_string(),
            item.to_string(),
            "--body-file".to_string(),
            "-".to_string(),
        ],
        Some(body.as_bytes()),
        None,
    )?)?;
    Ok(())
}

/// Close or reopen, through the verb `gh` owns — permission and state
/// validation stay on GitHub's side of the boundary.
///
/// # Errors
///
/// Whatever `gh` refused with.
pub fn set_work_item_open(
    cwd: &Path,
    kind: ItemKind,
    item: u64,
    open: bool,
) -> Result<(), GhError> {
    let verb = if open { "reopen" } else { "close" };
    checked(run_gh(
        Some(cwd),
        &[kind.word().to_string(), verb.to_string(), item.to_string()],
        None,
    )?)?;
    Ok(())
}

/// The rows the tab draws: both lists when the search asks for both, merged
/// and newest first.
///
/// A list that fails while the other answers is NOT a failure of the panel —
/// Orca throws only when nothing at all came back (`successfulRequestCount ===
/// 0`, :2412132), and a repository whose issues are turned off still has pull
/// requests worth showing.
///
/// # Errors
///
/// [`GhError::Missing`] when there is no `gh` on the machine, and whatever `gh`
/// said otherwise — but only when every list refused.
pub fn fetch_work_items(
    cwd: &Path,
    preset: &str,
    typed: &str,
    filters: &WorkItemFilters,
) -> Result<Vec<WorkItem>, GhError> {
    let plan = search_plan(preset, typed, filters);
    if plan.too_large {
        return Ok(Vec::new());
    }
    let mut rows: Vec<WorkItem> = Vec::new();
    let mut answered = 0usize;
    let mut refusal: Option<GhError> = None;
    for kind in plan.asks {
        match gh_run(cwd, &work_item_args(kind, &plan.query, WORK_ITEM_LIMIT))
            .and_then(|raw| read_work_items(&raw, kind))
        {
            Ok(found) => {
                answered += 1;
                rows.extend(found);
            }
            Err(error) => {
                if refusal.is_none() {
                    refusal = Some(error);
                }
            }
        }
    }
    if answered == 0
        && let Some(error) = refusal
    {
        return Err(error);
    }
    // `sortWorkItemsByNumber` then `slice(0, limit)` (:2411600 area): the two
    // lists are each capped at the limit, so the merge has to be cut again or
    // the tab shows twice what it promised.
    rows.sort_by(|left, right| right.number.cmp(&left.number));
    rows.truncate(WORK_ITEM_LIMIT);
    Ok(rows)
}

/// One `assignees` page, shaped into logins. Split from the fetch so the
/// shape has a test without a process behind it.
fn read_logins(raw: &str) -> Result<Vec<String>, GhError> {
    let body = parse_json(raw)?;
    Ok(body
        .as_array()
        .map(|list| list.iter().filter_map(|who| text(who, "login")).collect())
        .unwrap_or_default())
}

/// Everybody this repository can assign (`listAssignableUsers`, measured:
/// `repos/{owner}/{repo}/assignees?per_page=100`). The placeholders stay —
/// `gh` fills them from the checkout, so the resolved-slug round-trip Orca
/// spends first is already in `gh`'s own hand. One wide page, no paging:
/// a bench past a hundred names needs a search box this popover lacks.
pub fn fetch_assignable_users(cwd: &Path) -> Result<Vec<String>, GhError> {
    let raw = gh_run(
        cwd,
        &[
            "api".to_string(),
            "repos/{owner}/{repo}/assignees?per_page=100".to_string(),
        ],
    )?;
    read_logins(&raw)
}

/// Every project's answer, folded into the one list the tab draws.
///
/// Lenient the way a read across sources has to be: one repository that
/// cannot answer (no GitHub remote, a token short of a scope) does not blank
/// the projects that can. Only everybody refusing is a refusal — and then it
/// is the first one, not a chorus.
fn merge_project_answers(
    answers: Vec<(String, Result<Vec<WorkItem>, GhError>)>,
) -> Result<Vec<WorkItem>, GhError> {
    let mut rows: Vec<WorkItem> = Vec::new();
    let mut answered = 0usize;
    let mut refusal: Option<GhError> = None;
    for (home, answer) in answers {
        match answer {
            Ok(found) => {
                answered += 1;
                rows.extend(found.into_iter().map(|mut row| {
                    row.project = Some(home.clone());
                    row
                }));
            }
            Err(error) => {
                if refusal.is_none() {
                    refusal = Some(error);
                }
            }
        }
    }
    if answered == 0
        && let Some(error) = refusal
    {
        return Err(error);
    }
    // Numbers order one repository; across repositories they collide meaning
    // nothing. The merged list stands by when things last moved — the column
    // the table already dates every row with. Recorded deviation from the
    // single-project sort.
    rows.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(rows)
}

/// The tab's read across every chosen project — one `gh` per project, side
/// by side, because N repositories should cost the slowest one and not the
/// sum of all of them.
pub fn fetch_work_items_across(
    homes: &[(String, PathBuf)],
    preset: &str,
    typed: &str,
    filters: &WorkItemFilters,
) -> Result<Vec<WorkItem>, GhError> {
    let answers: Vec<(String, Result<Vec<WorkItem>, GhError>)> = std::thread::scope(|scope| {
        let hands: Vec<_> = homes
            .iter()
            .map(|(home, root)| {
                scope.spawn(move || (home.clone(), fetch_work_items(root, preset, typed, filters)))
            })
            .collect();
        hands
            .into_iter()
            .map(|hand| hand.join().expect("a project read does not panic"))
            .collect()
    });
    merge_project_answers(answers)
}

/// Where this repository lives on the web — the three addresses the page's own
/// buttons lead to (a new issue, the issue list, the pull request list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubWebUrls {
    pub repo: String,
    pub issues: String,
    pub pulls: String,
    pub new_issue: String,
}

/// One `gh repo view` answer, as the addresses around it.
///
/// The repository's own `url` is ASKED for rather than composed: an enterprise
/// checkout does not live on github.com, and composing that host would send
/// somebody to a repository that is not theirs. The slug is the fallback for a
/// `gh` that answers only `nameWithOwner`, and only there does the public host
/// get written down.
pub fn read_web_urls(raw: &str) -> Result<GithubWebUrls, GhError> {
    let body = parse_json(raw)?;
    let repo = text(&body, "url")
        .or_else(|| text(&body, "nameWithOwner").map(|slug| format!("https://github.com/{slug}")))
        .ok_or_else(|| GhError::Unreadable("GitHub repository view has no url".into()))?;
    let repo = repo.trim_end_matches('/').to_string();
    Ok(GithubWebUrls {
        issues: format!("{repo}/issues"),
        pulls: format!("{repo}/pulls"),
        new_issue: format!("{repo}/issues/new"),
        repo,
    })
}

/// # Errors
///
/// [`GhError::Missing`] when there is no `gh` on the machine, and whatever
/// `gh` refused with — a checkout with no GitHub remote refuses here, which is
/// the same fact the page's undetected-source state already says.
pub fn web_urls(cwd: &Path) -> Result<GithubWebUrls, GhError> {
    gh_run(
        cwd,
        &[
            "repo".to_string(),
            "view".to_string(),
            "--json".to_string(),
            "url,nameWithOwner".to_string(),
        ],
    )
    .and_then(|raw| read_web_urls(&raw))
}

/* ---- checking a pull request out --------------------------------------------
 *
 * The pure half of Orca's `resolveGitHubPrStartPoint` (out/main/index.js
 * @6338694). The impure half is in the shell, where the `Vcs` boundary is —
 * these three are the strings that road is made of, and they are here so they
 * can be tested without a repository.
 */

/// Which remote a pull request is fetched from
/// (`pickPreferredGitRemote`, :6345961), given `git remote`'s output.
///
/// `origin` wins whenever it exists; a repository with exactly one remote uses
/// it whatever it is called. Anything else is a question this window cannot
/// answer by guessing, so it comes back as the list it could not choose
/// between — empty when there are no remotes at all.
///
/// # Errors
///
/// The remotes it could not pick from, for the caller to say in its own words.
pub fn pick_remote(listed: &str) -> Result<String, Vec<String>> {
    let cleaned: Vec<String> = listed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if cleaned.iter().any(|remote| remote == "origin") {
        return Ok("origin".to_string());
    }
    match cleaned.len() {
        1 => Ok(cleaned[0].clone()),
        _ => Err(cleaned),
    }
}

/// Where a branch of `remote` is kept locally.
#[must_use]
pub fn tracking_ref(remote: &str, branch: &str) -> String {
    format!("refs/remotes/{remote}/{branch}")
}

/// The refspec that brings a pull request's head branch in
/// (`fetchPrHeadTrackingRef`, :6344467 — `+refs/heads/<branch>:<tracking>`).
///
/// Written out rather than `git fetch <remote> <branch>` because the `+` is the
/// difference between a fetch that updates a branch somebody force-pushed and
/// one that fails on it, and a pull request branch is the branch people force-
/// push.
#[must_use]
pub fn head_refspec(remote: &str, branch: &str) -> String {
    format!("+refs/heads/{branch}:{}", tracking_ref(remote, branch))
}

/// How long a pull request head may take to arrive
/// (`REVIEW_HEAD_FETCH_TIMEOUT_MS = 6e4`, :6337648).
pub const HEAD_FETCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor_cli::FakeRunner;
    use zerocode_core::checks::CheckTally;

    fn success(stdout: &str) -> Result<CliOutput, CliError> {
        Ok(CliOutput {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        })
    }

    fn failure(stdout: &str, stderr: &str) -> Result<CliOutput, CliError> {
        Ok(CliOutput {
            success: false,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        })
    }

    fn auth_fixture(active: &str) -> String {
        format!(
            r#"{{"hosts":{{"github.com":[
              {{"login":"hana","active":{},"state":"success","tokenSource":"keyring"}},
              {{"login":"mira","active":{},"state":"success","tokenSource":"keyring"}}
            ]}}}}"#,
            active == "hana",
            active == "mira"
        )
    }

    #[test]
    fn auth_status_is_host_relevant_stable_and_secret_free() {
        let raw = r#"{"hosts":{
          "github.com":[
            {"login":"Hana","active":true,"state":"success","tokenSource":"keyring",
             "token":"ghp_never_cross_ipc","scopes":["repo"]},
            {"login":"env-user","active":false,"state":"success","tokenSource":"GH_TOKEN"},
            {"login":"odd-user","active":false,"state":"success","tokenSource":"token file"},
            {"login":"narrow","active":false,"state":"success","tokenSource":"keyring",
             "scopes":"gist, read:org"}
          ],
          "broken.example":[
            {"login":"old","active":true,"state":"failure","tokenSource":"keyring"}
          ]
        }}"#;
        let first = read_auth_status(raw, Some("github.com".into())).expect("status");
        let again = read_auth_status(
            &raw.replace("\"Hana\"", "\"hana\""),
            Some("GITHUB.COM".to_ascii_lowercase()),
        )
        .expect("status again");
        let hana = first
            .accounts
            .iter()
            .find(|account| account.login == "Hana")
            .expect("hana account");
        let hana_again = again
            .accounts
            .iter()
            .find(|account| account.login == "hana")
            .expect("hana account again");
        let environment = first
            .accounts
            .iter()
            .find(|account| account.login == "env-user")
            .expect("environment account");
        assert_eq!(first.repo_standing, "connected");
        assert_eq!(first.active_account_id.as_deref(), Some(hana.id.as_str()));
        assert_eq!(hana.id, hana_again.id);
        assert!(hana.selectable);
        assert!(!environment.selectable);
        assert_eq!(environment.credential_source, "environment");
        // The variable's NAME crosses deliberately — the settings card says
        // which variable hides the keyring login. A token-source string that
        // is not shaped like a variable name is dropped, not guessed at.
        assert_eq!(environment.env_credential.as_deref(), Some("GH_TOKEN"));
        assert_eq!(hana.env_credential, None);
        let odd = first
            .accounts
            .iter()
            .find(|account| account.login == "odd-user")
            .expect("odd account");
        assert_eq!(odd.credential_source, "environment");
        assert_eq!(odd.env_credential, None);
        // Scope verdicts: judged against REQUIRED_SCOPES in both shapes the
        // CLI reports (array and comma-string); an absent report stays empty
        // rather than being read as "missing everything".
        assert!(
            hana.scope_gaps.is_empty(),
            "array-shape repo scope qualifies"
        );
        let narrow = first
            .accounts
            .iter()
            .find(|account| account.login == "narrow")
            .expect("narrow account");
        assert_eq!(narrow.scope_gaps, vec!["repo".to_string()]);
        assert!(odd.scope_gaps.is_empty(), "no scope report is not a gap");

        let serialized = serde_json::to_string(&first).expect("serialize status");
        // The token's own scope list stays behind the boundary — only the
        // judged gap (our constant's names) crosses.
        for secret_mark in [
            "ghp_never_cross_ipc",
            "tokenSource",
            "scopes",
            "gist",
            "read:org",
        ] {
            assert!(
                !serialized.contains(secret_mark),
                "auth status leaked `{secret_mark}`: {serialized}"
            );
        }
    }

    #[test]
    fn nonzero_auth_status_keeps_its_valid_account_snapshot() {
        let runner = FakeRunner::with([failure(
            r#"{"hosts":{"github.com":[
              {"login":"hana","active":true,"state":"success","tokenSource":"keyring"},
              {"login":"old","active":false,"state":"failure","tokenSource":"keyring"}
            ]}}"#,
            "authentication failed for one account",
        )]);
        let status = integration_status_with(
            &runner,
            Path::new("/repo"),
            Some("git@github.com:acme/tool.git"),
            false,
        )
        .expect("valid JSON was discarded because of the exit code");

        assert_eq!(status.accounts.len(), 2);
        assert_eq!(status.repo_standing, "connected");
        assert_eq!(
            status.active_account_id,
            Some(github_account_id("github.com", "hana"))
        );
    }

    #[test]
    fn connection_test_probes_only_the_active_account_with_an_exact_host() {
        let hana = github_account_id("github.com", "hana");
        let runner = FakeRunner::with([
            success(&auth_fixture("hana")),
            success(r#"{"login":"Hana","token":"never-read"}"#),
        ]);
        let report = test_connection_with(
            &runner,
            Path::new("/repo"),
            Some("https://github.com/acme/tool.git"),
            &hana,
        )
        .expect("active account test");

        assert_eq!(report.account_id, hana);
        assert_eq!(report.standing, "connected");
        assert_eq!(
            runner.calls()[1].args,
            ["api", "--hostname", "github.com", "user"]
        );
        assert_eq!(runner.calls()[1].budget, Some(CONNECTION_TEST_BUDGET));

        let mira = github_account_id("github.com", "mira");
        let inactive = FakeRunner::with([success(&auth_fixture("hana"))]);
        let refused = test_connection_with(
            &inactive,
            Path::new("/repo"),
            Some("git@github.com:acme/tool.git"),
            &mira,
        )
        .expect_err("inactive account was probed");
        assert!(matches!(refused, GhError::Refused(_)));
        assert_eq!(inactive.calls().len(), 1, "inactive account ran `gh api`");
    }

    #[test]
    fn connection_test_failure_does_not_forward_cli_diagnostics() {
        let hana = github_account_id("github.com", "hana");
        let runner = FakeRunner::with([
            success(&auth_fixture("hana")),
            failure("", "token ghp_should_not_cross_ipc is invalid"),
        ]);
        let failure = test_connection_with(
            &runner,
            Path::new("/repo"),
            Some("git@github.com:acme/tool.git"),
            &hana,
        )
        .expect_err("failed live probe reported success");

        assert_eq!(
            failure.detail(),
            Some("GitHub rejected the connection test")
        );
    }

    #[test]
    fn account_changes_bypass_responses_cached_under_the_previous_login() {
        assert_eq!(
            api_args("repos/acme/tool", true),
            ["api", "--cache", "60s", "repos/acme/tool"]
        );
        assert_eq!(
            api_args("repos/acme/tool", false),
            ["api", "repos/acme/tool"]
        );
    }

    #[test]
    fn account_switch_and_logout_use_fresh_ids_and_return_canonical_state() {
        let remote = "git@github.com:acme/tool.git";
        let mira = github_account_id("github.com", "mira");
        let switch = FakeRunner::with([
            success(&auth_fixture("hana")),
            success(""),
            success(&auth_fixture("mira")),
            success(r#"{"nameWithOwner":"acme/tool"}"#),
        ]);
        let selected = account_mutation_with(
            &switch,
            Path::new("/repo"),
            Some(remote),
            &mira,
            "switch",
            || {},
        )
        .expect("switch");
        assert_eq!(selected.active_account_id.as_deref(), Some(mira.as_str()));
        assert_eq!(selected.repo_standing, "connected");
        assert_eq!(
            switch.calls()[1].args,
            [
                "auth",
                "switch",
                "--hostname",
                "github.com",
                "--user",
                "mira"
            ]
        );

        let logout = FakeRunner::with([
            success(&auth_fixture("mira")),
            success(""),
            success(r#"{"hosts":{"github.com":[]}}"#),
        ]);
        let cleared = account_mutation_with(
            &logout,
            Path::new("/repo"),
            Some(remote),
            &mira,
            "logout",
            || {},
        )
        .expect("logout");
        assert!(cleared.accounts.is_empty());
        assert_eq!(cleared.active_account_id, None);
        assert_eq!(
            logout.calls()[1].args,
            [
                "auth",
                "logout",
                "--hostname",
                "github.com",
                "--user",
                "mira"
            ]
        );
        for call in switch.calls().into_iter().chain(logout.calls()) {
            assert!(
                !call
                    .args
                    .iter()
                    .any(|arg| arg == "--show-token" || arg == "token")
            );
        }
    }

    #[test]
    fn an_environment_account_cannot_claim_a_mutation() {
        let raw = r#"{"hosts":{"github.com":[
          {"login":"ci","active":true,"state":"success","tokenSource":"GH_TOKEN"}
        ]}}"#;
        let runner = FakeRunner::with([success(raw)]);
        let id = github_account_id("github.com", "ci");
        let refused = account_mutation_with(
            &runner,
            Path::new("/repo"),
            Some("https://github.com/acme/tool.git"),
            &id,
            "logout",
            || {},
        )
        .expect_err("environment credential was removed");
        assert!(matches!(refused, GhError::Refused(_)));
        assert_eq!(runner.calls().len(), 1, "a mutation command still ran");
    }

    #[test]
    fn remote_hosts_and_login_intents_are_data_not_shell_syntax() {
        assert_eq!(
            remote_host("git@GitHub.COM:acme/tool.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            remote_host("ssh://git@ghe.example:2222/acme/tool.git").as_deref(),
            Some("ghe.example")
        );
        assert_eq!(
            remote_host("https://user:secret@ghe.example/acme/tool.git").as_deref(),
            Some("ghe.example")
        );
        assert_eq!(remote_host("https://good.example;echo/pwn"), None);
        assert_eq!(
            login_command(Some("git@ghe.example:acme/tool.git")),
            "gh auth login --hostname ghe.example --web"
        );
        assert_eq!(
            login_command(Some("https://good.example;echo/pwn")),
            "gh auth login --web"
        );
    }

    #[test]
    fn a_run_takes_the_providers_page_over_githubs() {
        let runs = read_check_runs(
            r#"{"check_runs":[
                {"id":11,"name":"build","status":"completed","conclusion":"success",
                 "details_url":"https://ci.example.com/b/1",
                 "html_url":"https://github.com/o/r/runs/11"}]}"#,
        )
        .expect("the list did not parse");
        assert_eq!(runs[0].url.as_deref(), Some("https://ci.example.com/b/1"));
        assert_eq!(runs[0].check_run_id, Some(11));
        // A non-Actions details URL carries no run id, and nothing was nested.
        assert_eq!(runs[0].workflow_run_id, None);
    }

    #[test]
    fn a_workflow_run_id_is_found_in_the_url_or_the_nesting() {
        let runs = read_check_runs(
            r#"{"check_runs":[
                {"id":1,"name":"a","status":"completed","conclusion":"failure",
                 "details_url":"https://github.com/o/r/actions/runs/900/job/7"},
                {"id":2,"name":"b","status":"completed","conclusion":"success",
                 "html_url":"https://ci.example.com/x",
                 "check_suite":{"workflow_run":{"id":901}}}]}"#,
        )
        .expect("the list did not parse");
        assert_eq!(runs[0].workflow_run_id, Some(900));
        assert_eq!(runs[1].workflow_run_id, Some(901));
    }

    #[test]
    fn a_running_check_is_pending_whatever_its_conclusion_says() {
        let runs = read_check_runs(
            r#"{"check_runs":[
                {"id":1,"name":"test","status":"in_progress","conclusion":"failure"}]}"#,
        )
        .expect("the list did not parse");
        assert_eq!(runs[0].status, CheckStatus::InProgress);
        assert_eq!(runs[0].conclusion, Some(CheckConclusion::Pending));
        assert_eq!(CheckTally::of(&runs).pending, 1);
    }

    #[test]
    fn a_status_whose_name_a_run_already_used_is_dropped() {
        let runs = read_check_runs(
            r#"{"check_runs":[{"id":1,"name":"ci/test","status":"completed","conclusion":"success"}]}"#,
        )
        .expect("the list did not parse");
        let statuses = read_commit_statuses(
            r#"{"statuses":[
                {"context":"ci/test","state":"success","target_url":"https://x/1"},
                {"context":"legacy/lint","state":"error","target_url":"https://x/2"}]}"#,
            &runs,
        )
        .expect("the statuses did not parse");
        let names: Vec<&str> = statuses.iter().map(|check| check.name.as_str()).collect();
        assert_eq!(names, ["legacy/lint"], "a check was listed twice");
        assert_eq!(statuses[0].conclusion, Some(CheckConclusion::Failure));
    }

    #[test]
    fn a_suite_blocked_on_a_person_is_listed_when_it_has_no_run() {
        let runs = read_check_runs(
            r#"{"check_runs":[{"id":1,"name":"build","status":"completed","conclusion":"success",
                 "details_url":"https://github.com/o/r/actions/runs/500"}]}"#,
        )
        .expect("the list did not parse");
        let blocked = read_blocked_suites(
            r#"{"check_suites":[
                {"id":500,"conclusion":"action_required","app":{"name":"GitHub Actions"}},
                {"id":501,"conclusion":"ACTION_REQUIRED","app":{"name":"Deploy"}},
                {"id":502,"conclusion":"success","app":{"name":"Other"}}]}"#,
            "abc123",
            "o/r",
            &runs,
        )
        .expect("the suites did not parse");
        // 500 already has a run listed; 502 is not blocked.
        let names: Vec<&str> = blocked.iter().map(|check| check.name.as_str()).collect();
        assert_eq!(names, ["Deploy"]);
        assert_eq!(blocked[0].conclusion, Some(CheckConclusion::ActionRequired));
        assert_eq!(
            blocked[0].url.as_deref(),
            Some("https://github.com/o/r/commit/abc123/checks?check_suite_id=501")
        );
    }

    #[test]
    fn a_details_pane_reads_the_output_a_linter_wrote() {
        let details = read_check_details(
            r#"{"name":"clippy","status":"completed","conclusion":"failure",
                "html_url":"https://github.com/o/r/runs/9","started_at":"2026-08-07T01:02:03Z",
                "output":{"title":"2 warnings","summary":"needless clone","text":null}}"#,
            "fallback",
        )
        .expect("the details did not parse");
        assert_eq!(details.name, "clippy");
        assert_eq!(details.title.as_deref(), Some("2 warnings"));
        assert_eq!(details.summary.as_deref(), Some("needless clone"));
        assert_eq!(details.text, None);
        assert_eq!(details.started_at.as_deref(), Some("2026-08-07T01:02:03Z"));

        // An empty name falls back rather than drawing a nameless pane.
        let bare = read_check_details(r#"{"name":"  "}"#, "legacy/lint").expect("did not parse");
        assert_eq!(bare.name, "legacy/lint");
    }

    #[test]
    fn only_the_jobs_of_this_check_are_shown_when_one_matches() {
        let raw = r#"{"jobs":[
            {"id":1,"name":"test (macos)","status":"completed","conclusion":"failure",
             "steps":[{"name":"cargo test","status":"completed","conclusion":"failure"}]},
            {"id":2,"name":"test (linux)","status":"completed","conclusion":"success","steps":[]}]}"#;
        let mine = read_jobs(raw, Some("test (macos)")).expect("the jobs did not parse");
        assert_eq!(mine.len(), 1);
        assert!(mine[0].failed());
        assert!(mine[0].steps[0].failed());
        // No exact match — the check IS the workflow, so all of them.
        let all = read_jobs(raw, Some("build")).expect("the jobs did not parse");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn an_annotation_without_a_message_still_draws() {
        let annotations = read_annotations(
            r#"[{"path":"src/a.rs","start_line":12,"annotation_level":"failure","title":"E0308"},
                {"path":null,"message":"no file"}]"#,
        )
        .expect("the annotations did not parse");
        assert_eq!(annotations[0].message, "");
        assert_eq!(annotations[0].start_line, Some(12));
        assert_eq!(annotations[1].path, None);
        // A non-array body is empty, not an error: GitHub answers `[]` for a
        // run with no annotations and 404s for one that cannot have them.
        assert!(read_annotations("{}").expect("did not parse").is_empty());
    }

    #[test]
    fn a_pull_request_without_a_head_repository_is_no_review() {
        // `gh` answers `{}` when the branch has no PR, and a fork whose head
        // repository was deleted cannot be asked about.
        assert_eq!(read_pull_request("{}").expect("did not parse"), None);
        assert_eq!(
            read_pull_request(r#"{"number":7,"title":"t"}"#).expect("did not parse"),
            None
        );

        let review = read_pull_request(
            r#"{"number":7,"title":"Port the checks panel","url":"https://github.com/o/r/pull/7",
                "state":"OPEN","isDraft":true,"headRefOid":"deadbeef",
                "headRepository":{"name":"r"},"headRepositoryOwner":{"login":"o"},
                "mergeable":"CONFLICTING","updatedAt":"2026-08-17T12:00:00Z","baseRefName":"main"}"#,
        )
        .expect("did not parse")
        .expect("a review with a head repository was dropped");
        assert_eq!(review.owner_repo, "o/r");
        assert_eq!(review.state, "open", "the state kept GitHub's casing");
        assert!(review.draft);
        assert_eq!(review.head_sha, "deadbeef");
        assert_eq!(
            review.mergeable.as_deref(),
            Some("conflicting"),
            "mergeable kept GitHub's casing"
        );
        assert_eq!(review.updated_at.as_deref(), Some("2026-08-17T12:00:00Z"));
        assert_eq!(review.base_ref.as_deref(), Some("main"));

        // An older `gh` that has never heard of these fields answers without
        // them, and that must stay an answer — not a dropped review.
        let bare = read_pull_request(
            r#"{"number":8,"title":"t","url":"u","state":"OPEN","isDraft":false,
                "headRefOid":"cafe","headRepository":{"name":"r"},
                "headRepositoryOwner":{"login":"o"}}"#,
        )
        .expect("did not parse")
        .expect("a review without mergeability was dropped");
        assert_eq!(bare.mergeable, None);
        assert_eq!(bare.updated_at, None);
        assert_eq!(bare.base_ref, None);
    }

    /// The board's card wears one of four words, and GitHub shouts three of
    /// them.
    #[test]
    fn a_refused_pr_view_is_no_review_even_with_a_body_on_stdout() {
        // `gh` prints error bodies on stdout at times — the exit standing is
        // the verdict, and a refused call must not dress a card.
        let body = r#"{"number":7,"state":"OPEN","isDraft":false}"#;
        let refused = CliOutput {
            success: false,
            stdout: body.to_string(),
            stderr: String::new(),
        };
        assert_eq!(review_state_of(refused), None);
        let answered = CliOutput {
            success: true,
            stdout: body.to_string(),
            stderr: String::new(),
        };
        assert_eq!(
            review_state_of(answered),
            Some(ReviewMark {
                number: 7,
                state: "open"
            })
        );
    }

    #[test]
    fn a_review_wears_one_of_four_words_whatever_case_github_shouts() {
        for (state, draft, word) in [
            ("OPEN", false, "open"),
            ("open", false, "open"),
            ("OPEN", true, "draft"),
            ("Open", true, "draft"),
            ("MERGED", false, "merged"),
            ("merged", false, "merged"),
            // A merged review is never a draft, and reading the flag first
            // would say it was.
            ("MERGED", true, "merged"),
            ("CLOSED", false, "closed"),
            ("Closed", true, "closed"),
            // A word this table has not met is something to link to, not a
            // fifth tone.
            ("QUEUED", false, "open"),
            ("", false, "open"),
        ] {
            assert_eq!(
                review_word(state, draft),
                word,
                "`{state}` with draft={draft} does not wear `{word}`"
            );
        }

        // `gh` answers `{}` for a branch nobody opened a review for, and a body
        // with no number is that answer whatever else it carries.
        assert_eq!(read_review_state("{}").expect("did not parse"), None);
        assert_eq!(
            read_review_state(r#"{"state":"OPEN","isDraft":false}"#).expect("did not parse"),
            None
        );
        assert_eq!(
            read_review_state(r#"{"number":12,"state":"OPEN","isDraft":true}"#)
                .expect("did not parse"),
            Some(ReviewMark {
                number: 12,
                state: "draft"
            })
        );
        assert_eq!(
            read_review_state(r#"{"number":9,"state":"MERGED"}"#).expect("did not parse"),
            Some(ReviewMark {
                number: 9,
                state: "merged"
            })
        );
        // Not JSON at all is unreadable, which the caller turns into "no
        // review" rather than a card that says something went wrong.
        assert!(read_review_state("gh: command not found").is_err());
    }

    #[test]
    fn a_preset_chip_is_a_kind_and_one_slice_of_it() {
        // 신판 실측의 두 단(종류 × 프리셋)이 묻는 다섯 쌍. 표인 것은 그것이
        // 표이기 때문이고, 한 한정자의 오타가 남의 작업을 조용히 답하는
        // 검색이기 때문이다. 순서까지 재는 것은 이 문자열이 **검색칸에 보이는
        // 그 문장**이라서다.
        for (kind, chip, query) in [
            (ItemKind::Issue, "open", "is:issue is:open"),
            (ItemKind::Issue, "assigned", "is:issue is:open assignee:@me"),
            (ItemKind::Pr, "open", "is:pr is:open"),
            (ItemKind::Pr, "mine", "is:pr is:open author:@me"),
            (ItemKind::Pr, "review", "is:pr is:open review-requested:@me"),
        ] {
            let picked = GhPreset::from_word(chip).expect("a word the window sends");
            assert_eq!(preset_query(kind, picked), Some(query), "chip `{chip}`");
        }

        // 창이 보낸 낱말은 추측되지 않는다.
        assert_eq!(ItemKind::from_word("issue"), Some(ItemKind::Issue));
        assert_eq!(ItemKind::from_word("pr"), Some(ItemKind::Pr));
        for junk in ["", "mr", "issues", "PR"] {
            assert_eq!(ItemKind::from_word(junk), None, "`{junk}` was guessed at");
        }
        for junk in ["", "OPEN", "my-issues", "assignee"] {
            assert_eq!(GhPreset::from_word(junk), None, "`{junk}` was guessed at");
        }

        // 없는 쌍은 빈 목록이 아니라 거절이다: 이슈에는 리뷰어도, 「내 것」도
        // 없다 — 그런 질의의 빈 답은 「GitHub에 없다」로 읽힌다.
        for (kind, chip) in [
            (ItemKind::Issue, GhPreset::Mine),
            (ItemKind::Issue, GhPreset::ReviewNeeded),
            (ItemKind::Pr, GhPreset::AssignedToMe),
        ] {
            assert_eq!(preset_query(kind, chip), None, "{kind:?} + {chip:?}");
        }

        // 컴포저의 낱말 다섯은 같은 표를 지난다 — 문법의 사본은 하나뿐이다.
        for (preset, query) in [
            ("all", "is:issue is:open"),
            ("issues", "is:issue is:open"),
            ("my-issues", "is:issue is:open assignee:@me"),
            ("prs", "is:pr is:open"),
            ("my-prs", "is:pr is:open author:@me"),
            ("review", "is:pr is:open review-requested:@me"),
        ] {
            assert_eq!(
                composer_preset_query(preset),
                Some(query),
                "preset `{preset}`"
            );
        }
        // No preset is not a sixth preset: it is the composer's own first
        // state, and the plan below is what it means.
        assert_eq!(composer_preset_query(""), None);
        assert_eq!(composer_preset_query("my-prs-and-issues"), None);
    }

    /// 웹으로 나가는 단추 셋이 이 저장소로 간다.
    ///
    /// 주소를 짓는 자리가 하나인 이유는 문법의 자리가 하나인 이유와 같다:
    /// 창이 호스트를 지으면 엔터프라이즈 체크아웃의 「새 이슈」가 남의
    /// github.com 저장소를 연다.
    #[test]
    fn the_web_buttons_lead_to_this_repository() {
        let urls =
            read_web_urls(r#"{"url":"https://github.com/acme/tool","nameWithOwner":"acme/tool"}"#)
                .expect("a readable repository view");
        assert_eq!(urls.repo, "https://github.com/acme/tool");
        assert_eq!(urls.issues, "https://github.com/acme/tool/issues");
        assert_eq!(urls.pulls, "https://github.com/acme/tool/pulls");
        assert_eq!(urls.new_issue, "https://github.com/acme/tool/issues/new");

        // 엔터프라이즈는 github.com에 없다 — 주소는 `gh`가 한 말 그대로다.
        let hosted = read_web_urls(r#"{"url":"https://git.acme.example/team/tool/"}"#)
            .expect("a readable repository view");
        assert_eq!(hosted.repo, "https://git.acme.example/team/tool");
        assert_eq!(
            hosted.new_issue,
            "https://git.acme.example/team/tool/issues/new"
        );

        // 슬러그만 아는 답에는 공개 호스트가 유일한 답이고,
        let slug = read_web_urls(r#"{"nameWithOwner":"acme/tool"}"#).expect("a slug is enough");
        assert_eq!(slug.repo, "https://github.com/acme/tool");
        // 아무것도 말하지 않는 답에는 답이 없다 — 지어내지 않는다.
        assert!(read_web_urls("{}").is_err());
        assert!(read_web_urls(r#"{"url":"  "}"#).is_err());
    }

    #[test]
    fn a_search_asks_the_halves_its_qualifiers_can_answer() {
        // No filters picked, which is what the tab opens with — this whole
        // test is the shape of the question BEFORE the popover exists, and it
        // still has to be that shape after.
        let bare = WorkItemFilters::default();
        let plan = |preset: &str, typed: &str| search_plan(preset, typed, &bare);

        // Nothing chosen and nothing typed: both lists, the repository's
        // recent open work.
        let opening = plan("", "");
        assert_eq!(opening.asks, [ItemKind::Pr, ItemKind::Issue]);
        assert_eq!(opening.query, "is:open");

        // A preset that names one half asks that half alone.
        assert_eq!(plan("my-issues", "").asks, [ItemKind::Issue]);
        assert_eq!(plan("prs", "").asks, [ItemKind::Pr]);
        // Reviewers are a thing only a pull request has — asking the issue
        // list for them is not a narrower search, it is an empty one.
        assert_eq!(plan("review", "").asks, [ItemKind::Pr]);

        // Typed words ride BESIDE the preset rather than replacing it.
        let both = plan("my-prs", "login redirect");
        assert_eq!(both.query, "is:pr is:open author:@me login redirect");
        assert_eq!(both.asks, [ItemKind::Pr]);
        // Free text alone still reaches both halves.
        assert_eq!(plan("", "drain gate").query, "drain gate");
        assert_eq!(
            plan("", "  drain gate  ").asks,
            [ItemKind::Pr, ItemKind::Issue]
        );

        // A pasted page is not a query. Nothing is asked and the tab says so.
        let pasted = plan("", &"x".repeat(QUERY_MAX_BYTES + 1));
        assert!(pasted.too_large);
        assert!(pasted.asks.is_empty());
        assert!(!plan("", &"x".repeat(QUERY_MAX_BYTES)).too_large);
    }

    /// The filter popover rides as values and lands as grammar HERE.
    ///
    /// Two things are being held. First that the window's structured choice
    /// reaches GitHub as the qualifier it means — a filter nobody spells is a
    /// filter nobody applies, and the list would answer as if it were never
    /// picked. Second that a state only a pull request can be in pulls the
    /// scope with it: `state:merged` asked of the issue list is an empty half,
    /// which reads as "GitHub has nothing" rather than "issues never merge".
    #[test]
    fn a_picked_filter_reaches_github_as_the_qualifier_it_means() {
        let picked = |state: Option<&str>, draft: bool| WorkItemFilters {
            state: state.map(str::to_string),
            draft,
            ..WorkItemFilters::default()
        };

        // A state rides the plan and narrows nothing else: open and closed are
        // words both halves answer, so both halves are still asked.
        let closed = search_plan("", "", &picked(Some("closed"), false));
        assert_eq!(closed.query, "state:closed");
        assert_eq!(closed.asks, [ItemKind::Pr, ItemKind::Issue]);

        // Merged is a pull request's state, so it takes the scope with it.
        let merged = search_plan("", "", &picked(Some("merged"), false));
        assert_eq!(merged.query, "state:merged");
        assert_eq!(merged.asks, [ItemKind::Pr]);
        // As does draft, which the marker list already caught.
        let draft = search_plan("", "", &picked(None, true));
        assert_eq!(draft.query, "draft:true");
        assert_eq!(draft.asks, [ItemKind::Pr]);

        // Filters join AFTER the preset and after what was typed — the tail of
        // the sentence, in one order decided in one place.
        let all = search_plan("my-prs", "login redirect", &picked(Some("open"), true));
        assert_eq!(
            all.query,
            "is:pr is:open author:@me login redirect state:open draft:true"
        );

        // A word this file does not know is not a narrower search but an
        // unanswerable one, so it never leaves. Nor does an empty one.
        for junk in ["", "OPEN", "is:merged", "closed;drop", "any"] {
            let unknown = search_plan("", "", &picked(Some(junk), false));
            assert_eq!(
                unknown.query, "is:open",
                "`{junk}` was passed through to GitHub"
            );
        }

        // And the default filters are not a filter: byte for byte, the
        // question the tab asked before this popover existed.
        let bare = WorkItemFilters::default();
        assert_eq!(bare, picked(None, false));
        for (preset, typed, query) in [
            ("", "", "is:open"),
            ("my-issues", "", "is:issue is:open assignee:@me"),
            (
                "my-prs",
                "login redirect",
                "is:pr is:open author:@me login redirect",
            ),
            ("", "drain gate", "drain gate"),
        ] {
            assert_eq!(
                search_plan(preset, typed, &bare).query,
                query,
                "the unfiltered question changed for preset `{preset}`"
            );
        }
    }

    #[test]
    fn a_list_request_carries_the_state_in_the_search_and_the_sort_at_the_end() {
        let args = work_item_args(ItemKind::Pr, "is:pr is:open", 12);
        assert_eq!(args[0], "pr");
        assert_eq!(args[1], "list");
        // `--state all` with the state in the query: the flag and the
        // qualifier are two filters, and a default flag would intersect
        // `is:merged` with "open" and answer nothing.
        assert!(args.windows(2).any(|pair| pair == ["--state", "all"]));
        assert!(args.windows(2).any(|pair| pair == ["--limit", "12"]));
        assert_eq!(args.last().unwrap(), "is:pr is:open sort:created-desc");
        // The fields are the ones the row draws, and `headRefName` is the one
        // the whole checkout depends on.
        let fields = &args[args.iter().position(|arg| arg == "--json").unwrap() + 1];
        assert!(fields.contains("headRefName") && fields.contains("headRepositoryOwner"));
        assert!(
            !fields.contains("labels"),
            "a field nothing draws is being fetched"
        );
        // Nothing names a repository: `gh` reads that from the directory.
        assert!(!args.iter().any(|arg| arg == "--repo"));

        let issues = work_item_args(ItemKind::Issue, "is:issue is:open", 12);
        assert_eq!(issues[0], "issue");
        assert!(
            !issues[issues.iter().position(|a| a == "--json").unwrap() + 1].contains("headRefName")
        );
    }

    #[test]
    fn a_pull_request_row_knows_its_branch_its_base_and_whose_fork_it_is() {
        let rows = read_work_items(
            r#"[
              {"number":7,"title":"Port the checks panel","state":"OPEN","isDraft":false,
               "url":"https://github.com/acme/tool/pull/7","updatedAt":"2026-08-10T00:00:00Z",
               "author":{"login":"hana"},"headRefName":"checks-panel","baseRefName":"main",
               "assignees":[{"login":"hana"},{"login":"lee"}],
               "headRepositoryOwner":{"login":"acme"}},
              {"number":8,"title":"From a fork","state":"OPEN","isDraft":true,
               "url":"https://github.com/acme/tool/pull/8",
               "author":{"login":"outsider"},"headRefName":"patch-1","baseRefName":"main",
               "headRepositoryOwner":{"login":"outsider"}}
            ]"#,
            ItemKind::Pr,
        )
        .expect("the list did not parse");
        assert_eq!(rows[0].kind, "pr");
        assert_eq!(rows[0].branch.as_deref(), Some("checks-panel"));
        assert_eq!(rows[0].base.as_deref(), Some("main"));
        assert_eq!(rows[0].author.as_deref(), Some("hana"));
        assert!(!rows[0].cross_repo, "a same-repo PR was read as a fork");
        assert_eq!(rows[0].state, "open");
        // The 담당자 column's fact, in the API's order — and a row the API
        // sent without the field is an empty list, not a parse failure.
        assert_eq!(rows[0].assignees, ["hana", "lee"]);
        assert!(rows[1].assignees.is_empty());
        // The head owner is not this repository's owner — and the repository's
        // owner came out of the pull request's own URL, at no cost.
        assert!(rows[1].cross_repo);
        assert_eq!(rows[1].state, "draft", "a draft is its own state");
    }

    #[test]
    fn a_merged_pull_request_is_merged_whatever_else_it_was() {
        let rows = read_work_items(
            r#"[{"number":1,"title":"t","state":"MERGED","isDraft":true,
                 "url":"https://github.com/o/r/pull/1"},
                {"number":2,"title":"t","state":"CLOSED","isDraft":true,
                 "url":"https://github.com/o/r/pull/2"},
                {"title":"no number"}]"#,
            ItemKind::Pr,
        )
        .expect("the list did not parse");
        assert_eq!(rows.len(), 2, "a row without a number was drawn");
        assert_eq!(rows[0].state, "merged");
        assert_eq!(rows[1].state, "closed");
        // An issue list is a bare array too, and an answer that is not one is
        // an empty list rather than an error: `gh` prints `[]` for a
        // repository with issues turned off.
        assert!(
            read_work_items("{}", ItemKind::Issue)
                .expect("did not parse")
                .is_empty()
        );
    }

    #[test]
    fn the_remote_a_pull_request_is_fetched_from_is_origin_or_the_only_one() {
        assert_eq!(pick_remote("upstream\norigin\nfork\n").unwrap(), "origin");
        assert_eq!(pick_remote("  fork  \n").unwrap(), "fork");
        // Two remotes and no origin is a question, not a guess — the caller
        // gets the list so it can say which two.
        assert_eq!(
            pick_remote("upstream\nfork\n").unwrap_err(),
            ["upstream", "fork"]
        );
        assert_eq!(pick_remote("\n  \n").unwrap_err(), Vec::<String>::new());
    }

    #[test]
    fn a_head_fetch_forces_the_branch_people_force_push() {
        assert_eq!(
            head_refspec("origin", "feature/login"),
            "+refs/heads/feature/login:refs/remotes/origin/feature/login"
        );
        assert_eq!(
            tracking_ref("upstream", "main"),
            "refs/remotes/upstream/main"
        );
        // The leading `+` is the whole point: without it the fetch fails on a
        // pull request branch that was rebased, which is most of them.
        assert!(head_refspec("origin", "x").starts_with('+'));
    }

    /// 포크의 head가 어디 있는지, 그리고 그것이 없을 때.
    #[test]
    fn a_pull_requests_head_names_the_repository_it_can_be_pushed_to() {
        let raw = r#"{
            "maintainer_can_modify": false,
            "head": {
                "ref": "fix/typo",
                "repo": {
                    "full_name": "contributor/tool",
                    "name": "tool",
                    "owner": { "login": "Contributor" },
                    "clone_url": "https://github.com/contributor/tool.git",
                    "ssh_url": "git@github.com:contributor/tool.git"
                }
            }
        }"#;
        let head = read_push_head(raw).expect("read").expect("a head");
        assert_eq!(head.branch, "fix/typo");
        assert_eq!(head.maintainer_can_modify, Some(false));
        // 신원은 대소문자를 접는다 — GitHub이 접기 때문이다. 접지 않으면 이미
        // 있는 원격을 못 알아보고 같은 포크를 두 번 더한다.
        assert_eq!(head.identity(), "contributor/tool");

        // 지워진 포크: PR은 남고 `refs/pull/<n>/head`도 남지만 **밀 곳이 없다**,
        // 그리고 밀 곳이 이 함수의 질문 전부다.
        assert_eq!(
            read_push_head(r#"{"head":{"ref":"x","repo":null}}"#).expect("read"),
            None
        );
        // 필드 하나만 없어도 답이 아니다 — 절반짜리 원격은 더할 수 없다.
        assert_eq!(
            read_push_head(
                r#"{"head":{"ref":"x","repo":{"name":"t","owner":{"login":"o"},
                   "clone_url":"https://h/o/t.git"}}}"#
            )
            .expect("read"),
            None
        );
        // 있는데 안 적힌 것과 꺼진 것은 다르다.
        let quiet = read_push_head(
            r#"{"head":{"ref":"x","repo":{"name":"t","owner":{"login":"o"},
               "clone_url":"https://h/o/t.git","ssh_url":"git@h:o/t.git"}}}"#,
        )
        .expect("read")
        .expect("a head");
        assert_eq!(quiet.maintainer_can_modify, None);
    }

    /// 지어 주는 원격 이름과, 그 원격을 어느 URL로 더할 것인가.
    #[test]
    fn a_fork_remote_is_named_after_it_and_added_the_way_this_checkout_speaks() {
        assert_eq!(
            sanitize_remote_name("Contributor", "tool"),
            "pr-contributor-tool"
        );
        // ref 이름에 못 쓰는 것은 한 대시로 접히고, 대시가 연달으면 그것도 하나다.
        assert_eq!(sanitize_remote_name("a b/c", "d  e"), "pr-a-b-c-d-e");
        assert_eq!(sanitize_remote_name("--x--", "--y--"), "pr-x-y");
        // 남는 글자가 없으면 이름이 없는 것이 아니라 기본 이름이다.
        assert_eq!(sanitize_remote_name("...", "..."), "pr-head");

        let clone = "https://github.com/o/t.git";
        let ssh = "git@github.com:o/t.git";
        // 이 체크아웃이 https로 말하면 https로 더한다 — ssh 원격을 더해 두면
        // 첫 푸시가 열쇠를 묻는다.
        assert_eq!(
            pick_push_url(Some("https://github.com/a/b.git"), clone, ssh),
            clone
        );
        assert_eq!(pick_push_url(None, clone, ssh), clone);
        for spoken in [
            "git@github.com:a/b.git",
            "ssh://git@github.com/a/b.git",
            // GHES는 443 포트의 ssh를 `ssh.<host>`로 받는다 — 스킴만 보면 https다.
            "https://ssh.github.example.com/a/b.git",
            "https://git@ssh.github.example.com/a/b.git",
        ] {
            assert_eq!(pick_push_url(Some(spoken), clone, ssh), ssh, "{spoken}");
        }
    }

    #[test]
    fn a_refusal_says_which_kind_it_was() {
        assert_eq!(GhError::Missing.reason(), "missing");
        assert_eq!(GhError::Missing.detail(), None);
        let refused = GhError::Refused("gh: not logged in".into());
        assert_eq!(refused.reason(), "refused");
        assert_eq!(refused.detail(), Some("gh: not logged in"));
        // Malformed JSON is a third thing, not a refusal: it means the tool
        // answered and we could not read it.
        let unreadable = read_check_runs("not json").expect_err("garbage parsed");
        assert_eq!(unreadable.reason(), "unreadable");
    }

    #[test]
    fn an_opened_item_carries_its_body_and_every_voice_under_it() {
        let raw = r#"{
          "number": 7, "title": "로그인이 두 번 튕김", "state": "OPEN",
          "author": {"login": "kim"}, "createdAt": "2026-08-01T09:00:00Z",
          "body": "  재현: 로그인 → 뒤로 → 다시 로그인  ",
          "url": "https://github.com/acme/app/issues/7",
          "comments": [
            {"author": {"login": "lee"}, "createdAt": "2026-08-02T10:00:00Z",
             "body": "브랜치 fix/redirect에서 재현됨"},
            {"author": null, "createdAt": null, "body": ""}
          ]
        }"#;
        let detail = read_work_item_detail(raw, ItemKind::Issue).expect("detail");
        assert_eq!(detail.kind, "issue");
        assert_eq!(detail.number, 7);
        assert_eq!(detail.state, "open");
        assert_eq!(detail.author.as_deref(), Some("kim"));
        // The body is what was written — not trimmed, not judged.
        assert_eq!(detail.body, "  재현: 로그인 → 뒤로 → 다시 로그인  ");
        assert_eq!(detail.comments.len(), 2);
        assert_eq!(detail.comments[0].author.as_deref(), Some("lee"));
        assert_eq!(detail.comments[0].body, "브랜치 fix/redirect에서 재현됨");
        // A ghosted author and an emptied comment are still a row each.
        assert_eq!(detail.comments[1].author, None);
        assert_eq!(detail.comments[1].body, "");
        // And a draft PR reads as draft through the same shared precedence.
        let pr = read_work_item_detail(
            r#"{"number":9,"state":"OPEN","isDraft":true,"comments":[]}"#,
            ItemKind::Pr,
        )
        .expect("pr detail");
        assert_eq!(pr.state, "draft");
        assert_eq!(pr.kind, "pr");
    }

    #[test]
    fn a_comment_travels_on_stdin_never_argv() {
        let runner = FakeRunner::with([Ok(CliOutput {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        })]);
        comment_work_item_with(
            &runner,
            Path::new("/w/repo"),
            ItemKind::Issue,
            7,
            "이 줄은 프로세스 목록에 보이면 안 된다",
        )
        .expect("comment");
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].args,
            vec!["issue", "comment", "7", "--body-file", "-"]
        );
        assert_eq!(
            calls[0].stdin.as_deref(),
            Some("이 줄은 프로세스 목록에 보이면 안 된다".as_bytes())
        );
        assert!(
            !calls[0]
                .args
                .iter()
                .any(|arg| arg.contains("보이면 안 된다")),
            "the comment body leaked into argv: {:?}",
            calls[0].args
        );
    }

    /// A login joins the question only when it is one — spaces, quotes, a
    /// leading hyphen are not a person, they are grammar this window may not
    /// smuggle into the one `--search` sentence.
    #[test]
    fn a_login_joins_the_question_only_when_it_is_one() {
        let plan = search_plan(
            "",
            "",
            &WorkItemFilters {
                state: None,
                draft: false,
                author: Some("hana-dev".to_string()),
                assignee: Some("no space".to_string()),
            },
        );
        assert_eq!(plan.query, "author:hana-dev");

        let plan = search_plan(
            "",
            "로그인",
            &WorkItemFilters {
                state: Some("open".to_string()),
                draft: false,
                author: Some("-edge".to_string()),
                assignee: Some("lee".to_string()),
            },
        );
        assert_eq!(plan.query, "로그인 state:open assignee:lee");
    }

    /// The bench read is one page of logins, and rows without one drop out.
    #[test]
    fn the_bench_is_logins_alone() {
        let logins =
            read_logins(r#"[{"login": "hana", "avatar_url": "x"}, {"id": 9}, {"login": "lee"}]"#)
                .expect("readable");
        assert_eq!(logins, vec!["hana".to_string(), "lee".to_string()]);
    }

    fn merged_row(number: u64, updated: &str) -> WorkItem {
        WorkItem {
            kind: "issue",
            number,
            title: format!("항목 {number}"),
            url: String::new(),
            state: "open".to_string(),
            author: None,
            branch: None,
            base: None,
            cross_repo: false,
            updated_at: Some(updated.to_string()),
            assignees: Vec::new(),
            project: None,
        }
    }

    /// One repository that cannot answer does not blank the ones that can,
    /// every surviving row says which project answered it, and the merged
    /// list stands by freshness — numbers order nothing across repositories.
    #[test]
    fn a_pr_detail_carries_githubs_own_verdicts() {
        let raw = r#"{
            "number": 7,
            "title": "t",
            "state": "OPEN",
            "url": "u",
            "comments": [],
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "UNSTABLE",
            "reviewDecision": "REVIEW_REQUIRED"
        }"#;
        let detail = read_work_item_detail(raw, ItemKind::Pr).expect("detail");
        assert_eq!(detail.mergeable.as_deref(), Some("MERGEABLE"));
        assert_eq!(detail.merge_state_status.as_deref(), Some("UNSTABLE"));
        assert_eq!(detail.review_decision.as_deref(), Some("REVIEW_REQUIRED"));
        // The ask itself names the three fields — nothing arrives unasked.
        let asked = work_item_view_args(ItemKind::Pr, 7).join(" ");
        for field in ["mergeable", "mergeStateStatus", "reviewDecision"] {
            assert!(asked.contains(field), "{field} was never asked for");
        }
        assert!(
            !work_item_view_args(ItemKind::Issue, 7)
                .join(" ")
                .contains("mergeable")
        );
    }

    #[test]
    fn a_merge_travels_as_one_flagged_argv() {
        assert_eq!(
            merge_pr_args(42, PrMergeMethod::Squash),
            ["pr", "merge", "42", "--squash"]
        );
        assert_eq!(merge_pr_args(42, PrMergeMethod::Merge)[3], "--merge");
        assert_eq!(merge_pr_args(42, PrMergeMethod::Rebase)[3], "--rebase");
        assert!(PrMergeMethod::parse("delete-branch").is_err());
        assert_eq!(PrMergeMethod::parse("squash"), Ok(PrMergeMethod::Squash));
    }

    #[test]
    fn projects_fold_by_freshness_and_one_refusal_is_not_a_blackout() {
        let folded = merge_project_answers(vec![
            (
                "/home/a".to_string(),
                Ok(vec![merged_row(70, "2026-08-01T00:00:00Z")]),
            ),
            (
                "/home/b".to_string(),
                Err(GhError::Refused("no remote".to_string())),
            ),
            (
                "/home/c".to_string(),
                Ok(vec![merged_row(9, "2026-08-15T00:00:00Z")]),
            ),
        ])
        .expect("two answers are an answer");
        assert_eq!(
            folded
                .iter()
                .map(|row| (row.number, row.project.as_deref()))
                .collect::<Vec<_>>(),
            vec![(9, Some("/home/c")), (70, Some("/home/a"))],
            "the fresher row should stand first regardless of its number"
        );

        let blackout = merge_project_answers(vec![
            (
                "/home/a".to_string(),
                Err(GhError::Refused("first".to_string())),
            ),
            (
                "/home/b".to_string(),
                Err(GhError::Refused("second".to_string())),
            ),
        ]);
        assert_eq!(
            blackout,
            Err(GhError::Refused("first".to_string())),
            "everybody refusing should answer with the first refusal alone"
        );
    }
    /* ---- t-2733: the appended half of a PR detail, reviewers, the stack ---- */

    /// Files with their ± and status, the rollup's checks through the ONE
    /// normaliser, reviewers with where they stand, and review comments
    /// folded into threads — appended after the fields the dialog already
    /// reads, so the shape never forks between an issue and a PR.
    #[test]
    fn a_pr_detail_appends_files_checks_reviewers_and_threads() {
        let files = r#"[
          {"filename":"src/a.rs","additions":3,"deletions":1,"status":"modified"},
          {"filename":"src/b.rs","previous_filename":"src/old.rs","additions":0,"deletions":0,"status":"renamed"}
        ]"#;
        let listed = read_pr_files(files).expect("files");
        assert_eq!(listed.len(), 2);
        assert_eq!(
            (
                listed[0].path.as_str(),
                listed[0].additions,
                listed[0].deletions,
                listed[0].status.as_str()
            ),
            ("src/a.rs", 3, 1, "modified")
        );
        assert_eq!(listed[1].previous_path.as_deref(), Some("src/old.rs"));

        let view = r#"{
          "number": 7, "title": "t", "state": "OPEN", "url": "u", "comments": [],
          "statusCheckRollup": [
            {"__typename":"CheckRun","name":"ci/test","status":"COMPLETED","conclusion":"FAILURE",
             "detailsUrl":"https://github.com/o/r/actions/runs/55/job/1"},
            {"__typename":"CheckRun","name":"ci/lint","status":"IN_PROGRESS","conclusion":null,"detailsUrl":null},
            {"__typename":"StatusContext","context":"deploy/preview","state":"SUCCESS","targetUrl":"https://x"}
          ],
          "reviewRequests": [{"login":"kim"}],
          "latestReviews": [
            {"author":{"login":"lee"},"state":"APPROVED"},
            {"author":{"login":"park"},"state":"CHANGES_REQUESTED"}
          ]
        }"#;
        let detail = read_work_item_detail(view, ItemKind::Pr).expect("detail");
        assert_eq!(detail.checks.state, "failing");
        let names: Vec<&str> = detail
            .checks
            .rows
            .iter()
            .map(|check| check.name.as_str())
            .collect();
        assert_eq!(names, ["ci/test", "ci/lint", "deploy/preview"]);
        assert_eq!(
            detail.checks.rows[0].conclusion,
            Some(CheckConclusion::Failure)
        );
        assert_eq!(detail.checks.rows[0].workflow_run_id, Some(55));
        assert_eq!(
            detail.checks.rows[1].conclusion,
            Some(CheckConclusion::Pending)
        );
        assert_eq!(
            detail.checks.rows[2].conclusion,
            Some(CheckConclusion::Success)
        );
        let who: Vec<(&str, &str)> = detail
            .reviewers
            .iter()
            .map(|one| (one.login.as_str(), one.state.as_str()))
            .collect();
        assert_eq!(
            who,
            [
                ("kim", "requested"),
                ("lee", "approved"),
                ("park", "changes_requested")
            ]
        );
        // An issue appends the same fields, empty — the shape never forks.
        let issue =
            read_work_item_detail(r#"{"number":1,"comments":[]}"#, ItemKind::Issue).expect("issue");
        assert!(issue.files.is_empty() && issue.reviewers.is_empty());
        assert!(issue.conversation.is_empty());
        assert_eq!(issue.checks.state, "none");
        // The ask names the appended fields — nothing arrives unasked.
        let asked = work_item_view_args(ItemKind::Pr, 7).join(" ");
        for field in ["statusCheckRollup", "reviewRequests", "latestReviews"] {
            assert!(asked.contains(field), "{field} was never asked for");
        }

        let comments = r#"[
          {"id":1,"user":{"login":"lee"},"body":"root","created_at":"2026-09-01T00:00:00Z",
           "path":"src/a.rs","line":12},
          {"id":2,"user":{"login":"kim"},"body":"reply","created_at":"2026-09-01T01:00:00Z","in_reply_to_id":1},
          {"id":3,"user":null,"body":"orphan reply","in_reply_to_id":999},
          {"id":4,"user":{"login":"park"},"body":"other root","created_at":"2026-09-02T00:00:00Z"}
        ]"#;
        let threads = read_conversation(comments).expect("threads");
        assert_eq!(threads.len(), 3, "root, promoted orphan, root");
        assert_eq!(threads[0].id, 1);
        assert_eq!(threads[0].replies.len(), 1);
        assert_eq!(threads[0].replies[0].body, "reply");
        assert_eq!(threads[0].path.as_deref(), Some("src/a.rs"));
        assert_eq!(threads[0].line, Some(12));
        // A reply whose root is gone still shows, as a root of its own.
        assert_eq!(threads[1].id, 3);
        assert_eq!(threads[1].author, None);
        assert_eq!(threads[2].id, 4);
    }

    /// The login array goes to GitHub exactly as given — one flag per login,
    /// in order, untrimmed and uncased — and a refusal is `gh`'s first line.
    #[test]
    fn reviewer_logins_travel_unmodified_one_per_flag_and_a_refusal_is_ghs_first_line() {
        let runner = FakeRunner::with([success("")]);
        set_reviewers_with(
            &runner,
            Path::new("/w/repo"),
            7,
            &["Kim-Lee".to_string(), " park ".to_string()],
            false,
        )
        .expect("set");
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].args,
            vec![
                "api",
                "-X",
                "POST",
                "repos/{owner}/{repo}/pulls/7/requested_reviewers",
                "-f",
                "reviewers[]=Kim-Lee",
                "-f",
                "reviewers[]= park ",
            ]
        );
        let runner = FakeRunner::with([success("")]);
        set_reviewers_with(&runner, Path::new("/w/repo"), 7, &["kim".to_string()], true)
            .expect("remove");
        assert_eq!(runner.calls()[0].args[2], "DELETE");
        let runner = FakeRunner::with([failure(
            "",
            "gh: Reviews may only be requested from collaborators. (HTTP 422)\nmore lines",
        )]);
        let refused =
            set_reviewers_with(&runner, Path::new("/w/repo"), 7, &["x".to_string()], false)
                .expect_err("refused");
        assert_eq!(
            refused,
            GhError::Refused(
                "gh: Reviews may only be requested from collaborators. (HTTP 422)".into()
            )
        );
        // Nobody named is refused here, without spending a process.
        let runner = FakeRunner::with(Vec::<Result<CliOutput, CliError>>::new());
        assert!(set_reviewers_with(&runner, Path::new("/w/repo"), 7, &[], false).is_err());
        assert!(runner.calls().is_empty());
    }

    /// The stack is read off the open reviews and ordered by the pure
    /// function — the shell never re-derives a parent here.
    #[test]
    fn a_stack_is_read_off_the_open_reviews() {
        let raw = r#"[
          {"number":10,"title":"a","html_url":"https://g/o/r/pull/10","state":"open","draft":false,
           "head":{"ref":"feat/a"},"base":{"ref":"main"}},
          {"number":11,"title":"b","html_url":"https://g/o/r/pull/11","state":"open","draft":true,
           "head":{"ref":"feat/b"},"base":{"ref":"feat/a"}}
        ]"#;
        let reviews = read_stack_reviews(raw).expect("reviews");
        assert_eq!(reviews.len(), 2);
        assert_eq!(
            (
                reviews[1].number,
                reviews[1].head.as_str(),
                reviews[1].base.as_str(),
                reviews[1].state.as_str()
            ),
            (11, "feat/b", "feat/a", "draft")
        );
        let links: Vec<zerocode_core::checks::StackPr> =
            reviews.iter().map(StackReview::link).collect();
        assert_eq!(
            zerocode_core::checks::stack_order(10, &links).chain,
            [11, 10]
        );
    }
}
