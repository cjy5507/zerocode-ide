//! The environment that stops Git asking a question nobody can answer.
//!
//! An agent working in a pane nobody is watching cannot answer a credential
//! prompt, and Git's default is to ask: an expired token turns `git fetch`
//! from an error into a process that sits there forever, and the pane looks
//! busy rather than broken. Orca guards exactly the unattended launches and
//! leaves a person's own terminal alone — "unattended agents must fail
//! instead of looping on OS credential prompts; user terminals keep normal
//! Git behavior" (`main/ipc/pty.ts:1783`).
//!
//! What the guard is made of is Orca's too
//! (`shared/git-credential-prompt-env.ts:81`): the scalars that turn the
//! terminal prompt and Git Credential Manager's own window off, empty askpass
//! programs so no GUI helper is reached for, and two config entries that
//! disable the interactive FALLBACK of a credential helper while leaving the
//! helper itself alone — a cached credential must keep working.
//!
//! The config half travels through Git's indexed protocol
//! (`GIT_CONFIG_COUNT` plus `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>`),
//! which is all-or-nothing: Git rejects the whole set when the count and the
//! pairs disagree. So the count is read and VALIDATED before anything is
//! appended, and when it does not add up the scalars go alone rather than
//! overwriting indices that may hold somebody else's entries.

/// The two config entries the guard adds, in Orca's order.
const GUARDS: [(&str, &str); 2] = [
    ("credential.interactive", "false"),
    ("credential.guiPrompt", "false"),
];

/// Where a new indexed entry may safely start, or [`None`] when the protocol
/// already in the environment does not add up.
///
/// Valid means: no count and no indexed keys at all (start at zero), or a
/// count that parses, has exactly two keys per entry, has both halves of every
/// entry, and has no index at or beyond the count.
#[must_use]
pub fn append_at(look: &impl Fn(&str) -> Option<String>) -> Option<usize> {
    let raw = look("GIT_CONFIG_COUNT");
    // Nothing declared: only a clean slate may be appended to.
    let Some(raw) = raw else {
        return (0..=MAX_SCAN)
            .all(|at| {
                look(&format!("GIT_CONFIG_KEY_{at}")).is_none()
                    && look(&format!("GIT_CONFIG_VALUE_{at}")).is_none()
            })
            .then_some(0);
    };
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return None;
    }
    let count: usize = raw.parse().ok()?;
    if count > MAX_SCAN {
        return None;
    }
    for at in 0..count {
        if look(&format!("GIT_CONFIG_KEY_{at}")).is_none()
            || look(&format!("GIT_CONFIG_VALUE_{at}")).is_none()
        {
            return None;
        }
    }
    // A pair beyond the count is the ambiguous case Git would read past.
    for at in count..=MAX_SCAN {
        if look(&format!("GIT_CONFIG_KEY_{at}")).is_some()
            || look(&format!("GIT_CONFIG_VALUE_{at}")).is_some()
        {
            return None;
        }
    }
    Some(count)
}

/// How far the scan for stray indices goes.
///
/// Git's protocol has no ceiling, and a scan has to stop somewhere. Orca reads
/// the whole environment instead; a bounded scan is the same answer for every
/// real environment and cannot be made to walk forever by a hostile one.
const MAX_SCAN: usize = 64;

/// The variables an unattended pane is given, on top of what it inherited.
///
/// `look` answers what the child would otherwise see for a name — the parent
/// environment, in production. Askpass programs are only emptied when nothing
/// set them: a person who pointed Git at their own helper keeps it.
#[must_use]
pub fn guard_env(look: &impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut env = vec![
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        // Git Credential Manager can ignore both of the above and open its own
        // window, so it is told separately.
        ("GCM_INTERACTIVE".to_string(), "never".to_string()),
    ];
    for name in ["GIT_ASKPASS", "SSH_ASKPASS"] {
        if look(name).is_none() {
            env.push((name.to_string(), String::new()));
        }
    }
    if let Some(base) = append_at(look) {
        for (at, (key, value)) in GUARDS.iter().enumerate() {
            env.push((format!("GIT_CONFIG_KEY_{}", base + at), (*key).to_string()));
            env.push((
                format!("GIT_CONFIG_VALUE_{}", base + at),
                (*value).to_string(),
            ));
        }
        env.push((
            "GIT_CONFIG_COUNT".to_string(),
            (base + GUARDS.len()).to_string(),
        ));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn looker(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let held: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |name: &str| held.get(name).cloned()
    }

    /// A clean environment gets both entries, at zero, with the count that
    /// matches them.
    #[test]
    fn a_clean_environment_is_appended_to_from_zero() {
        let env = guard_env(&looker(&[]));
        let held: HashMap<&str, &str> = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(held.get("GIT_TERMINAL_PROMPT"), Some(&"0"));
        assert_eq!(held.get("GCM_INTERACTIVE"), Some(&"never"));
        assert_eq!(held.get("GIT_ASKPASS"), Some(&""));
        assert_eq!(held.get("SSH_ASKPASS"), Some(&""));
        assert_eq!(
            held.get("GIT_CONFIG_KEY_0"),
            Some(&"credential.interactive")
        );
        assert_eq!(held.get("GIT_CONFIG_KEY_1"), Some(&"credential.guiPrompt"));
        assert_eq!(held.get("GIT_CONFIG_VALUE_0"), Some(&"false"));
        assert_eq!(held.get("GIT_CONFIG_COUNT"), Some(&"2"));
    }

    /// Somebody else's askpass is theirs to keep — the guard turns the
    /// interactive fallback off, it does not take a helper away.
    #[test]
    fn an_askpass_that_was_already_set_is_left_alone() {
        let env = guard_env(&looker(&[("GIT_ASKPASS", "/usr/local/bin/mine")]));
        assert!(
            !env.iter().any(|(key, _)| key == "GIT_ASKPASS"),
            "the guard replaced a caller's own askpass: {env:?}"
        );
        assert!(
            env.iter()
                .any(|(key, value)| key == "SSH_ASKPASS" && value.is_empty())
        );
    }

    /// An environment that already carries entries is appended AFTER them.
    #[test]
    fn existing_entries_keep_their_indices() {
        let env = guard_env(&looker(&[
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "http.proxy"),
            ("GIT_CONFIG_VALUE_0", "http://proxy.example:8080"),
        ]));
        let held: HashMap<&str, &str> = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            held.get("GIT_CONFIG_KEY_1"),
            Some(&"credential.interactive")
        );
        assert_eq!(held.get("GIT_CONFIG_KEY_2"), Some(&"credential.guiPrompt"));
        assert_eq!(held.get("GIT_CONFIG_COUNT"), Some(&"3"));
        assert!(
            !held.contains_key("GIT_CONFIG_KEY_0"),
            "the guard overwrote an entry that was already there: {env:?}"
        );
    }

    /// And a protocol that does not add up gets the scalars only. Overwriting
    /// indices in that state is how a caller's own config disappears.
    #[test]
    fn an_ambiguous_protocol_is_left_untouched() {
        for broken in [
            vec![("GIT_CONFIG_COUNT", "2"), ("GIT_CONFIG_KEY_0", "a")],
            vec![("GIT_CONFIG_KEY_0", "a"), ("GIT_CONFIG_VALUE_0", "b")],
            vec![
                ("GIT_CONFIG_COUNT", "1"),
                ("GIT_CONFIG_KEY_0", "a"),
                ("GIT_CONFIG_VALUE_0", "b"),
                ("GIT_CONFIG_KEY_4", "stray"),
            ],
            vec![("GIT_CONFIG_COUNT", "07")],
            vec![("GIT_CONFIG_COUNT", "many")],
        ] {
            let env = guard_env(&looker(&broken));
            assert!(
                !env.iter().any(|(key, _)| key.starts_with("GIT_CONFIG_")),
                "an ambiguous protocol was appended to anyway ({broken:?}): {env:?}"
            );
            assert!(
                env.iter()
                    .any(|(key, value)| key == "GIT_TERMINAL_PROMPT" && value == "0"),
                "the scalars stopped travelling when the config half could not"
            );
        }
    }
}
