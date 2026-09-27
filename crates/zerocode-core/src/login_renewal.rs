//! How an agent's own CLI is asked to renew a login it keeps (t-10915).
//!
//! A managed account nobody is running keeps the access token it had when
//! its CLI last ran, and that token expires — the CLI refreshes it only
//! when it runs. So an account the window reads but does not run went
//! unreadable a few hours after the person switched away from it: HTTP 401
//! on every read, until somebody pressed "log in again" for a login that
//! was never dead (live report 2026-09-27: four of five Claude accounts).
//!
//! The window never refreshes a login itself — no call to a token endpoint,
//! no refresh token read, no client identity borrowed. It runs the vendor's
//! own CLI once, with the account's store named, and lets the CLI do what
//! it does at every start: refresh its own login, in its own store. What
//! differs per provider is the words that make the CLI start that far and
//! no further, so each provider is a ROW of [`LOGIN_RENEWALS`] and the
//! runner is one ([`crate::account`] names the Claude store; the shell's
//! `accounts::renew_login` runs a row).
//!
//! A row must not spend the account: the words are ones the CLI answers
//! locally, so no model is asked and no token of the plan is used. Measured
//! for each row below, and re-measured with `tools/login-renewal-probe`
//! (a fake API that counts `/v1/messages`) whenever a CLI moves.

/// One provider's renewal: its CLI, the words after the program's name,
/// and what it is told on stdin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginRenewal {
    /// The catalog id whose program runs it.
    pub agent: &'static str,
    pub argv: &'static [&'static str],
    pub stdin: &'static str,
}

/// Claude Code, headless ([`crate::type_value::CLAUDE_HEADLESS`]), told
/// `/cost` — a slash command the CLI answers itself, from the plan's own
/// usage, after it has started and refreshed its login.
///
/// Measured 2026-09-27 on Claude Code 2.1.283:
///
/// * `claude auth status` is NOT a renewal: in a store that already knows
///   its account it answers from disk and touches nothing (182 ms, the
///   token still expired, no network at all).
/// * this row renewed an account whose token had expired 608 minutes
///   earlier to one good for 480 more, in 2,718 ms — and against a fake
///   API it sent no `/v1/messages` (`num_turns` 0, every token count 0),
///   while the control, the same words with `hello` on stdin, did.
///
/// `--disable-slash-commands` is deliberately absent: with it, `/cost`
/// would be a prompt, and a prompt is a model call on the person's plan.
pub const CLAUDE_RENEWAL: LoginRenewal = LoginRenewal {
    agent: "claude",
    argv: crate::type_value::CLAUDE_HEADLESS,
    stdin: "/cost",
};

/// Every provider this window can ask to renew a login it does not run.
pub const LOGIN_RENEWALS: &[LoginRenewal] = &[CLAUDE_RENEWAL];

/// The row for `agent`, when it has one.
#[must_use]
pub fn for_agent(agent: &str) -> Option<&'static LoginRenewal> {
    LOGIN_RENEWALS.iter().find(|row| row.agent == agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Claude row is the headless run and nothing that could make it a
    /// question: a slash command on stdin, no model named, no system
    /// prompt, and the slash commands left on — so `/cost` is answered by
    /// the CLI, never by a model.
    #[test]
    fn the_claude_renewal_is_a_slash_command_the_cli_answers_itself() {
        let row = for_agent("claude").expect("the Claude row");
        assert_eq!(row.argv, crate::type_value::CLAUDE_HEADLESS);
        assert!(row.stdin.starts_with('/') && !row.stdin.contains(char::is_whitespace));
        for asks_a_model in [
            "--disable-slash-commands",
            "--model",
            "--system-prompt",
            "--resume",
            "--continue",
        ] {
            assert!(
                !row.argv.contains(&asks_a_model),
                "{asks_a_model} rides the renewal"
            );
        }
        for shut in ["--no-session-persistence", "--strict-mcp-config"] {
            assert!(row.argv.contains(&shut), "{shut} is missing");
        }
        assert_eq!(for_agent("no-such-agent"), None);
    }
}
