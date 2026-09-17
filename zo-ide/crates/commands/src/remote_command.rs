use super::slash_commands::SlashCommandParseError;

/// Session-local remote-control lifecycle actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteAction {
    /// Open the in-app Remote onboarding and status modal.
    Open,
    Start,
    Status,
    Qr,
    Rotate,
    Stop,
    Approve { code: String },
    Deny { code: String },
}

pub(super) fn parse_remote_action(args: &[&str]) -> Result<RemoteAction, SlashCommandParseError> {
    match args {
        [] => Ok(RemoteAction::Open),
        ["start"] => Ok(RemoteAction::Start),
        ["status"] => Ok(RemoteAction::Status),
        ["qr"] => Ok(RemoteAction::Qr),
        ["rotate"] => Ok(RemoteAction::Rotate),
        ["stop"] => Ok(RemoteAction::Stop),
        ["approve", code] if !code.trim().is_empty() => Ok(RemoteAction::Approve {
            code: code.to_ascii_uppercase(),
        }),
        ["deny", code] if !code.trim().is_empty() => Ok(RemoteAction::Deny {
            code: code.to_ascii_uppercase(),
        }),
        _ => Err(SlashCommandParseError::new(
            "Usage: /remote [start|status|qr|rotate|stop|approve <code>|deny <code>]",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{RemoteAction, parse_remote_action};

    #[test]
    fn bare_remote_onboarding_is_distinct_from_explicit_start() {
        assert_eq!(parse_remote_action(&[]), Ok(RemoteAction::Open));
        assert_eq!(parse_remote_action(&["start"]), Ok(RemoteAction::Start));
    }

    #[test]
    fn parses_lifecycle_and_pairing_actions() {
        assert_eq!(parse_remote_action(&[]), Ok(RemoteAction::Open));
        assert_eq!(parse_remote_action(&["start"]), Ok(RemoteAction::Start));
        assert_eq!(parse_remote_action(&["status"]), Ok(RemoteAction::Status));
        assert_eq!(parse_remote_action(&["qr"]), Ok(RemoteAction::Qr));
        assert_eq!(parse_remote_action(&["rotate"]), Ok(RemoteAction::Rotate));
        assert_eq!(parse_remote_action(&["stop"]), Ok(RemoteAction::Stop));
        assert_eq!(
            parse_remote_action(&["approve", "ab12-cd"]),
            Ok(RemoteAction::Approve {
                code: "AB12-CD".to_string(),
            })
        );
        assert!(parse_remote_action(&["approve"]).is_err());
        assert!(parse_remote_action(&["unknown"]).is_err());
    }
}
