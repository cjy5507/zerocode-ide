//! Typed model and parser for the `/mcp` (and `zo mcp`) subcommands.
//!
//! Parsing is deliberately separated from execution. [`McpAction::parse`] is the
//! single grammar authority used by both the slash-command parser and the report
//! renderer, so the two surfaces can never drift on which forms are valid.

/// Canonical `/mcp` action tokens shared by the parser and the renderer so
/// the two surfaces serialize the same words.
const ACTION_LIST: &str = "list";
const ACTION_SHOW: &str = "show";
const ACTION_AUTH: &str = "auth";
const ACTION_LOGOUT: &str = "logout";
const ACTION_HELP: &str = "help";

/// A parsed `/mcp` subcommand.
///
/// `auth list` is reserved, so a server literally named `list` cannot be
/// authenticated through `/mcp auth list`; use the runtime `McpAuth` tool for
/// that unusual case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAction {
    /// `/mcp` or `/mcp list` — summarize configured servers.
    List,
    /// `/mcp show <server>` — detail a single server.
    Show(String),
    /// `/mcp auth` or `/mcp auth list` — list OAuth-capable servers and status.
    AuthList,
    /// `/mcp auth <server>` — run the OAuth flow for one server.
    Auth(String),
    /// `/mcp logout <server>` — remove stored MCP OAuth credentials.
    Logout(String),
    /// `/mcp help`.
    Help,
}

/// Why a `/mcp` argument list could not be parsed into an [`McpAction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCommandError {
    /// A subcommand that needs a `<server>` argument was given none.
    MissingServer { action: &'static str },
    /// Extra tokens followed an otherwise complete subcommand.
    UnexpectedArguments { action: &'static str },
    /// The leading token is not a known subcommand.
    UnknownAction { action: String },
}

/// The full `/mcp` usage line, shown for unknown actions and as a fallback.
pub(crate) const MCP_FULL_USAGE: &str =
    "/mcp [list|show <server>|auth [list|<server>]|logout <server>|help]";

impl McpCommandError {
    /// The canonical usage string to show alongside this error, scoped to the
    /// offending subcommand so the hint is as specific as possible.
    #[must_use]
    pub fn usage(&self) -> &'static str {
        match self {
            Self::UnknownAction { .. } => MCP_FULL_USAGE,
            Self::MissingServer { action } | Self::UnexpectedArguments { action } => {
                match *action {
                    ACTION_SHOW => "/mcp show <server>",
                    ACTION_LOGOUT => "/mcp logout <server>",
                    "auth list" => "/mcp auth list",
                    ACTION_AUTH => "/mcp auth <server>",
                    ACTION_LIST => "/mcp list",
                    _ => MCP_FULL_USAGE,
                }
            }
        }
    }

    /// A human-facing, actionable error message.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::MissingServer { action } => {
                format!("/mcp {action} needs a <server> argument.")
            }
            Self::UnexpectedArguments { action } => {
                format!("Unexpected arguments after /mcp {action}.")
            }
            Self::UnknownAction { action } => {
                format!(
                    "Unknown /mcp action '{action}'. Use list, show <server>, \
                     auth [list|<server>], logout <server>, or help."
                )
            }
        }
    }
}

impl McpAction {
    /// Parse already-tokenized `/mcp` arguments into a typed action.
    ///
    /// The empty slice maps to [`McpAction::List`], matching the bare `/mcp`.
    pub fn parse(tokens: &[&str]) -> Result<Self, McpCommandError> {
        match tokens {
            [] | ["list"] => Ok(Self::List),
            ["list", ..] => Err(McpCommandError::UnexpectedArguments {
                action: ACTION_LIST,
            }),
            ["help" | "-h" | "--help"] => Ok(Self::Help),
            ["show"] => Err(McpCommandError::MissingServer {
                action: ACTION_SHOW,
            }),
            ["show", server] => Ok(Self::Show((*server).to_string())),
            ["show", ..] => Err(McpCommandError::UnexpectedArguments {
                action: ACTION_SHOW,
            }),
            ["auth"] | ["auth", "list"] => Ok(Self::AuthList),
            ["auth", "list", ..] => Err(McpCommandError::UnexpectedArguments {
                action: "auth list",
            }),
            ["auth", server] => Ok(Self::Auth((*server).to_string())),
            ["auth", ..] => Err(McpCommandError::UnexpectedArguments {
                action: ACTION_AUTH,
            }),
            ["logout"] => Err(McpCommandError::MissingServer {
                action: ACTION_LOGOUT,
            }),
            ["logout", server] => Ok(Self::Logout((*server).to_string())),
            ["logout", ..] => Err(McpCommandError::UnexpectedArguments {
                action: ACTION_LOGOUT,
            }),
            [other, ..] => Err(McpCommandError::UnknownAction {
                action: (*other).to_string(),
            }),
        }
    }

    /// Lower a typed action back to the `(action, target)` pair carried by
    /// [`crate::SlashCommand::Mcp`], keeping that variant's shape consistent
    /// with its siblings while remaining round-trippable through the renderer.
    #[must_use]
    pub fn into_slash_parts(self) -> (Option<String>, Option<String>) {
        match self {
            Self::List => (None, None),
            Self::Help => (Some(ACTION_HELP.to_string()), None),
            Self::Show(server) => (Some(ACTION_SHOW.to_string()), Some(server)),
            Self::AuthList => (Some(ACTION_AUTH.to_string()), Some(ACTION_LIST.to_string())),
            Self::Auth(server) => (Some(ACTION_AUTH.to_string()), Some(server)),
            Self::Logout(server) => (Some(ACTION_LOGOUT.to_string()), Some(server)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_valid_forms() {
        let cases: &[(&[&str], McpAction)] = &[
            (&[], McpAction::List),
            (&["list"], McpAction::List),
            (&["help"], McpAction::Help),
            (&["-h"], McpAction::Help),
            (&["--help"], McpAction::Help),
            (&["show", "demo"], McpAction::Show("demo".to_string())),
            (&["auth"], McpAction::AuthList),
            (&["auth", "list"], McpAction::AuthList),
            (&["auth", "demo"], McpAction::Auth("demo".to_string())),
            (&["logout", "demo"], McpAction::Logout("demo".to_string())),
        ];
        for (tokens, expected) in cases {
            assert_eq!(
                McpAction::parse(tokens).as_ref(),
                Ok(expected),
                "tokens {tokens:?}"
            );
        }
    }

    #[test]
    fn rejects_invalid_forms_with_actionable_errors() {
        assert_eq!(
            McpAction::parse(&["show"]),
            Err(McpCommandError::MissingServer { action: "show" })
        );
        assert_eq!(
            McpAction::parse(&["logout"]),
            Err(McpCommandError::MissingServer { action: "logout" })
        );
        assert_eq!(
            McpAction::parse(&["show", "a", "b"]),
            Err(McpCommandError::UnexpectedArguments { action: "show" })
        );
        assert_eq!(
            McpAction::parse(&["auth", "list", "extra"]),
            Err(McpCommandError::UnexpectedArguments {
                action: "auth list"
            })
        );
        assert_eq!(
            McpAction::parse(&["bogus"]),
            Err(McpCommandError::UnknownAction {
                action: "bogus".to_string()
            })
        );
    }

    #[test]
    fn slash_parts_round_trip_through_parse() {
        for action in [
            McpAction::List,
            McpAction::Help,
            McpAction::Show("demo".to_string()),
            McpAction::AuthList,
            McpAction::Auth("demo".to_string()),
            McpAction::Logout("demo".to_string()),
        ] {
            let (act, target) = action.clone().into_slash_parts();
            let joined = match (act, target) {
                (None, None) => String::new(),
                (Some(a), None) => a,
                (Some(a), Some(t)) => format!("{a} {t}"),
                // into_slash_parts never yields (None, Some(_)); documented here.
                (None, Some(_)) => unreachable!("into_slash_parts never yields (None, Some(_))"),
            };
            let tokens: Vec<&str> = joined.split_whitespace().collect();
            assert_eq!(McpAction::parse(&tokens), Ok(action));
        }
    }
}
