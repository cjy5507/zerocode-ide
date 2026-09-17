/// Env override naming the agent-manifest store directory.
pub const AGENT_STORE_ENV: &str = "ZO_AGENT_STORE";

/// Directory name of the agent-manifest store under the per-project state base.
pub(super) const AGENT_STORE_DIR_NAME: &str = "agents";

/// Resolve the agent-manifest store directory — the single resolution shared by
/// the spawn path (writer) and every HUD/viewer reader, so they can never drift
/// onto different paths.
///
/// Precedence:
/// 1. `ZO_AGENT_STORE` — explicit override (a non-empty value wins).
/// 2. Otherwise the user-global per-project state dir
///    (`runtime::zo_project_state_dir(cwd)/agents`, honoring
///    `ZO_STATE_DIR`), i.e. `~/.zo/projects/<slug>/state/agents`.
///
/// The default deliberately lives **outside the working tree**, matching the
/// todo store's migration (`runtime::todo_store`). The old location
/// (`<cwd ancestor[2]>/.zo/agents`) wrote agent manifests *into* the cwd, so
/// a read-only or wrong-owner working tree (e.g. a `.zo/agents/` left
/// `root`-owned by a prior `sudo zo`, or a read-only mount) made the spawn
/// path's `create_dir_all`/`write` fail with a bare OS `Permission denied
/// (os error 13)` — surfacing as a failed `Spawned …` even under
/// `danger-full-access`, because this is an OS-uid rejection, not the rule
/// enforcer. Rooting the store in the always-user-writable zo home removes
/// that failure and stops polluting the working tree.
pub fn agent_store_dir() -> Result<std::path::PathBuf, String> {
    if let Some(path) = std::env::var_os(AGENT_STORE_ENV) {
        if !path.to_string_lossy().trim().is_empty() {
            return Ok(std::path::PathBuf::from(path));
        }
    }
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    Ok(runtime::zo_project_state_dir(&cwd).join(AGENT_STORE_DIR_NAME))
}

pub(super) fn make_agent_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("agent-{nanos}")
}

pub(super) fn slugify_agent_name(description: &str) -> String {
    let mut out = description
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').chars().take(32).collect()
}

pub(crate) fn display_agent_label(
    raw_name: Option<&str>,
    description: &str,
    fallback_name: &str,
    subagent_type: &str,
) -> Option<String> {
    let candidate = raw_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let description = description.trim();
            (!description.is_empty() && !description.contains('\n')).then_some(description)
        })?;
    let specialized = !subagent_type.eq_ignore_ascii_case("general-purpose");
    let candidate = if specialized {
        format!("{subagent_type}·{candidate}")
    } else {
        candidate.to_string()
    };
    let label = truncate_agent_label(&candidate, 48);
    if !specialized && label == fallback_name {
        None
    } else {
        Some(label)
    }
}

fn truncate_agent_label(label: &str, max_chars: usize) -> String {
    let mut out = label.chars().take(max_chars).collect::<String>();
    if label.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{agent_store_dir, AGENT_STORE_DIR_NAME, AGENT_STORE_ENV};
    use std::path::Path;

    /// Restore an env var to its prior value (or unset) on drop, so a test that
    /// mutates process-global env never leaks into a sibling test.
    struct EnvGuard {
        key: &'static str,
        prior: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prior = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prior }
        }
        fn unset(key: &'static str) -> Self {
            let prior = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prior }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn explicit_non_empty_env_override_wins() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _store = EnvGuard::set(AGENT_STORE_ENV, "/tmp/explicit-agent-store");
        assert_eq!(
            agent_store_dir().expect("override resolves"),
            Path::new("/tmp/explicit-agent-store"),
        );
    }

    #[test]
    fn blank_env_override_is_ignored() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _store = EnvGuard::set(AGENT_STORE_ENV, "   ");
        let _state = EnvGuard::unset("ZO_STATE_DIR");
        let dir = agent_store_dir().expect("blank override falls through to default");
        // A blank override must not send the store to a literal "   " path; it
        // falls through to the per-project state dir, ending in `/agents`.
        assert!(dir.ends_with(AGENT_STORE_DIR_NAME), "got {}", dir.display());
    }

    #[test]
    fn default_store_lives_outside_the_working_tree() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _store = EnvGuard::unset(AGENT_STORE_ENV);
        // Pin the per-project state base to a known root so we can assert the
        // store lands there (and *not* under the cwd) — the whole point of the
        // EACCES fix: a read-only/wrong-owner working tree never blocks a spawn.
        let root = std::env::temp_dir().join(format!(
            "zo-agent-store-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _state = EnvGuard::set("ZO_STATE_DIR", &root);
        let dir = agent_store_dir().expect("default resolves");
        let cwd = std::env::current_dir().expect("cwd");

        assert!(
            dir.starts_with(&root),
            "store must live under ZO_STATE_DIR root {}, got {}",
            root.display(),
            dir.display(),
        );
        assert!(
            !dir.starts_with(&cwd),
            "store must NOT live inside the working tree {}, got {}",
            cwd.display(),
            dir.display(),
        );
        assert!(dir.ends_with(AGENT_STORE_DIR_NAME));
    }
}
