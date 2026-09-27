//! Asking a Claude Code CLI to renew the login it keeps (t-11045).
//!
//! zo never spends a Claude Code login's refresh token: the token is the
//! CLI's, and every pane of the same account renews it too — two spenders of
//! one rotating token is how one of them loses its login. When the login zo
//! reads has expired, the api layer asks this road ([`api::ClaudeLoginRenewal`]);
//! this road runs the store's own CLI once, headless, with the window's renewal
//! row ([`zerocode_core::login_renewal::CLAUDE_RENEWAL`]: a slash command the
//! CLI answers itself, so no model is asked and no plan token is spent), and
//! the api layer reads the store again. The CLI takes its own refresh lock and
//! reads its store again under it, which is what makes this ask safe beside
//! every pane of the account.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::ide::reporter;

/// How long one renewal may run before it is abandoned and its whole process
/// group ended. A renewal took 2,718 ms on Claude Code 2.1.283 (t-10915); the
/// wall is the window's own for one CLI start (`LOGIN_PROBE_DEADLINE`).
const RENEWAL_WALL: Duration = Duration::from_secs(20);

/// The names that tie a process to a pane. A renewal is no pane's, so none of
/// them rides into it: nothing it does can be heard as this pane's turn (the
/// window's `run_once` takes the same names out).
const PANE_COORDINATES: &[&str] = &[
    reporter::ENV_PORT,
    reporter::ENV_TOKEN,
    reporter::ENV_HOOK_ENDPOINT,
    reporter::ENV_PANE_KEY,
    reporter::ENV_TAB_ID,
    reporter::ENV_LAUNCH_TOKEN,
    reporter::ENV_WORKTREE_ID,
];

/// Install this road as the api layer's one renewer. Called once, first thing
/// in `main`; a library user or a test installs its own, or none.
pub fn install() {
    api::install_claude_login_renewer(Some(Arc::new(renew)));
}

fn renew(renewal: &api::ClaudeLoginRenewal) -> api::RenewalRun {
    // A hermetic run reads no credential store outside itself, and starts no
    // CLI that would.
    if api::managed_account::external_credentials_disabled() {
        return api::RenewalRun::NotRun("this run is hermetic".to_string());
    }
    let row = zerocode_core::login_renewal::CLAUDE_RENEWAL;
    let Some(program) = found_program(row.agent) else {
        return api::RenewalRun::NotRun(format!("no `{}` on PATH", row.agent));
    };
    let Some(env) = renewal_env(renewal.folder.as_deref()) else {
        return api::RenewalRun::NotRun(
            "the login's folder is not a path the CLI can be told".to_string(),
        );
    };
    // An empty folder of its own to start in: a CLI reads the folder it starts
    // in (its project's instructions, its notes), and none of that belongs to
    // a renewal.
    let Ok(one_shot) = tempfile::tempdir() else {
        return api::RenewalRun::NotRun("no empty folder to start it in".to_string());
    };
    let mut command = Command::new(program);
    command.args(row.argv).current_dir(one_shot.path());
    for (name, value) in &env {
        if value.is_empty() {
            command.env_remove(name);
        } else {
            command.env(name, value);
        }
    }
    for name in PANE_COORDINATES {
        command.env_remove(name);
    }
    match plugins::run_bounded_process(
        command,
        Some(row.stdin.as_bytes()),
        "Claude Code login renewal",
        RENEWAL_WALL,
    ) {
        // The CLI exits 0 over a refresh that failed, so its status says
        // nothing: the store, read again by the caller, is the answer.
        Ok(_) => api::RenewalRun::Ran,
        Err(plugins::PluginError::TimedOut(_)) => api::RenewalRun::NotRun(format!(
            "it outlived its {}s wall",
            RENEWAL_WALL.as_secs()
        )),
        Err(error) => api::RenewalRun::NotRun(format!("it would not start: {error}")),
    }
}

/// The agent's program on this process's `PATH`, by the catalogue's own names
/// for it — the rule the window's detection uses.
fn found_program(agent: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH");
    zerocode_core::agent::agent_spec(agent)?
        .detect_names()
        .find_map(|name| zerocode_core::agent::resolve_on_path(path.as_deref(), name))
}

/// The environment that names the login's store to its CLI: the folder as both
/// the config and the credential folder — the window's renewal of an account
/// nobody runs does the same (t-10915), so the run refreshes this store under
/// the refresh lock every pane of the account takes, and writes its profile
/// into the account's own folder rather than a runtime home other accounts
/// share — or, for the machine's own login, neither folder, as a terminal's
/// own `claude` runs. Every variable that would make the CLI speak as another
/// credential is taken out, as a launch takes it out. `None` when the folder
/// cannot be said as a string.
fn renewal_env(folder: Option<&Path>) -> Option<Vec<(String, String)>> {
    let headers = std::env::var(zerocode_core::account::CUSTOM_HEADERS_VAR).ok();
    let named = match folder {
        Some(folder) => Some(folder.to_str()?),
        None => None,
    };
    let mut env = zerocode_core::launch_env(headers.as_deref(), named, named);
    if named.is_none() {
        for name in [
            zerocode_core::account::CONFIG_DIR_VAR,
            zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR,
        ] {
            env.push((name.to_string(), String::new()));
        }
    }
    Some(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        env.iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// A scoped login is renewed with its folder as both folders — the store
    /// the panes of that account renew, under the lock they take — and the
    /// machine's own login with neither, whatever this pane was launched with.
    #[test]
    fn a_renewal_names_exactly_the_store_it_renews() {
        let scoped = renewal_env(Some(Path::new("/Users/dev/app/claude-accounts/a-1")))
            .expect("a folder a CLI can be told");
        for name in [
            zerocode_core::account::CONFIG_DIR_VAR,
            zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR,
        ] {
            assert_eq!(
                value(&scoped, name),
                Some("/Users/dev/app/claude-accounts/a-1"),
                "{name}"
            );
        }
        for name in zerocode_core::account::OVERRIDING_AUTH_VARS {
            assert_eq!(value(&scoped, name), Some(""), "{name} rode into the renewal");
        }

        let machine = renewal_env(None).expect("the machine's own login");
        for name in [
            zerocode_core::account::CONFIG_DIR_VAR,
            zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR,
        ] {
            assert_eq!(
                value(&machine, name),
                Some(""),
                "{name} would scope the machine's own login to this pane's folder"
            );
        }
    }

    /// The renewal is the window's row — the headless words and a slash
    /// command on stdin — and nothing that could ask a model.
    #[test]
    fn the_renewal_runs_the_windows_row() {
        let row = zerocode_core::login_renewal::CLAUDE_RENEWAL;
        assert_eq!(row.agent, "claude");
        assert!(row.stdin.starts_with('/'));
        assert!(!row.argv.contains(&"--model"));
    }

    /// A hermetic run starts no CLI.
    #[test]
    fn a_hermetic_run_asks_no_cli() {
        let _lock = crate::support::test_env_lock();
        let prior = std::env::var_os(api::managed_account::EXTERNAL_CREDENTIALS_DISABLED_ENV);
        std::env::set_var(api::managed_account::EXTERNAL_CREDENTIALS_DISABLED_ENV, "1");
        let asked = renew(&api::ClaudeLoginRenewal { folder: None });
        match prior {
            Some(value) => {
                std::env::set_var(api::managed_account::EXTERNAL_CREDENTIALS_DISABLED_ENV, value);
            }
            None => std::env::remove_var(api::managed_account::EXTERNAL_CREDENTIALS_DISABLED_ENV),
        }
        assert!(matches!(asked, api::RenewalRun::NotRun(why) if why.contains("hermetic")));
    }
}
