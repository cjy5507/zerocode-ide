//! The readiness snapshot's runtime (t-3996): this process's one table of
//! what the witnesses last said per agent, the probe that asks them, and the
//! three doors that read it.
//!
//! The doors and their freshness:
//!
//! - the window's picker (`list_agents`) asks at DISPLAY freshness, so a
//!   settings row can say 「로그인 필요」 before anybody launches;
//! - the worker summons asks at LAUNCH freshness, right before the door that
//!   refuses a briefing bound for a login screen, and the refusal names what
//!   the snapshot saw;
//! - the ledger's `agent-list` PEEKS — it holds the ledger while it plans and
//!   may not spawn a witness — and a stale row sends one sweep over every
//!   stale row on a thread of its own, for the next reader.
//!
//! The probe decides nothing by an agent's name. It asks the capability
//! table which witnesses the row allows ([`AuthProbeKind`]) and runs the
//! checks the account modules already own: the identity file the CLI writes
//! (`accounts::signed_in`, `codex_accounts::identity_in`), the scoped
//! keychain item this window seeds (`accounts::keychain_says`, and only for
//! the home this window owns), and `gh auth status` (`gh::integration_status`).
//! It never logs anybody in, never materializes a home, never runs the CLI's
//! own status command — a look, on a timer, is what rewrote a person's
//! credentials once already (`accounts::reading_env_for`'s history).
//!
//! Every number is `zerocode_core::readiness::Limits` with the settings
//! overlay laid over it; this file spells none of its own.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use zerocode_core::account::{OVERRIDING_AUTH_VARS, Provider};
use zerocode_core::agent::{AgentPresence, resolve_on_path};
use zerocode_core::capabilities::AuthProbeKind;
use zerocode_core::codex_account::{API_KEY_VAR, AuthKind};
use zerocode_core::readiness::{
    AgentReadinessSnapshot, AuthState, BinaryState, Limits, OVERLAY_PREFIX, Purpose, ReadinessCache,
};

use crate::*;

/// The process's one table: where the probe reads, how fresh a purpose
/// needs the rows, the rows, and which rows a background refresh is
/// already asking about.
///
/// A value rather than four statics so a test can stand one of its own
/// beside the process's — the doors below are thin over [`Table`].
pub(crate) struct Table {
    root: Mutex<Option<PathBuf>>,
    limits: Mutex<Option<Limits>>,
    cache: OnceLock<Mutex<ReadinessCache>>,
    refreshing: OnceLock<Mutex<HashSet<String>>>,
}

impl Table {
    pub(crate) const fn new() -> Self {
        Self {
            root: Mutex::new(None),
            limits: Mutex::new(None),
            cache: OnceLock::new(),
            refreshing: OnceLock::new(),
        }
    }

    fn cache(&self) -> &Mutex<ReadinessCache> {
        self.cache
            .get_or_init(|| Mutex::new(ReadinessCache::default()))
    }

    fn limits(&self) -> Limits {
        self.limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or_default()
    }

    fn root(&self) -> Option<PathBuf> {
        self.root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The fresh row, or the probe's answer, remembered. The probe runs
    /// OUTSIDE the table's lock: a keychain read or a `gh` call under it
    /// would make every peek wait on the slowest witness.
    pub(crate) fn ensure_with(
        &self,
        agent: &str,
        purpose: Purpose,
        now_ms: i64,
        probe: impl FnOnce(i64) -> AgentReadinessSnapshot,
    ) -> AgentReadinessSnapshot {
        let limits = self.limits();
        if let Some(held) = self
            .cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fresh(agent, purpose, now_ms, &limits)
        {
            return held.clone();
        }
        let found = probe(now_ms).trimmed(&limits);
        self.cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(found.clone());
        found
    }

    /// What is held, whatever its age; `None` when nobody observed.
    pub(crate) fn peek(&self, agent: &str) -> Option<AgentReadinessSnapshot> {
        self.cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .peek(agent)
            .cloned()
    }

    /// Whether a peek at `agent` should send a refresh: nothing held, or a
    /// row too old for a look.
    fn wants_refresh(&self, agent: &str, now_ms: i64) -> bool {
        let limits = self.limits();
        self.cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fresh(agent, Purpose::Display, now_ms, &limits)
            .is_none()
    }

    fn invalidate_where(&self, stale: impl FnMut(&str) -> bool) -> usize {
        self.cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate_where(stale)
    }

    /// Claim a name for one background refresh; `false` when one is already
    /// out under it.
    fn claim_refresh(&self, agent: &str) -> bool {
        self.refreshing
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(agent.to_string())
    }

    fn release_refresh(&self, agent: &str) {
        self.refreshing
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(agent);
    }
}

static TABLE: Table = Table::new();

/// Where the probe reads from — the config root that holds the account
/// stores. Named once at boot; until then every door answers unknown
/// rather than reading a home nobody pointed it at.
pub(crate) fn configure_root(config_root: &Path) {
    *TABLE
        .root
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config_root.to_path_buf());
}

/// The numbers, with the saved overlay laid over them. Boot and every
/// settings commit hand them over, the way the hang watchdog's are.
pub(crate) fn configure(limits: Limits) {
    *TABLE
        .limits
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(limits);
}

/// The overlay's road from the settings document: `{"launch_fresh_ms":
/// 10000}` under the document's `readiness` object is
/// `readiness.launch_fresh_ms` to the table.
pub(crate) fn limits_of(overlay: &serde_json::Value) -> Limits {
    let map: BTreeMap<String, u64> = u64_overlay(OVERLAY_PREFIX, overlay);
    Limits::default().overlaid(&map)
}

/* ---- the doors ---------------------------------------------------------- */

/// The snapshot, fresh for this purpose — the summons's door. A stale row
/// is probed again here and now.
pub(crate) fn ensure(agent: &str, purpose: Purpose) -> AgentReadinessSnapshot {
    TABLE.ensure_with(agent, purpose, epoch_ms_now(), |now_ms| {
        probe(agent, None, now_ms)
    })
}

/// [`ensure`] for a caller that already holds the agent's presence row —
/// the picker, which lists every agent and would otherwise pay a PATH walk
/// per row.
pub(crate) fn ensure_seen(presence: &AgentPresence, purpose: Purpose) -> AgentReadinessSnapshot {
    TABLE.ensure_with(presence.id, purpose, epoch_ms_now(), |now_ms| {
        probe(presence.id, Some(presence), now_ms)
    })
}

/// Forget one agent's row: the door that read it proved it wrong, or
/// repaired what it described.
pub(crate) fn invalidate(agent: &str) {
    TABLE.invalidate_where(|held| held == agent);
}

/// What is held for `agent`, whatever its age — the ledger's door. The
/// verb may not spawn a witness while it holds the ledger, so a row too old
/// for a look sends ONE sweep over every stale row on a thread of its own,
/// and this answer is what was there; the next reader gets the fresh ones.
pub(crate) fn observe(agent: &str) -> Option<AgentReadinessSnapshot> {
    let now_ms = epoch_ms_now();
    if TABLE.wants_refresh(agent, now_ms) {
        sweep_in_background();
    }
    TABLE.peek(agent)
}

/// The one claim a sweep holds while it runs, so `agent-list`'s thirty-six
/// peeks send one thread and one PATH walk, not thirty-six of each.
const SWEEP: &str = "*";

fn sweep_in_background() {
    if TABLE.root().is_none() || !TABLE.claim_refresh(SWEEP) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("readiness-sweep".into())
        .spawn(|| {
            let now_ms = epoch_ms_now();
            for presence in detected_agents(false) {
                if TABLE.wants_refresh(presence.id, now_ms) {
                    let _ = ensure_seen(&presence, Purpose::Display);
                }
            }
            TABLE.release_refresh(SWEEP);
        });
    if spawned.is_err() {
        TABLE.release_refresh(SWEEP);
    }
}

/// A login on `provider` moved — an account switch, a login, a logout, a
/// removal — so every agent that reads that provider's login is observed
/// again by its next reader.
pub(crate) fn login_moved(provider: Provider) {
    TABLE.invalidate_where(|agent| {
        zerocode_core::agent_capabilities(agent)
            .and_then(|caps| caps.auth_probe)
            .is_some_and(|probe| probe.providers.contains(&provider))
    });
}

/// The GitHub CLI's login moved: the agents whose witness it is.
pub(crate) fn gh_login_moved() {
    TABLE.invalidate_where(|agent| {
        zerocode_core::agent_capabilities(agent)
            .and_then(|caps| caps.auth_probe)
            .is_some_and(|probe| probe.probes.contains(&AuthProbeKind::GhAuthStatus))
    });
}

/// The PATH was re-read: every binary verdict may have changed.
pub(crate) fn installs_changed() {
    TABLE.invalidate_where(|_| true);
}

/// The refusal a summons gives, with what the snapshot saw beside the
/// sentence the door already had — so a coordinator reading "cannot start"
/// also reads which witness said so and how old that word is.
pub(crate) fn refusal_with_evidence(reason: &str, snapshot: &AgentReadinessSnapshot) -> String {
    let age_s = snapshot.age_ms(epoch_ms_now()) / 1_000;
    format!(
        "{reason} — readiness: auth={}, {}, observed {age_s}s ago",
        snapshot.auth.as_str(),
        snapshot.evidence
    )
}

/* ---- the probe ---------------------------------------------------------- */

/// The facts a probe reads from the machine rather than from the account
/// stores, named so a test can hand it a machine of its own.
pub(crate) struct Witnesses<'a> {
    /// The machine's own Codex home (`~/.codex`), or `None` where there is
    /// none to read.
    pub(crate) codex_system_home: Option<PathBuf>,
    /// Whether the GitHub CLI has a connected account; `None` when it could
    /// not be asked.
    pub(crate) gh_connected: &'a dyn Fn() -> Option<bool>,
    /// Whether this process's environment sets the named variable to
    /// something — the auth overrides a launch inherits.
    pub(crate) env_set: &'a dyn Fn(&str) -> bool,
}

/// One observation of `agent`, against the process's own machine. The
/// presence row is handed in where the caller already has one (the picker
/// lists every agent) and looked up where it does not (a summons).
fn probe(agent: &str, presence: Option<&AgentPresence>, now_ms: i64) -> AgentReadinessSnapshot {
    let Some(root) = TABLE.root() else {
        return AgentReadinessSnapshot::unknown(agent, "readiness: no config root yet", now_ms);
    };
    let looked_up = presence
        .is_none()
        .then(|| {
            detected_agents(false)
                .into_iter()
                .find(|row| row.id == agent)
        })
        .flatten();
    let path = shell_path::launch_path();
    let gh_connected = || {
        gh::integration_status(&root, None, false)
            .ok()
            .map(|status| status.connected)
    };
    let env_set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let witnesses = Witnesses {
        codex_system_home: codex_accounts::system_home(),
        gh_connected: &gh_connected,
        env_set: &env_set,
    };
    probe_in(
        &root,
        agent,
        presence.or(looked_up.as_ref()),
        path.as_deref(),
        &witnesses,
        now_ms,
    )
}

/// One observation of `agent`, against the machine the caller names: the
/// binary off `path`, the login off the row's witnesses.
pub(crate) fn probe_in(
    config_root: &Path,
    agent: &str,
    presence: Option<&AgentPresence>,
    path: Option<&OsStr>,
    witnesses: &Witnesses<'_>,
    now_ms: i64,
) -> AgentReadinessSnapshot {
    let Some(caps) = zerocode_core::agent_capabilities(agent) else {
        return AgentReadinessSnapshot::unknown(agent, "no agent by that name", now_ms);
    };
    let binary = presence
        .and_then(|row| row.found_as.as_deref())
        .and_then(|name| resolve_on_path(path, name))
        .map_or(BinaryState::Missing, |file| BinaryState::Present {
            path: file.display().to_string(),
        });
    let mut verdicts = Vec::new();
    let mut said = Vec::new();
    if let Some(probe) = caps.auth_probe {
        for provider in probe.providers {
            let (state, words) = match provider {
                Provider::Anthropic => anthropic_witness(config_root, probe.probes, witnesses),
                Provider::OpenAi => openai_witness(config_root, probe.probes, witnesses),
            };
            verdicts.push(state);
            said.push(words);
        }
        if probe.probes.contains(&AuthProbeKind::GhAuthStatus) {
            let (state, words) = gh_witness((witnesses.gh_connected)());
            verdicts.push(state);
            said.push(words);
        }
    }
    AgentReadinessSnapshot {
        agent: agent.to_string(),
        binary,
        auth: fold(&verdicts),
        observed_at_ms: now_ms,
        evidence: if said.is_empty() {
            "no local witness for this agent".to_string()
        } else {
            said.join("; ")
        },
    }
}

/// One login is enough to start; every witness saying "none" is the only
/// "signed out"; anything else is not known.
fn fold(verdicts: &[AuthState]) -> AuthState {
    if verdicts.contains(&AuthState::Authorized) {
        AuthState::Authorized
    } else if !verdicts.is_empty()
        && verdicts
            .iter()
            .all(|verdict| *verdict == AuthState::Unauthorized)
    {
        AuthState::Unauthorized
    } else {
        AuthState::Unknown
    }
}

const fn names(named: bool) -> &'static str {
    if named {
        "names an account"
    } else {
        "names nobody"
    }
}

/// The Anthropic login as a launch of this agent would read it.
///
/// A managed home is judged by its identity file, and — where the row
/// allows — by the keychain item this window seeded there; the launch
/// blanks every overriding variable for such a home, so the environment is
/// no witness. The machine's own login is the person's: its file may be
/// read, its keychain item may not, and an override in the environment is
/// a login the launch will inherit.
fn anthropic_witness(
    config_root: &Path,
    probes: &[AuthProbeKind],
    witnesses: &Witnesses<'_>,
) -> (AuthState, String) {
    if !probes.contains(&AuthProbeKind::IdentityFile) {
        return (
            AuthState::Unknown,
            "anthropic: no witness on the row".into(),
        );
    }
    match accounts::login_look(config_root) {
        accounts::LoginLook::Managed(home) => {
            let named = accounts::signed_in(&home);
            let mut state = if named {
                AuthState::Authorized
            } else {
                AuthState::Unauthorized
            };
            let mut words = format!("anthropic: identity file {} (runtime home)", names(named));
            if probes.contains(&AuthProbeKind::Keychain) {
                let keychain = accounts::keychain_says(&home);
                words.push_str("; keychain: ");
                words.push_str(keychain.word());
                if keychain.is_signed_out_document() {
                    state = AuthState::Unauthorized;
                }
            }
            (state, words)
        }
        accounts::LoginLook::System(home) => {
            if let Some(name) = OVERRIDING_AUTH_VARS
                .iter()
                .find(|name| (witnesses.env_set)(name))
            {
                return (
                    AuthState::Authorized,
                    format!("anthropic: {name} in the environment (system login)"),
                );
            }
            match home {
                Some(home) => {
                    let named = accounts::signed_in(&home);
                    (
                        if named {
                            AuthState::Authorized
                        } else {
                            AuthState::Unauthorized
                        },
                        format!("anthropic: identity file {} (system login)", names(named)),
                    )
                }
                None => (
                    AuthState::Unknown,
                    "anthropic: system login, no home to read".into(),
                ),
            }
        }
    }
}

/// The OpenAI login as a launch of this agent would read it: the key in the
/// environment, which the launch leaves standing, or the `auth.json` of the
/// home the launch would point at.
fn openai_witness(
    config_root: &Path,
    probes: &[AuthProbeKind],
    witnesses: &Witnesses<'_>,
) -> (AuthState, String) {
    if (witnesses.env_set)(API_KEY_VAR) {
        return (
            AuthState::Authorized,
            format!("openai: {API_KEY_VAR} in the environment"),
        );
    }
    if !probes.contains(&AuthProbeKind::IdentityFile) {
        return (AuthState::Unknown, "openai: no witness on the row".into());
    }
    match codex_accounts::active_home_in(config_root, witnesses.codex_system_home.clone()) {
        Some((home, whose)) => {
            let kind = codex_accounts::identity_in(&home).0;
            let word = match kind {
                AuthKind::Oauth => "oauth",
                AuthKind::ApiKey => "api-key",
                AuthKind::None => "none",
            };
            let whose = match whose {
                codex_accounts::CodexHomeKind::Account => "account home",
                codex_accounts::CodexHomeKind::System => "~/.codex",
            };
            (
                if matches!(kind, AuthKind::None) {
                    AuthState::Unauthorized
                } else {
                    AuthState::Authorized
                },
                format!("openai: auth.json {word} ({whose})"),
            )
        }
        None => (AuthState::Unknown, "openai: no home to read".into()),
    }
}

/// `gh auth status` is a witness FOR a login and never against one: the
/// agent that rides it also holds a login of its own, which gh cannot see.
fn gh_witness(connected: Option<bool>) -> (AuthState, String) {
    match connected {
        Some(true) => (
            AuthState::Authorized,
            "gh auth status: a connected account".into(),
        ),
        Some(false) => (
            AuthState::Unknown,
            "gh auth status: no connected account (the CLI may hold its own login)".into(),
        ),
        None => (AuthState::Unknown, "gh auth status: no answer".into()),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use zerocode_core::agent::agent_presence;

    /// A machine with these commands on its PATH.
    fn machine(commands: &[&str]) -> (tempfile::TempDir, OsString) {
        let dir = tempfile::tempdir().expect("temp dir");
        for command in commands {
            let file = dir.path().join(command);
            std::fs::write(&file, b"#!/bin/sh\n").expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
        let path = dir.path().as_os_str().to_os_string();
        (dir, path)
    }

    fn quiet_witnesses<'a>(
        codex_system_home: Option<PathBuf>,
        gh: &'a dyn Fn() -> Option<bool>,
        env: &'a dyn Fn(&str) -> bool,
    ) -> Witnesses<'a> {
        Witnesses {
            codex_system_home,
            gh_connected: gh,
            env_set: env,
        }
    }

    fn nobody_answers() -> Option<bool> {
        None
    }

    fn nothing_set(_: &str) -> bool {
        false
    }

    fn observe(
        config_root: &Path,
        agent: &str,
        path: Option<&OsStr>,
        witnesses: &Witnesses<'_>,
    ) -> AgentReadinessSnapshot {
        let rows = agent_presence(path, "macos");
        let presence = rows.iter().find(|row| row.id == agent);
        probe_in(config_root, agent, presence, path, witnesses, 1_000)
    }

    fn name_an_account(home: &Path) {
        std::fs::create_dir_all(home).expect("home");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"worker@example.test"}}"#,
        )
        .expect("identity file");
    }

    /// Claude's three states off the runtime home's identity file: named
    /// is authorized, empty is unauthorized, and the evidence says which
    /// witness spoke — with the keychain word beside it where the row
    /// allows the keychain, and never a secret.
    #[test]
    fn claude_is_judged_by_the_runtime_homes_identity_file_and_says_so() {
        let config = tempfile::tempdir().expect("config root");
        let (_machine, path) = machine(&["claude"]);
        let witnesses = quiet_witnesses(None, &nobody_answers, &nothing_set);

        let empty = observe(config.path(), "claude", Some(&path), &witnesses);
        assert_eq!(empty.auth, AuthState::Unauthorized, "{empty:?}");
        assert!(
            empty
                .evidence
                .starts_with("anthropic: identity file names nobody (runtime home); keychain: "),
            "{}",
            empty.evidence
        );
        assert!(
            matches!(&empty.binary, BinaryState::Present { path } if path.ends_with("/claude")),
            "{:?}",
            empty.binary
        );

        name_an_account(&accounts::runtime_home(config.path()));
        let named = observe(config.path(), "claude", Some(&path), &witnesses);
        assert_eq!(named.auth, AuthState::Authorized, "{named:?}");
        assert!(
            named
                .evidence
                .starts_with("anthropic: identity file names an account (runtime home)"),
            "{}",
            named.evidence
        );
        assert!(
            !named.evidence.contains("worker@example.test"),
            "the evidence carried the identity itself"
        );
        assert_eq!(named.observed_at_ms, 1_000);
    }

    /// The machine's own login: its file may be read, its keychain never,
    /// and an override in the environment is a login the launch inherits.
    #[test]
    fn the_system_login_is_read_from_its_file_or_its_environment_and_never_its_keychain() {
        let config = tempfile::tempdir().expect("config root");
        accounts::use_system_default(config.path()).expect("system default");
        let (_machine, path) = machine(&["claude"]);

        // Under test the person's own home is deliberately unreadable
        // (`accounts::external_runtime_home`), so nobody is accused.
        let quiet = quiet_witnesses(None, &nobody_answers, &nothing_set);
        let unread = observe(config.path(), "claude", Some(&path), &quiet);
        assert_eq!(unread.auth, AuthState::Unknown, "{unread:?}");
        assert_eq!(unread.evidence, "anthropic: system login, no home to read");
        assert!(!unread.evidence.contains("keychain"));

        let keyed = |name: &str| name == "ANTHROPIC_API_KEY";
        let witnesses = quiet_witnesses(None, &nobody_answers, &keyed);
        let by_env = observe(config.path(), "claude", Some(&path), &witnesses);
        assert_eq!(by_env.auth, AuthState::Authorized, "{by_env:?}");
        assert_eq!(
            by_env.evidence,
            "anthropic: ANTHROPIC_API_KEY in the environment (system login)"
        );
    }

    /// Codex's three states off `auth.json` in the home a launch would
    /// point at — an API key counts, `none` is signed out, and the
    /// environment's key is a login the launch keeps.
    #[test]
    fn codex_is_judged_by_auth_json_in_the_home_a_launch_would_read() {
        let config = tempfile::tempdir().expect("config root");
        let system = tempfile::tempdir().expect("~/.codex");
        let (_machine, path) = machine(&["codex"]);

        let quiet = quiet_witnesses(
            Some(system.path().to_path_buf()),
            &nobody_answers,
            &nothing_set,
        );
        let signed_out = observe(config.path(), "codex", Some(&path), &quiet);
        assert_eq!(signed_out.auth, AuthState::Unauthorized, "{signed_out:?}");
        assert_eq!(signed_out.evidence, "openai: auth.json none (~/.codex)");

        std::fs::write(
            system.path().join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-fixture","tokens":null}"#,
        )
        .expect("auth.json");
        let keyed = observe(config.path(), "codex", Some(&path), &quiet);
        assert_eq!(keyed.auth, AuthState::Authorized, "{keyed:?}");
        assert_eq!(keyed.evidence, "openai: auth.json api-key (~/.codex)");
        assert!(!keyed.evidence.contains("sk-fixture"));

        // A managed account whose home holds a login is the launch's home.
        let account_home = config.path().join("codex-accounts").join("c-1");
        std::fs::create_dir_all(&account_home).expect("account home");
        std::fs::write(
            account_home.join("auth.json"),
            r#"{"tokens":{"id_token":"","access_token":"","refresh_token":"","account_id":"acct"},"OPENAI_API_KEY":"sk-managed"}"#,
        )
        .expect("managed auth.json");
        codex_accounts::write_store(
            config.path(),
            &codex_accounts::CodexAccountStore {
                accounts: vec![zerocode_core::codex_account::CodexAccount {
                    id: "c-1".into(),
                    email: None,
                    workspace_label: None,
                    provider_account_id: None,
                    home_dir: account_home.display().to_string(),
                    added_at: 0,
                }],
                selection: zerocode_core::codex_account::CodexSelection {
                    active: Some("c-1".into()),
                },
            },
        )
        .expect("store");
        let managed = observe(config.path(), "codex", Some(&path), &quiet);
        assert_eq!(managed.auth, AuthState::Authorized, "{managed:?}");
        assert_eq!(managed.evidence, "openai: auth.json api-key (account home)");

        let no_home = quiet_witnesses(None, &nobody_answers, &nothing_set);
        let fresh_config = tempfile::tempdir().expect("config root");
        let unread = observe(fresh_config.path(), "codex", Some(&path), &no_home);
        assert_eq!(unread.auth, AuthState::Unknown, "{unread:?}");

        let keyed_env = |name: &str| name == API_KEY_VAR;
        let by_env = quiet_witnesses(None, &nobody_answers, &keyed_env);
        let inherited = observe(fresh_config.path(), "codex", Some(&path), &by_env);
        assert_eq!(inherited.auth, AuthState::Authorized, "{inherited:?}");
        assert_eq!(
            inherited.evidence,
            "openai: OPENAI_API_KEY in the environment"
        );
    }

    /// `gh auth status` is a witness for a login and never against one, an
    /// agent with no witness on its row is unknown, and a missing binary is
    /// said as such.
    #[test]
    fn gh_speaks_for_a_login_never_against_and_an_agent_without_witnesses_is_unknown() {
        let config = tempfile::tempdir().expect("config root");
        let (_machine, path) = machine(&["copilot"]);
        let connected = || Some(true);
        let witnesses = quiet_witnesses(None, &connected, &nothing_set);
        let signed_in = observe(config.path(), "copilot", Some(&path), &witnesses);
        assert_eq!(signed_in.auth, AuthState::Authorized, "{signed_in:?}");
        assert_eq!(signed_in.evidence, "gh auth status: a connected account");

        let nobody = || Some(false);
        let witnesses = quiet_witnesses(None, &nobody, &nothing_set);
        let unsure = observe(config.path(), "copilot", Some(&path), &witnesses);
        assert_eq!(
            unsure.auth,
            AuthState::Unknown,
            "a gh 'no' accused the CLI's own login"
        );

        let quiet = quiet_witnesses(None, &nobody_answers, &nothing_set);
        let silent = observe(config.path(), "copilot", Some(&path), &quiet);
        assert_eq!(silent.auth, AuthState::Unknown);
        assert_eq!(silent.evidence, "gh auth status: no answer");

        let witnessless = observe(config.path(), "kimi", Some(&path), &quiet);
        assert_eq!(witnessless.auth, AuthState::Unknown);
        assert_eq!(witnessless.evidence, "no local witness for this agent");
        assert_eq!(witnessless.binary, BinaryState::Missing);

        let nobody_here = observe(config.path(), "not-an-agent", Some(&path), &quiet);
        assert_eq!(nobody_here.evidence, "no agent by that name");
    }

    /// An agent on two providers starts on either: one login is enough,
    /// and only every witness saying "none" is signed out.
    #[test]
    fn an_agent_on_two_providers_is_authorized_by_either_and_signed_out_by_both() {
        let config = tempfile::tempdir().expect("config root");
        let system = tempfile::tempdir().expect("~/.codex");
        let (_machine, path) = machine(&["zo"]);
        let quiet = quiet_witnesses(
            Some(system.path().to_path_buf()),
            &nobody_answers,
            &nothing_set,
        );

        let both_out = observe(config.path(), "zo", Some(&path), &quiet);
        assert_eq!(both_out.auth, AuthState::Unauthorized, "{both_out:?}");
        assert!(
            both_out.evidence.contains("anthropic: ") && both_out.evidence.contains("; openai: ")
        );

        name_an_account(&accounts::runtime_home(config.path()));
        let one_in = observe(config.path(), "zo", Some(&path), &quiet);
        assert_eq!(one_in.auth, AuthState::Authorized, "{one_in:?}");

        assert_eq!(fold(&[]), AuthState::Unknown);
        assert_eq!(
            fold(&[AuthState::Unauthorized, AuthState::Unknown]),
            AuthState::Unknown
        );
    }

    /// The table's doors: a display-fresh row is served to a look and
    /// probed again for a launch, a moved login is probed again by the next
    /// reader, and a probe never runs under the table's lock.
    #[test]
    fn a_look_reads_the_row_a_launch_reprobes_it_and_a_moved_login_forgets_it() {
        let table = Table::new();
        let now = 10 * 60 * 1_000;
        let seen = |auth: AuthState, at: i64| AgentReadinessSnapshot {
            agent: "claude".into(),
            binary: BinaryState::Missing,
            auth,
            observed_at_ms: at,
            evidence: "fixture".into(),
        };
        let mut probes = 0;
        let first = table.ensure_with("claude", Purpose::Display, now - 120_000, |at| {
            probes += 1;
            seen(AuthState::Authorized, at)
        });
        assert_eq!((probes, first.auth), (1, AuthState::Authorized));
        let looked = table.ensure_with("claude", Purpose::Display, now, |at| {
            probes += 1;
            seen(AuthState::Unauthorized, at)
        });
        assert_eq!(
            (probes, looked.auth),
            (1, AuthState::Authorized),
            "a look re-probed a fresh row"
        );
        assert!(!table.wants_refresh("claude", now));
        let launched = table.ensure_with("claude", Purpose::Launch, now, |at| {
            probes += 1;
            seen(AuthState::Unauthorized, at)
        });
        assert_eq!(
            (probes, launched.auth),
            (2, AuthState::Unauthorized),
            "a launch read a two-minute-old row"
        );
        // A login moved on the provider claude reads: the row goes, the
        // next look probes; a provider claude does not read leaves it.
        assert_eq!(
            table.invalidate_where(|agent| {
                zerocode_core::agent_capabilities(agent)
                    .and_then(|caps| caps.auth_probe)
                    .is_some_and(|probe| probe.providers.contains(&Provider::OpenAi))
            }),
            0
        );
        assert!(table.peek("claude").is_some());
        assert_eq!(
            table.invalidate_where(|agent| {
                zerocode_core::agent_capabilities(agent)
                    .and_then(|caps| caps.auth_probe)
                    .is_some_and(|probe| probe.providers.contains(&Provider::Anthropic))
            }),
            1
        );
        assert!(table.peek("claude").is_none());
        assert!(table.wants_refresh("claude", now));
        // The probe runs outside the lock: probing may peek.
        let probed_while_peeking = table.ensure_with("claude", Purpose::Display, now, |at| {
            assert!(
                table.peek("claude").is_none(),
                "the table was locked under the probe"
            );
            seen(AuthState::Authorized, at)
        });
        assert_eq!(probed_while_peeking.auth, AuthState::Authorized);
        // One background sweep at a time.
        assert!(table.claim_refresh(SWEEP));
        assert!(!table.claim_refresh(SWEEP));
        table.release_refresh(SWEEP);
        assert!(table.claim_refresh(SWEEP));
    }

    /// The numbers come from the table with the document's overlay laid
    /// over it, and a table nobody configured answers the defaults.
    #[test]
    fn the_limits_are_the_tables_with_the_documents_overlay() {
        assert_eq!(limits_of(&serde_json::json!({})), Limits::default());
        let moved =
            limits_of(&serde_json::json!({"launch_fresh_ms": 5000, "display_fresh_ms": "no"}));
        assert_eq!(moved.launch_fresh_ms, 5_000);
        assert_eq!(moved.display_fresh_ms, Limits::default().display_fresh_ms);
        assert_eq!(Table::new().limits(), Limits::default());
    }

    /// The refusal carries the state, the evidence and the age beside the
    /// sentence the door already had.
    #[test]
    fn a_refusal_names_the_snapshots_evidence() {
        let snapshot = AgentReadinessSnapshot {
            agent: "claude".into(),
            binary: BinaryState::Missing,
            auth: AuthState::Unauthorized,
            observed_at_ms: epoch_ms_now() - 2_500,
            evidence: "anthropic: identity file names nobody (runtime home)".into(),
        };
        let said = refusal_with_evidence("an unattended worker cannot start: X", &snapshot);
        assert!(
            said.starts_with(
                "an unattended worker cannot start: X — readiness: auth=unauthorized, \
                 anthropic: identity file names nobody (runtime home), observed 2s ago"
            ),
            "{said}"
        );
    }

    /// MEASURED, not assumed: a cache hit is a lock and a clone, a miss is
    /// the file witnesses. The numbers are printed for the launch-path
    /// budget (`--nocapture`), and the hit has to be the cheaper road.
    #[test]
    fn a_cache_hit_costs_less_than_a_probe() {
        let config = tempfile::tempdir().expect("config root");
        name_an_account(&accounts::runtime_home(config.path()));
        let (_machine, path) = machine(&["claude"]);
        let witnesses = quiet_witnesses(None, &nobody_answers, &nothing_set);
        let rows = agent_presence(Some(&path), "macos");
        let presence = rows.iter().find(|row| row.id == "claude");
        let table = Table::new();
        let rounds = 20;

        let started = std::time::Instant::now();
        for round in 0..rounds {
            // A fresh row every round: a launch-fresh window in the past.
            table.invalidate_where(|_| true);
            let _ = table.ensure_with("claude", Purpose::Launch, round, |at| {
                probe_in(
                    config.path(),
                    "claude",
                    presence,
                    Some(&path),
                    &witnesses,
                    at,
                )
            });
        }
        let miss_us = started.elapsed().as_micros() / rounds as u128;

        let started = std::time::Instant::now();
        for _ in 0..rounds {
            let _ = table.ensure_with("claude", Purpose::Launch, rounds, |at| {
                panic!("a hit probed at {at}")
            });
        }
        let hit_us = started.elapsed().as_micros() / rounds as u128;
        eprintln!(
            "readiness ensure: cache hit {hit_us} µs, probe miss {miss_us} µs (claude, file witnesses)"
        );
        assert!(hit_us <= miss_us, "hit {hit_us} µs, miss {miss_us} µs");
    }
}
