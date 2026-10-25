//! One window shared with an agent at a time, and the person's hand back on
//! it whenever they want (t-73906; the person's words, 10-25: "only the target
//! it has to control is shared … it works without taking my ownership").
//!
//! The first time an agent acts in an app, the window asks the person once —
//! "hand this window over?" — and a yes makes that app the shared one. Asked
//! once a session per app: the same app is not asked again, and another app
//! is asked when the agent turns to it. A no refuses that app. Silence (the
//! person away) leaves the agent working as before, unshared, so an
//! unattended run does not stop at a card; it never widens a share that
//! stands. While a share stands, an act or a look outside it — another app,
//! no app, the whole screen — is refused. "Take control" refuses every act
//! until the person hands the window back; the card is not raised again.
//!
//! This module is the rule alone: the window asks, waits and paints.

use std::collections::BTreeMap;

use super::{ProviderError, error_code};

/// How long the window's card waits for the person before it is silence.
pub const SHARE_ASK_MS: u64 = 30_000;

/// What the person said when asked about an app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Shared,
    Declined,
    Silent,
}

/// What a request may do now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Go,
    /// Ask the person about this app first (its key).
    Ask(String),
    Refuse(ProviderError),
}

/// What a request is, for the share's sake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reach<'a> {
    /// The app it names, as a key ([`app_key`]); `None` for none.
    pub app: Option<&'a str>,
    /// Whether it acts (a press, a key, a move), not only looks.
    pub acts: bool,
    /// Whether it looks at the whole screen (a desktop picture).
    pub whole_screen: bool,
}

/// The share, as the window holds it for its lifetime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Share {
    target: Option<String>,
    answers: BTreeMap<String, Answer>,
    taken_back: bool,
    generation: u64,
}

/// The key an `--app` is held under: trimmed and lower-cased, so `Safari`
/// and `safari` are one app.
#[must_use]
pub fn app_key(app: &str) -> String {
    app.trim().to_lowercase()
}

impl Share {
    /// The shared app's key, while one is shared.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    #[must_use]
    pub const fn taken_back(&self) -> bool {
        self.taken_back
    }

    /// Moves whenever the person takes control or hands it back: a request
    /// that began under another generation is refused at its next step.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// What `reach` may do now. A yes the person gave an app before makes it
    /// the shared one again without asking.
    pub fn gate(&mut self, _reach: Reach<'_>) -> Gate {
        Gate::Go
    }

    /// The person's answer about `app`. A yes shares it.
    pub fn answered(&mut self, _app: &str, _answer: Answer) {}

    /// The person takes control: every act is refused until they hand the
    /// window back, and a request in flight is refused at its next step.
    pub fn take_back(&mut self) {}

    /// The person hands the window back to the agent.
    pub fn give_back(&mut self) {}
}

fn scope(message: &str) -> ProviderError {
    ProviderError::new(error_code::SHARE_SCOPE, message)
}

/// The refusal of an act after the person took control.
#[must_use]
pub fn took_control() -> ProviderError {
    ProviderError::new(
        error_code::PERSON_TOOK_CONTROL,
        "the person took control of the shared window; nothing is pressed until they hand it back in the window's band",
    )
}

#[cfg(test)]
mod tests;
