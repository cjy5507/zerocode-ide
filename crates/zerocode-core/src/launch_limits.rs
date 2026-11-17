//! The tool limits a conversation's launch put on it, and the words a resume
//! gives them back with (t-112277).
//!
//! A window reopens a pane's conversation with the agent's own resume — for
//! zo, `zo --resume <id>` behind the launch words the window always gives.
//! Nothing on that line knew the conversation had been opened on a short tool
//! list, or barred from starting other agents: a review opened on four tools,
//! a helper pane cut from its parent's brief, a `zo --allowed-tools …` typed
//! into a pane's shell each came back after a restart on every tool — and a
//! turn the restart cut is told to go on. The window never sees most of those
//! launches (a brief is a file, a typed line is the shell's), so the agent
//! says what it was launched under in its `SessionStart` ([`PAYLOAD_KEY`]),
//! the window keeps that beside the conversation, and a resume gives it back
//! in the agent's own spelling ([`LimitFlags`], off the agent's row).
//!
//! Only plain words ride a command line ([`LaunchLimits::is_plain`]). Limits
//! said in any other shape are [`Reported::Unplain`]: the conversation is not
//! reopened wider than it was opened, it is not reopened at all.

use serde::{Deserialize, Serialize};

/// How an agent's command line takes a conversation's limits back — measured
/// per agent and carried on its row (`Harness::resume_limits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LimitFlags {
    /// The flag the tool list rides behind, its names joined by commas.
    pub allowed_tools: &'static str,
    /// The flag, taking no value, that bars the tools starting other agents.
    pub no_spawn: &'static str,
}

/// Zo's spelling (`zo-ide` `ide/args.rs`): `--allowed-tools` reads one
/// comma-separated list, `--no-spawn` stands alone.
pub const ZO_LIMIT_FLAGS: LimitFlags = LimitFlags {
    allowed_tools: "--allowed-tools",
    no_spawn: "--no-spawn",
};

/// The `SessionStart` payload key the limits travel under (zo's reporter).
pub const PAYLOAD_KEY: &str = "launch_limits";

/// The longest tool name a limit carries.
const NAME_MAX: usize = 256;

/// The most names a list carries.
const NAMES_MAX: usize = 512;

/// The limits one launch put on a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchLimits {
    /// The only tools the conversation may use, by the names its agent keeps
    /// them under; `None` when the launch named no list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Whether the tools that start other agents were taken away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_spawn: bool,
}

impl LaunchLimits {
    /// Whether these limits can stand on a command line as they are: at least
    /// one of them, a list of at least one name, and every name a plain tool
    /// name — ASCII letters, digits and `_ - . :`, never opening with `-`
    /// (the agent's parser would read a flag) and never holding a comma (one
    /// name would be read as two).
    #[must_use]
    pub fn is_plain(&self) -> bool {
        let named = self.allowed_tools.as_ref().is_none_or(|names| {
            !names.is_empty()
                && names.len() <= NAMES_MAX
                && names.iter().all(|name| plain_name(name))
        });
        named && (self.allowed_tools.is_some() || self.no_spawn)
    }

    /// The words that give these limits back, in `flags`' spelling — `None`
    /// for limits that are not plain, which no command line carries.
    #[must_use]
    pub fn words(&self, flags: LimitFlags) -> Option<Vec<String>> {
        if !self.is_plain() {
            return None;
        }
        let mut words = Vec::new();
        if let Some(names) = &self.allowed_tools {
            words.push(flags.allowed_tools.to_string());
            words.push(names.join(","));
        }
        if self.no_spawn {
            words.push(flags.no_spawn.to_string());
        }
        Some(words)
    }
}

/// A name a command line carries as it is ([`LaunchLimits::is_plain`]).
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

/// What one `SessionStart` says about its conversation's limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reported {
    /// None: the launch put none on, or the agent says no such thing.
    None,
    /// These, plain ([`LaunchLimits::is_plain`]).
    Limits(LaunchLimits),
    /// Limits were said, in a shape no command line carries.
    Unplain,
}

/// What `payload` — a `SessionStart`'s — says about the limits of the
/// conversation it opened. An absent or null key says none; anything said
/// that is not plain limits is [`Reported::Unplain`], never none.
#[must_use]
pub fn reported_in(payload: &serde_json::Value) -> Reported {
    match payload.get(PAYLOAD_KEY) {
        None | Some(serde_json::Value::Null) => Reported::None,
        Some(said) => match serde_json::from_value::<LaunchLimits>(said.clone()) {
            Ok(limits) if limits.is_plain() => Reported::Limits(limits),
            _ => Reported::Unplain,
        },
    }
}

/// What one hook event says about the limits of the conversation it names:
/// `None` unless it is the lead's own `SessionStart`. No other event says what
/// a launch put on, and a helper's start (child-attributed) speaks for the
/// helper, not for the conversation its pane holds.
#[must_use]
pub fn reported_by(
    event_name: &str,
    payload: &crate::payload::HookPayload<'_>,
) -> Option<Reported> {
    if crate::hook::normalized_event(event_name) != "sessionstart"
        || crate::hook::helper_attributed_parsed(payload)
    {
        return None;
    }
    payload.tree().map(reported_in)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(said: &[&str]) -> Option<Vec<String>> {
        Some(said.iter().map(|name| (*name).to_string()).collect())
    }

    /// A list and the spawn bar come back as zo reads them: one
    /// comma-joined list behind `--allowed-tools`, `--no-spawn` alone.
    #[test]
    fn limits_are_given_back_in_the_agents_spelling() {
        let both = LaunchLimits {
            allowed_tools: names(&["read_file", "grep_search", "mcp__ctx7__query"]),
            no_spawn: true,
        };
        assert_eq!(
            both.words(ZO_LIMIT_FLAGS),
            Some(vec![
                "--allowed-tools".to_string(),
                "read_file,grep_search,mcp__ctx7__query".to_string(),
                "--no-spawn".to_string(),
            ])
        );
        let barred = LaunchLimits {
            allowed_tools: None,
            no_spawn: true,
        };
        assert_eq!(
            barred.words(ZO_LIMIT_FLAGS),
            Some(vec!["--no-spawn".to_string()])
        );
    }

    /// No word that would read as a flag, split in two or say nothing rides a
    /// command line: such limits have no words at all.
    #[test]
    fn limits_that_are_not_plain_have_no_words() {
        for list in [
            names(&[]),
            names(&["--resume"]),
            names(&["read_file,bash"]),
            names(&["read file"]),
            names(&[""]),
            names(&["read_file\n"]),
        ] {
            let limits = LaunchLimits {
                allowed_tools: list.clone(),
                no_spawn: false,
            };
            assert!(!limits.is_plain(), "{list:?}");
            assert_eq!(limits.words(ZO_LIMIT_FLAGS), None, "{list:?}");
        }
        let nothing = LaunchLimits {
            allowed_tools: None,
            no_spawn: false,
        };
        assert!(!nothing.is_plain(), "no limit is nothing to give back");
    }

    /// A `SessionStart` that says nothing says none; one that says plain
    /// limits says them; anything else said is unplain — never none, which
    /// would reopen the conversation wider than it was opened.
    #[test]
    fn a_session_start_says_its_limits_none_or_unplain() {
        let base = json!({"source": "startup", "session_id": "session-1-0"});
        assert_eq!(reported_in(&base), Reported::None);
        assert_eq!(
            reported_in(&json!({"session_id": "session-1-0", "launch_limits": null})),
            Reported::None
        );
        assert_eq!(
            reported_in(
                &json!({"launch_limits": {"allowed_tools": ["read_file"], "no_spawn": true}})
            ),
            Reported::Limits(LaunchLimits {
                allowed_tools: names(&["read_file"]),
                no_spawn: true,
            })
        );
        for unplain in [
            json!({"launch_limits": {}}),
            json!({"launch_limits": {"allowed_tools": []}}),
            json!({"launch_limits": {"allowed_tools": ["-x"]}}),
            json!({"launch_limits": {"allowed_tools": "read_file"}}),
            json!({"launch_limits": {"allowed_tools": ["read_file"], "deny": ["bash"]}}),
            json!({"launch_limits": "read_file"}),
        ] {
            assert_eq!(reported_in(&unplain), Reported::Unplain, "{unplain}");
        }
    }

    /// Only the lead's own `SessionStart` speaks for its conversation's
    /// limits: another event, or a helper's start, says nothing — not even
    /// none, which would take a conversation's record away.
    #[test]
    fn only_the_leads_session_start_speaks_for_the_limits() {
        let said = r#"{"session_id":"session-1-0","launch_limits":{"no_spawn":true}}"#;
        let barred = Some(Reported::Limits(LaunchLimits {
            allowed_tools: None,
            no_spawn: true,
        }));
        let payload = crate::payload::HookPayload::of(said);
        assert_eq!(reported_by("SessionStart", &payload), barred);
        assert_eq!(reported_by("session_start", &payload), barred);
        let none = r#"{"session_id":"session-1-0","source":"resume"}"#;
        assert_eq!(
            reported_by("SessionStart", &crate::payload::HookPayload::of(none)),
            Some(Reported::None)
        );
        for other in ["Stop", "UserPromptSubmit", "SubagentStart"] {
            assert_eq!(reported_by(other, &payload), None, "{other}");
        }
        let helper =
            r#"{"session_id":"session-1-0","agent_id":"a-1","launch_limits":{"no_spawn":true}}"#;
        assert_eq!(
            reported_by("SessionStart", &crate::payload::HookPayload::of(helper)),
            None
        );
        assert_eq!(
            reported_by("SessionStart", &crate::payload::HookPayload::of("not json")),
            None
        );
    }
}
