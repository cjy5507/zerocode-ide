//! Why a provider credential could not be used — and whether anything was
//! configured at all.
//!
//! The two answers mean different things to whoever reads them (t-6248).
//! "Nothing is configured" is a fact about this machine: asking the provider
//! again changes nothing until a person signs in, so a model list records it
//! as a skip and waits. "Something is configured and did not work" — a
//! session that expired and would not refresh, a keychain that would not
//! answer, a key the endpoint refused — is a failure: the rows the provider
//! answered last time stay, and the next connection asks again. Reading the
//! second as the first is how one zo whose Claude Code refresh token had been
//! superseded told every zo on the machine that no Anthropic credential
//! existed, and Opus 5.5 vanished for an hour (2026-09-23).

/// Why a provider's credential could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialMiss {
    /// Nothing is configured where this provider's credential lives.
    Absent,
    /// Something is configured and could not be used — why, in a person's
    /// words, naming the way out when there is one. Never carries a token.
    Unusable(String),
}

impl CredentialMiss {
    /// The first reason to say, in the order a resolution chain tried its
    /// rungs: once one rung found something it could not use, a later rung
    /// that found nothing does not make the whole chain "absent".
    #[must_use]
    pub fn or(self, later: Self) -> Self {
        match self {
            Self::Unusable(_) => self,
            Self::Absent => later,
        }
    }
}
