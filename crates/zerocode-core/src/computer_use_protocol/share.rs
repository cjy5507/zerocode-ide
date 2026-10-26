//! One window shared with an agent at a time, and the person's hand back on
//! it whenever they want (t-73906; the person's words, 10-25: "only the target
//! it has to control is shared … it works without taking my ownership").
//!
//! The first time an agent acts in an app, the window asks the person once —
//! "hand this window over?" — and a yes makes that app the shared one. Asked
//! once a session per app, unless the person ends its share: that forgets
//! its answer and invalidates requests in flight. Another app is asked when
//! the agent turns to it. A no refuses that app. Silence (the
//! person away) leaves the agent working as before, unshared, so an
//! unattended run does not stop at a card by default. When the person requires
//! sharing, unnamed acts are refused and silence is no lasting no (t-74826):
//! the app waits for the person. Its acts are refused at once, with no card,
//! for [`SHARE_ASK_AGAIN_MS`]; the first act after that asks again; and the
//! person can hand the waiting app over from the window at any time. Looks
//! keep the same rules. Silence never widens a share that
//! stands. While a share stands, an act or a look outside it — another app,
//! no app, the whole screen — is refused. "Take control" refuses every act
//! until the person hands the window back; the card is not raised again.
//!
//! This module is the rule alone: the window asks, waits and paints.

use std::collections::BTreeMap;

use super::{ProviderError, error_code};

/// How long the window's card waits for the person before it is silence.
pub const SHARE_ASK_MS: u64 = 30_000;

/// With sharing required, how long after a silence the same app is refused
/// without a card. Silence means the person is away; a card raised again at
/// every retry would stand on their screen most of the time they are gone.
/// After this quiet one card stands at most 30 s of every 150 s, and the
/// band offers the hand-over the whole time, so a person who comes back
/// never waits for a card.
pub const SHARE_ASK_AGAIN_MS: u64 = 120_000;

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

/// The rule's verdict before anything moves: [`Share::gate`] carries it out,
/// [`Share::still_holds`] only reads it.
#[derive(Debug)]
enum Judged {
    Go,
    /// Go, and an app the person said yes to before is the shared one again.
    ShareAgain(String),
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
    required: bool,
    target: Option<String>,
    answers: BTreeMap<String, Answer>,
    /// When each app's card went unanswered, on the window's clock (ms).
    silent_at: BTreeMap<String, u64>,
    /// The app whose card went unanswered last, while no answer replaced it.
    waiting: Option<String>,
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
    /// End the displayed app's share, forgetting its answer. A late press
    /// cannot end a share handed over since; requests already admitted lose
    /// their generation even when the same app is shared again.
    #[must_use]
    pub fn end(&mut self, app: &str, generation: u64) -> bool {
        if self.target() != Some(app) || self.generation != generation {
            return false;
        }
        self.target = None;
        self.answers.remove(app);
        self.silent_at.remove(app);
        if self.waiting.as_deref() == Some(app) {
            self.waiting = None;
        }
        self.taken_back = false;
        self.generation += 1;
        true
    }

    /// Whether an act must wait for an explicit share (off by default).
    pub fn set_required(&mut self, required: bool) {
        self.required = required;
    }

    /// Whether an act must wait for an explicit share.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.required
    }

    /// The shared app's key, while one is shared.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// The app that waits for the person to hand it over: the last one whose
    /// card went unanswered, while sharing is required.
    #[must_use]
    pub fn waiting(&self) -> Option<&str> {
        self.waiting.as_deref().filter(|_| self.required)
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

    /// What `reach` may do now, at `now_ms` on the window's clock. A yes the
    /// person gave an app before makes it the shared one again without asking.
    pub fn gate(&mut self, reach: Reach<'_>, now_ms: u64) -> Gate {
        match self.judge(reach, now_ms) {
            Judged::Go => Gate::Go,
            Judged::ShareAgain(app) => {
                self.target = Some(app);
                Gate::Go
            }
            Judged::Ask(app) => Gate::Ask(app),
            Judged::Refuse(error) => Gate::Refuse(error),
        }
    }

    /// Whether a request admitted before may take its next step, at `now_ms`:
    /// judged as [`Self::gate`] judges a new request — the same refusals,
    /// word for word — without moving anything (t-74826). An app said yes to
    /// before goes on without the share coming back to it from the app the
    /// person handed over since; what the gate would ask about is refused,
    /// since no card is raised mid-step.
    pub fn still_holds(&self, reach: Reach<'_>, now_ms: u64) -> Result<(), ProviderError> {
        match self.judge(reach, now_ms) {
            Judged::Go | Judged::ShareAgain(_) => Ok(()),
            Judged::Ask(_) if self.required => Err(required()),
            Judged::Ask(app) => Err(scope(&format!(
                "the person has not answered for {app} yet; a new request in {app} asks them"
            ))),
            Judged::Refuse(error) => Err(error),
        }
    }

    /// The share's rule for `reach` at `now_ms`, read without moving anything.
    fn judge(&self, reach: Reach<'_>, now_ms: u64) -> Judged {
        if reach.acts && self.taken_back {
            return Judged::Refuse(took_control());
        }
        let Some(target) = self.target.as_deref() else {
            if !reach.acts {
                return Judged::Go;
            }
            return match reach.app {
                None if self.required => Judged::Refuse(required()),
                None => Judged::Go,
                Some(app) => self.first_act(app, now_ms),
            };
        };
        if reach.whole_screen {
            return Judged::Refuse(scope(&format!(
                "the person shared {target} only; look at that app's window, not the whole screen"
            )));
        }
        match reach.app {
            Some(app) if app == target => Judged::Go,
            None if !reach.acts => Judged::Go,
            None => Judged::Refuse(scope(&format!(
                "the person shared {target} only; name it as --app"
            ))),
            Some(app) if !reach.acts => Judged::Refuse(scope(&format!(
                "the person shared {target} only, not {app}"
            ))),
            Some(app) => match self.answers.get(app) {
                Some(Answer::Shared) => Judged::ShareAgain(app.to_string()),
                Some(Answer::Silent) if self.required => self.silent_required(app, now_ms),
                Some(Answer::Declined | Answer::Silent) => Judged::Refuse(scope(&format!(
                    "the person shared {target} only, not {app}"
                ))),
                None => Judged::Ask(app.to_string()),
            },
        }
    }

    fn first_act(&self, app: &str, now_ms: u64) -> Judged {
        match self.answers.get(app) {
            Some(Answer::Shared) => Judged::ShareAgain(app.to_string()),
            Some(Answer::Declined) => {
                Judged::Refuse(scope(&format!("the person did not hand {app} over")))
            }
            Some(Answer::Silent) if self.required => self.silent_required(app, now_ms),
            Some(Answer::Silent) => Judged::Go,
            None => Judged::Ask(app.to_string()),
        }
    }

    /// An unanswered app under required sharing: refused while the quiet
    /// after its silence lasts, asked again after it.
    fn silent_required(&self, app: &str, now_ms: u64) -> Judged {
        let again = self
            .silent_at
            .get(app)
            .map_or(now_ms, |at| at.saturating_add(SHARE_ASK_AGAIN_MS));
        if now_ms >= again {
            return Judged::Ask(app.to_string());
        }
        Judged::Refuse(waiting_for(app, again - now_ms))
    }

    /// The person's answer about `app`, at `now_ms`. A yes shares it; silence
    /// leaves it waiting, should sharing be required.
    pub fn answered(&mut self, app: &str, answer: Answer, now_ms: u64) {
        if answer == Answer::Silent {
            self.answers.insert(app.to_string(), answer);
            self.silent_at.insert(app.to_string(), now_ms);
            self.waiting = Some(app.to_string());
            return;
        }
        self.settle(app, answer);
    }

    /// The person hands the waiting app over from the window, without a card:
    /// that app only, and only while it waits. Whether it was handed over.
    pub fn hand_over(&mut self, app: &str) -> bool {
        if self.waiting() != Some(app) {
            return false;
        }
        self.settle(app, Answer::Shared);
        true
    }

    /// A yes or a no about `app`: it ends any wait for it.
    fn settle(&mut self, app: &str, answer: Answer) {
        self.answers.insert(app.to_string(), answer);
        self.silent_at.remove(app);
        if self.waiting.as_deref() == Some(app) {
            self.waiting = None;
        }
        if answer == Answer::Shared {
            self.target = Some(app.to_string());
        }
    }

    /// The person takes control: every act is refused until they hand the
    /// window back, and a request in flight is refused at its next step.
    pub fn take_back(&mut self) {
        if !self.taken_back {
            self.taken_back = true;
            self.generation += 1;
        }
    }

    /// The person hands the window back to the agent.
    pub fn give_back(&mut self) {
        if self.taken_back {
            self.taken_back = false;
            self.generation += 1;
        }
    }
}

/// An act needs the person to explicitly share its app.
#[must_use]
pub fn required() -> ProviderError {
    ProviderError::new(
        error_code::SHARE_REQUIRED,
        "sharing is required for actions; the person must explicitly hand this app over in the window before it can act (silence does not grant a share)",
    )
}

/// An act in an app that waits for the person under required sharing: when
/// the window asks again, and where the person can hand it over meanwhile.
fn waiting_for(app: &str, again_ms: u64) -> ProviderError {
    ProviderError::new(
        error_code::SHARE_REQUIRED,
        format!(
            "sharing is required for actions and the person did not answer the share card for {app}; nothing was done. The person can hand {app} over from the window's band at any time, and your first act in {app} after {} s asks them again. Tell the person you are waiting for {app}; do not retry sooner",
            again_ms.div_ceil(1_000)
        ),
    )
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
