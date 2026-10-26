//! `${NAME}` and `${NAME:-default}` in an MCP server's settings, resolved when
//! the server is launched.
//!
//! The settings keep the reference, never the value: the parser, a round-trip
//! through `zo mcp add` and the server's signature all see `${NAME}`. Only the
//! transport a launch opens holds the value, and only for that launch. The
//! roads, in order: zo's own environment, then the window's keychain item for
//! the name — for a server the person's own settings file declares, never one
//! a repository's file names — then the `:-default` text. A reference none of
//! them answers keeps its server from starting at all: the server is never
//! handed the text `${NAME}` as if it were a token.
//!
//! **A value is never said.** What this module reports names the reference,
//! not what it resolved to.

use std::collections::BTreeMap;
use std::fmt;

use crate::config::ConfigSource;
use crate::mcp_client::{McpClientBootstrap, McpClientTransport};

/// Where one name's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceSource {
    Environment,
    Keychain,
    Default,
}

impl ReferenceSource {
    /// The word a JSON answer names this road by.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Environment => "env",
            Self::Keychain => "keychain",
            Self::Default => "default",
        }
    }
}

/// The roads a reference's value may come from. A test hands one of its own,
/// so no test reads the process environment's keys or the person's keychain.
pub trait ReferenceKeys {
    /// zo's own environment, when it holds a non-empty value.
    fn environment(&self, name: &str) -> Option<String>;
    /// The window's keychain item for `name`.
    fn keychain(&self, name: &str) -> api::ReferenceKey;
}

/// The roads a launch takes: this process's environment and the keychain road
/// the host installed (`api::find_reference_keys_in_keychain`) — none
/// installed, and the keychain is never read.
pub struct LaunchReferenceKeys;

impl ReferenceKeys for LaunchReferenceKeys {
    fn environment(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }

    fn keychain(&self, name: &str) -> api::ReferenceKey {
        api::reference_key_in_window_keychain(name)
    }
}

/// Why a name has no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceMiss {
    /// Not `[A-Za-z_][A-Za-z0-9_]*`.
    NotAName,
    /// Neither the environment nor (where it may be read) the keychain holds it.
    Absent,
    /// The keychain was asked and did not answer.
    KeychainUnreadable,
}

impl ReferenceMiss {
    /// One sentence for a person, naming no value.
    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::NotAName => "the name is not a reference name (letters, digits and _, not starting with a digit).",
            Self::Absent => "neither zo's environment nor the window's keychain holds a value under this name.",
            Self::KeychainUnreadable => "the window's keychain did not answer when it was asked for this name.",
        }
    }
}

/// A reference no road answered. Says the reference, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedReference {
    pub name: String,
}

impl fmt::Display for UnresolvedReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unresolved reference ${{{}}}", self.name)
    }
}

impl std::error::Error for UnresolvedReference {}

/// Whether `name` can stand inside `${…}`.
#[must_use]
pub fn is_reference_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first == b'_' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

/// The value of one name and the road it came from: the environment, then —
/// when `keychain` allows it — the window's keychain. No default here; that is
/// the reference's own text ([`resolve_references`]).
///
/// # Errors
///
/// Why no road answered.
pub fn resolve_reference_name(
    name: &str,
    keys: &dyn ReferenceKeys,
    keychain: bool,
) -> Result<(String, ReferenceSource), ReferenceMiss> {
    if !is_reference_name(name) {
        return Err(ReferenceMiss::NotAName);
    }
    if let Some(value) = keys.environment(name) {
        return Ok((value, ReferenceSource::Environment));
    }
    if !keychain {
        return Err(ReferenceMiss::Absent);
    }
    match keys.keychain(name) {
        api::ReferenceKey::Found(value) if !value.is_empty() => {
            Ok((value, ReferenceSource::Keychain))
        }
        api::ReferenceKey::Found(_) | api::ReferenceKey::Absent => Err(ReferenceMiss::Absent),
        api::ReferenceKey::Unreadable => Err(ReferenceMiss::KeychainUnreadable),
    }
}

/// `text` with every `${NAME}` and `${NAME:-default}` replaced. A `$` not
/// followed by `{`, and a `${` that does not open a well-formed reference, stay
/// as written.
///
/// # Errors
///
/// The first reference without a value and without a default.
pub fn resolve_references(
    text: &str,
    keys: &dyn ReferenceKeys,
    keychain: bool,
) -> Result<String, UnresolvedReference> {
    if !text.contains("${") {
        return Ok(text.to_string());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some((name, default, consumed)) = reference_at(after) else {
            out.push_str("${");
            rest = after;
            continue;
        };
        match (resolve_reference_name(name, keys, keychain), default) {
            (Ok((value, _)), _) => out.push_str(&value),
            (Err(_), Some(default)) => out.push_str(default),
            (Err(_), None) => {
                return Err(UnresolvedReference {
                    name: name.to_string(),
                });
            }
        }
        rest = &after[consumed..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The reference that opens `after` (the text just past `${`): its name, its
/// default, and how many bytes it takes up to and including the `}`.
fn reference_at(after: &str) -> Option<(&str, Option<&str>, usize)> {
    let name_len = after
        .bytes()
        .position(|byte| byte != b'_' && !byte.is_ascii_alphanumeric())
        .unwrap_or(after.len());
    let name = &after[..name_len];
    if !is_reference_name(name) {
        return None;
    }
    let tail = &after[name_len..];
    if tail.starts_with('}') {
        return Some((name, None, name_len + 1));
    }
    let default = tail.strip_prefix(":-")?;
    let close = default.find('}')?;
    Some((name, Some(&default[..close]), name_len + 2 + close + 1))
}

/// The bootstrap a launch opens: a stdio server's `env` values and `args`, and
/// a remote server's `headers` values, resolved. The keychain is read only for
/// a server the person's own settings file declared.
///
/// # Errors
///
/// The first reference without a value; the server is not to be started.
pub fn resolve_launch_references(
    bootstrap: &McpClientBootstrap,
    keys: &dyn ReferenceKeys,
) -> Result<McpClientBootstrap, UnresolvedReference> {
    let keychain = bootstrap.scope == ConfigSource::User;
    let mut resolved = bootstrap.clone();
    match &mut resolved.transport {
        McpClientTransport::Stdio(stdio) => {
            stdio.env = resolve_values(&stdio.env, keys, keychain)?;
            stdio.args = stdio
                .args
                .iter()
                .map(|arg| resolve_references(arg, keys, keychain))
                .collect::<Result<_, _>>()?;
        }
        McpClientTransport::Sse(remote)
        | McpClientTransport::Http(remote)
        | McpClientTransport::WebSocket(remote) => {
            remote.headers = resolve_values(&remote.headers, keys, keychain)?;
        }
        McpClientTransport::Sdk(_) | McpClientTransport::ManagedProxy(_) => {}
    }
    Ok(resolved)
}

fn resolve_values(
    values: &BTreeMap<String, String>,
    keys: &dyn ReferenceKeys,
    keychain: bool,
) -> Result<BTreeMap<String, String>, UnresolvedReference> {
    values
        .iter()
        .map(|(key, value)| Ok((key.clone(), resolve_references(value, keys, keychain)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use crate::config::{
        ConfigSource, McpRemoteServerConfig, McpServerConfig, McpStdioServerConfig,
        ScopedMcpServerConfig,
    };
    use crate::mcp_client::{McpClientBootstrap, McpClientTransport};

    use super::{
        resolve_launch_references, resolve_reference_name, resolve_references, ReferenceKeys,
        ReferenceMiss, ReferenceSource,
    };

    /// Values no real key has, so a leak is a string a test can look for.
    const ENV_VALUE: &str = "fixture-env-value-5521";
    const KEYCHAIN_VALUE: &str = "fixture-keychain-value-8834";

    /// An environment and a keychain of the test's own, which remembers every
    /// name the keychain was asked for.
    #[derive(Default)]
    struct Keys {
        environment: BTreeMap<String, String>,
        keychain: BTreeMap<String, String>,
        unreadable: bool,
        asked: RefCell<Vec<String>>,
    }

    impl Keys {
        fn with_env(mut self, name: &str, value: &str) -> Self {
            self.environment.insert(name.to_string(), value.to_string());
            self
        }

        fn with_keychain(mut self, name: &str, value: &str) -> Self {
            self.keychain.insert(name.to_string(), value.to_string());
            self
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }
    }

    impl ReferenceKeys for Keys {
        fn environment(&self, name: &str) -> Option<String> {
            self.environment.get(name).cloned()
        }

        fn keychain(&self, name: &str) -> api::ReferenceKey {
            self.asked.borrow_mut().push(name.to_string());
            if self.unreadable {
                return api::ReferenceKey::Unreadable;
            }
            self.keychain
                .get(name)
                .cloned()
                .map_or(api::ReferenceKey::Absent, api::ReferenceKey::Found)
        }
    }

    fn stdio(scope: ConfigSource, env: &[(&str, &str)], args: &[&str]) -> McpClientBootstrap {
        McpClientBootstrap::from_scoped_config(
            "refs",
            &ScopedMcpServerConfig {
                scope,
                config: McpServerConfig::Stdio(McpStdioServerConfig {
                    command: "npx".to_string(),
                    args: args.iter().map(|arg| (*arg).to_string()).collect(),
                    env: env
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                        .collect(),
                    tool_call_timeout_ms: None,
                }),
            },
        )
    }

    fn stdio_env(bootstrap: &McpClientBootstrap) -> (BTreeMap<String, String>, Vec<String>) {
        match &bootstrap.transport {
            McpClientTransport::Stdio(stdio) => (stdio.env.clone(), stdio.args.clone()),
            _ => panic!("expected a stdio transport"),
        }
    }

    #[test]
    fn a_user_server_takes_the_environment_first_then_the_keychain_then_the_default() {
        let keys = Keys::default()
            .with_env("FROM_ENV", ENV_VALUE)
            .with_env("BOTH", ENV_VALUE)
            .with_keychain("BOTH", KEYCHAIN_VALUE)
            .with_keychain("FROM_KEYCHAIN", KEYCHAIN_VALUE);
        let bootstrap = stdio(
            ConfigSource::User,
            &[
                ("A_TOKEN", "${FROM_ENV}"),
                ("B_TOKEN", "${BOTH}"),
                ("C_TOKEN", "Bearer ${FROM_KEYCHAIN}"),
                ("D_TOKEN", "${NOWHERE:-fallback-text}"),
                ("E_TOKEN", "${FROM_KEYCHAIN:-unused}"),
            ],
            &["--token=${FROM_ENV}", "plain"],
        );

        let resolved = resolve_launch_references(&bootstrap, &keys).expect("every reference resolves");
        let (env, args) = stdio_env(&resolved);
        assert_eq!(env["A_TOKEN"], ENV_VALUE, "environment hit");
        assert_eq!(env["B_TOKEN"], ENV_VALUE, "the environment wins over the keychain");
        assert_eq!(env["C_TOKEN"], format!("Bearer {KEYCHAIN_VALUE}"), "keychain hit inside text");
        assert_eq!(env["D_TOKEN"], "fallback-text", "the default when no road answers");
        assert_eq!(env["E_TOKEN"], KEYCHAIN_VALUE, "a found value wins over the default");
        assert_eq!(args, vec![format!("--token={ENV_VALUE}"), "plain".to_string()]);
        // The settings keep the reference: the bootstrap a session holds, and
        // its signature, are untouched.
        let (kept, _) = stdio_env(&bootstrap);
        assert_eq!(kept["A_TOKEN"], "${FROM_ENV}");
        assert_eq!(resolved.signature, bootstrap.signature);
    }

    #[test]
    fn a_remote_server_resolves_its_header_values() {
        let keys = Keys::default().with_keychain("REMOTE_TOKEN", KEYCHAIN_VALUE);
        let bootstrap = McpClientBootstrap::from_scoped_config(
            "remote",
            &ScopedMcpServerConfig {
                scope: ConfigSource::User,
                config: McpServerConfig::Http(McpRemoteServerConfig {
                    url: "https://vendor.example/mcp".to_string(),
                    headers: BTreeMap::from([(
                        "Authorization".to_string(),
                        "Bearer ${REMOTE_TOKEN}".to_string(),
                    )]),
                    headers_helper: None,
                    oauth: None,
                }),
            },
        );
        let resolved = resolve_launch_references(&bootstrap, &keys).expect("resolves");
        let McpClientTransport::Http(remote) = &resolved.transport else {
            panic!("expected http");
        };
        assert_eq!(remote.headers["Authorization"], format!("Bearer {KEYCHAIN_VALUE}"));
    }

    #[test]
    fn an_unresolved_reference_names_itself_and_no_value() {
        let keys = Keys::default().with_env("PRESENT", ENV_VALUE);
        let bootstrap = stdio(
            ConfigSource::User,
            &[("A_FIRST", "${PRESENT}"), ("B_SECOND", "${MISSING_NAME}")],
            &[],
        );
        let unresolved = resolve_launch_references(&bootstrap, &keys).expect_err("one is missing");
        assert_eq!(unresolved.to_string(), "unresolved reference ${MISSING_NAME}");
        let said = format!("{unresolved} {unresolved:?}");
        assert!(!said.contains(ENV_VALUE), "no value in what is said: {said}");
    }

    #[test]
    fn a_project_or_local_server_never_reads_the_keychain() {
        for scope in [ConfigSource::Project, ConfigSource::Local] {
            let keys = Keys::default().with_keychain("REPO_WANTS_THIS", KEYCHAIN_VALUE);
            let bootstrap = stdio(scope, &[("TOKEN", "${REPO_WANTS_THIS}")], &[]);
            let unresolved = resolve_launch_references(&bootstrap, &keys)
                .expect_err("the keychain is not a road for a repository's server");
            assert_eq!(unresolved.to_string(), "unresolved reference ${REPO_WANTS_THIS}");
            assert!(keys.asked().is_empty(), "{scope:?}: keychain asked for {:?}", keys.asked());

            // The environment and the default still apply there.
            let keys = Keys::default()
                .with_env("REPO_ENV", ENV_VALUE)
                .with_keychain("REPO_DEFAULTED", KEYCHAIN_VALUE);
            let bootstrap = stdio(
                scope,
                &[("A", "${REPO_ENV}"), ("B", "${REPO_DEFAULTED:-plain}")],
                &[],
            );
            let resolved = resolve_launch_references(&bootstrap, &keys).expect("env and default");
            let (env, _) = stdio_env(&resolved);
            assert_eq!(env["A"], ENV_VALUE);
            assert_eq!(env["B"], "plain");
            assert!(keys.asked().is_empty(), "{scope:?}: keychain asked for {:?}", keys.asked());
        }
    }

    #[test]
    fn text_that_is_not_a_reference_stays_as_written() {
        let keys = Keys::default().with_env("HOME_LIKE", ENV_VALUE);
        for text in [
            "$HOME_LIKE",
            "costs $5",
            "${",
            "${HOME_LIKE",
            "${1DIGIT}",
            "${with space}",
            "${HOME_LIKE:default}",
            "{{name}}",
        ] {
            assert_eq!(resolve_references(text, &keys, true).as_deref(), Ok(text), "{text}");
        }
        assert_eq!(
            resolve_references("a ${ and ${HOME_LIKE} b", &keys, true).as_deref(),
            Ok(format!("a ${{ and {ENV_VALUE} b").as_str())
        );
        assert_eq!(resolve_references("${EMPTY_DEFAULT:-}", &keys, true).as_deref(), Ok(""));
    }

    #[test]
    fn one_name_says_which_road_answered() {
        let keys = Keys::default()
            .with_env("ON_ENV", ENV_VALUE)
            .with_keychain("ON_KEYCHAIN", KEYCHAIN_VALUE);
        assert_eq!(
            resolve_reference_name("ON_ENV", &keys, true),
            Ok((ENV_VALUE.to_string(), ReferenceSource::Environment))
        );
        assert_eq!(
            resolve_reference_name("ON_KEYCHAIN", &keys, true),
            Ok((KEYCHAIN_VALUE.to_string(), ReferenceSource::Keychain))
        );
        assert_eq!(resolve_reference_name("ON_KEYCHAIN", &keys, false), Err(ReferenceMiss::Absent));
        assert_eq!(resolve_reference_name("NOWHERE", &keys, true), Err(ReferenceMiss::Absent));
        assert_eq!(resolve_reference_name("not-a-name", &keys, true), Err(ReferenceMiss::NotAName));
        let unreadable = Keys {
            unreadable: true,
            ..Keys::default()
        };
        assert_eq!(
            resolve_reference_name("LOCKED", &unreadable, true),
            Err(ReferenceMiss::KeychainUnreadable)
        );
    }
}
