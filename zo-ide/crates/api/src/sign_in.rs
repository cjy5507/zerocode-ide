//! Where a person signs a provider's login in again, in this build — the one
//! table every login message reads (t-11378).
//!
//! zo has no sign-in command of its own: there is no `/login` in the composer
//! and no `zo login` verb. Each login lives where another program keeps it —
//! the window's account settings, or that provider's own CLI — and zo reads
//! it from there, so the way back is always that program's. A message that
//! sent a person to `/login` sent them nowhere.

/// The ways back in to a Claude login: the window's account row, or the
/// account's own CLI. A macro so `concat!` can build the fixed refusals that
/// a waiting turn recognises by value (`MISSING_CLAUDE_LOGIN`).
macro_rules! claude_sign_in_road {
    () => {
        "the window's Settings › Claude accounts › Sign in again, or `claude` in a terminal"
    };
}

/// One provider's login and where it is made again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignInRoad {
    /// The provider's name as a credential error carries it
    /// (`ApiError::MissingCredentials`, `provider_name`).
    pub provider: &'static str,
    /// The account as a person knows it.
    pub account: &'static str,
    /// Where that login is signed in again.
    pub road: &'static str,
}

/// Claude: the window's account row or `claude` (t-11045).
pub const CLAUDE: SignInRoad = SignInRoad {
    provider: "Anthropic",
    account: "Claude",
    road: claude_sign_in_road!(),
};

/// ChatGPT: zo follows the window's chosen Codex account, else the Codex
/// home the terminal names (`managed_account::resolve_codex_home`).
pub const CHATGPT: SignInRoad = SignInRoad {
    provider: "OpenAI",
    account: "ChatGPT",
    road: "the window's Settings › Codex accounts, or `codex login` in a terminal",
};

/// Gemini: a saved Code Assist login cannot be made in this build, so the
/// way back is Google's own credentials (`google_auth`) or a key.
pub const GEMINI: SignInRoad = SignInRoad {
    provider: "Google",
    account: "Gemini",
    road: "`gcloud auth application-default login` in a terminal, or GOOGLE_API_KEY in zo's environment",
};

/// Grok: zo reads the session the Grok CLI keeps (`cli_sessions`).
pub const GROK: SignInRoad = SignInRoad {
    provider: "xAI",
    account: "Grok",
    road: "`grok login` in a terminal",
};

/// Every provider zo reads a login for, in the order a list shows them.
pub const SIGN_IN_ROADS: &[SignInRoad] = &[CLAUDE, CHATGPT, GEMINI, GROK];

/// The road for a provider as a credential error names it.
#[must_use]
pub fn road_for(provider: &str) -> Option<&'static SignInRoad> {
    SIGN_IN_ROADS.iter().find(|road| road.provider == provider)
}

/// What is wrong with a login a request was refused for — each has its own
/// first step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginTrouble {
    /// The provider revoked the token: something else renewed or replaced
    /// this login since zo read it. The newer login is already where zo reads
    /// it, so trying again comes before signing in.
    Revoked,
    /// The login expired, or the provider does not accept it.
    Expired,
}

impl LoginTrouble {
    /// Which trouble a 401's own words describe.
    #[must_use]
    pub fn of_refusal<'a>(words: impl IntoIterator<Item = &'a str>) -> Self {
        if words
            .into_iter()
            .any(|word| word.to_ascii_lowercase().contains("revoked"))
        {
            Self::Revoked
        } else {
            Self::Expired
        }
    }

    /// The advice for a refusal whose provider is not known here: what to do
    /// first, then every provider's road, since the model says which it is.
    #[must_use]
    pub fn advice(self) -> String {
        let first = match self {
            Self::Revoked => {
                "The provider revoked this login — another program renewed it or signed in again since zo read it.\n  \
                 Try again first: zo reads the login again on the next request and picks up the newer one.\n  \
                 If it is refused again, sign in again where this model's provider keeps its login:"
            }
            Self::Expired => {
                "Authentication failed — the login expired or is not valid.\n  \
                 Sign in again where this model's provider keeps its login, then retry:"
            }
        };
        format!("{first}\n{}", every_road())
    }
}

/// Every road, one line each, for a message that does not know whose login
/// failed.
#[must_use]
pub fn every_road() -> String {
    SIGN_IN_ROADS
        .iter()
        .map(|road| format!("    • {}: {}", road.account, road.road))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{road_for, LoginTrouble, SIGN_IN_ROADS};

    #[test]
    fn a_revoked_login_is_retried_before_it_is_signed_in_again() {
        let revoked = LoginTrouble::of_refusal(["authentication_error", "OAuth access token has been revoked."]);
        assert_eq!(revoked, LoginTrouble::Revoked);
        let advice = revoked.advice();
        let retry = advice.find("Try again first").expect("retry is named");
        let sign_in = advice.find("sign in again").expect("signing in is named");
        assert!(retry < sign_in, "{advice}");
        assert_eq!(
            LoginTrouble::of_refusal(["Invalid authentication credentials"]),
            LoginTrouble::Expired
        );
    }

    #[test]
    fn every_provider_a_credential_error_names_has_one_road() {
        for provider in ["Anthropic", "OpenAI", "Google", "xAI"] {
            assert!(road_for(provider).is_some(), "{provider}");
        }
        assert!(road_for("Ollama").is_none(), "an API-key adapter has no sign-in");
        let advice = LoginTrouble::Expired.advice();
        for road in SIGN_IN_ROADS {
            assert!(advice.contains(road.road), "{advice}");
        }
    }
}
