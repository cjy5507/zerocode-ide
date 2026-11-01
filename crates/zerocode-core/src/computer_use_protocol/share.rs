//! Which apps the agents work in, and the person's hand back on them whenever
//! they want (t-73906; widened 2026-11-01, the person's words: "내 컴퓨터를 다
//! 사용할수있어야해", the agent can use the whole computer).
//!
//! By default an agent acts in any app without waiting: the app joins the ones in
//! use and the band names them, with the press that takes everything back. Several
//! agents work in several apps at once, so nothing one of them reaches limits the
//! others, and a look is never held to what another agent reaches. A card that
//! stood for thirty seconds before each new app was a stall per app for a person
//! who was away, and it interrupted one who was not.
//!
//! The person ending an app's use is a no until they say otherwise: its next act
//! asks them with a card, and silence keeps it refused. "Take control" refuses
//! every act until the person hands it back.
//!
//! When the person requires sharing (their setting, off by default) the old rule
//! stands: nothing acts in an app they have not handed over, an unnamed act is
//! refused, silence is no lasting no (t-74826) but makes the app wait, and while
//! apps are handed over, a look outside them or at the whole screen is refused.
//!
//! This module is the rule alone: the window asks, waits and paints.

use std::collections::{BTreeMap, BTreeSet};

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

/// What is remembered of an app besides the person's answers: they ended its use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Answered(Answer),
    /// The person ended it. The next act asks, and only a yes lets it go on.
    Ended,
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

/// How long the panel stays beside an app after its last act (ms).
const PANEL_STAYS_MS: u64 = 3_000;

/// The share, as the window holds it for its lifetime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Share {
    required: bool,
    /// The apps in use, by key: acted in, or handed over when sharing is required.
    targets: BTreeSet<String>,
    /// The app acted in last: what the panel stands beside.
    latest: Option<String>,
    /// The app the panel stands beside, and when it was last acted in there (ms).
    beside: Option<(String, u64)>,
    answers: BTreeMap<String, Standing>,
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
    /// End the displayed share: every app in use, each remembered as ended. A
    /// late press cannot end a share made since; requests already admitted lose
    /// their generation even when the same app is used again.
    #[must_use]
    pub fn end(&mut self, app: &str, generation: u64) -> bool {
        if !self.targets.contains(app) || self.generation != generation {
            return false;
        }
        for ended in std::mem::take(&mut self.targets) {
            self.silent_at.remove(&ended);
            if self.waiting.as_deref() == Some(ended.as_str()) {
                self.waiting = None;
            }
            self.answers.insert(ended, Standing::Ended);
        }
        self.latest = None;
        self.taken_back = false;
        self.generation += 1;
        true
    }

    /// Whether an act must wait for an explicit share (off by default).
    ///
    /// Turned on, only what the person handed over stays: an app the agents were
    /// merely using goes back to waiting for its card.
    pub fn set_required(&mut self, required: bool) {
        if required && !self.required {
            let handed_over = |app: &String, answers: &BTreeMap<String, Standing>| {
                answers.get(app) == Some(&Standing::Answered(Answer::Shared))
            };
            let answers = &self.answers;
            self.targets.retain(|app| handed_over(app, answers));
            if self
                .latest
                .as_ref()
                .is_some_and(|app| !self.targets.contains(app))
            {
                self.latest = self.targets.iter().next_back().cloned();
            }
        }
        self.required = required;
    }

    /// Whether an act must wait for an explicit share.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.required
    }

    /// The app acted in last, while any is in use.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.latest.as_deref()
    }

    /// The app the panel stands beside: the one acted in last, once a moment has
    /// passed since another's act.
    #[must_use]
    pub fn panel_target(&self) -> Option<&str> {
        let latest = self.latest.as_deref()?;
        Some(
            self.beside
                .as_ref()
                .map(|(beside, _)| beside.as_str())
                .filter(|beside| self.targets.contains(*beside))
                .unwrap_or(latest),
        )
    }

    /// Every app in use, by key.
    pub fn targets(&self) -> impl Iterator<Item = &str> {
        self.targets.iter().map(String::as_str)
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

    /// What `reach` may do now, at `now_ms` on the window's clock. An app that
    /// may be acted in joins the ones in use.
    pub fn gate(&mut self, reach: Reach<'_>, now_ms: u64) -> Gate {
        match self.judge(reach, now_ms) {
            Judged::Go => {
                if reach.acts
                    && let Some(app) = reach.app
                    && self.targets.contains(app)
                {
                    self.latest = Some(app.to_string());
                    self.panel_follows(app, now_ms);
                }
                Gate::Go
            }
            Judged::ShareAgain(app) => {
                self.targets.insert(app.clone());
                self.panel_follows(&app, now_ms);
                self.latest = Some(app);
                Gate::Go
            }
            Judged::Ask(app) => Gate::Ask(app),
            Judged::Refuse(error) => Gate::Refuse(error),
        }
    }

    /// Whether a request admitted before may take its next step, at `now_ms`:
    /// judged as [`Self::gate`] judges a new request — the same refusals,
    /// word for word — without moving anything (t-74826). What the gate would
    /// ask about is refused, since no card is raised mid-step.
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
        if !self.required {
            return self.judge_free(reach);
        }
        self.judge_required(reach, now_ms)
    }

    /// Sharing not required: every look goes, every act goes, and only what the
    /// person said no to stops.
    fn judge_free(&self, reach: Reach<'_>) -> Judged {
        if !reach.acts {
            return Judged::Go;
        }
        let Some(app) = reach.app else {
            return Judged::Go;
        };
        if self.targets.contains(app) {
            return Judged::Go;
        }
        match self.answers.get(app) {
            Some(Standing::Answered(Answer::Declined)) => {
                Judged::Refuse(scope(&format!("the person did not hand {app} over")))
            }
            Some(Standing::Ended) => Judged::Ask(app.to_string()),
            _ => Judged::ShareAgain(app.to_string()),
        }
    }

    /// Sharing required: only what the person handed over, and while anything is
    /// handed over, nothing outside it is even looked at.
    fn judge_required(&self, reach: Reach<'_>, now_ms: u64) -> Judged {
        if self.targets.is_empty() {
            if !reach.acts {
                return Judged::Go;
            }
            return match reach.app {
                None => Judged::Refuse(required()),
                Some(app) => self.required_act(app, now_ms),
            };
        }
        let held = self.held_words();
        if reach.whole_screen {
            return Judged::Refuse(scope(&format!(
                "the person shared {held} only; look at that app's window, not the whole screen"
            )));
        }
        match reach.app {
            Some(app) if self.targets.contains(app) => Judged::Go,
            None if !reach.acts => Judged::Go,
            None => Judged::Refuse(scope(&format!(
                "the person shared {held} only; name it as --app"
            ))),
            Some(app) if !reach.acts => {
                Judged::Refuse(scope(&format!("the person shared {held} only, not {app}")))
            }
            Some(app) => self.required_act(app, now_ms),
        }
    }

    fn required_act(&self, app: &str, now_ms: u64) -> Judged {
        match self.answers.get(app) {
            Some(Standing::Answered(Answer::Shared)) => Judged::ShareAgain(app.to_string()),
            Some(Standing::Answered(Answer::Declined)) => {
                Judged::Refuse(scope(&format!("the person did not hand {app} over")))
            }
            Some(Standing::Answered(Answer::Silent)) => self.silent_required(app, now_ms),
            Some(Standing::Ended) | None => Judged::Ask(app.to_string()),
        }
    }

    fn held_words(&self) -> String {
        self.targets.iter().cloned().collect::<Vec<_>>().join(", ")
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

    /// The person's answer about `app`, at `now_ms`. A yes shares it. Silence
    /// leaves it waiting when sharing is required, and keeps it refused after the
    /// person ended it: what they took away does not come back by itself.
    pub fn answered(&mut self, app: &str, answer: Answer, now_ms: u64) {
        if answer == Answer::Silent {
            if !self.required && self.answers.get(app) == Some(&Standing::Ended) {
                self.settle(app, Answer::Declined);
                return;
            }
            self.answers
                .insert(app.to_string(), Standing::Answered(answer));
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

    /// The panel goes to the app acted in, unless another was acted in a moment ago:
    /// agents taking turns in two apps would walk it between their windows. Where the
    /// person hands over each app, the panel stays beside the one in use.
    fn panel_follows(&mut self, app: &str, now_ms: u64) {
        let another_in_use = !self.required
            && self.beside.as_ref().is_some_and(|(beside, at)| {
                beside != app
                    && self.targets.contains(beside)
                    && now_ms < at.saturating_add(PANEL_STAYS_MS)
            });
        if !another_in_use {
            self.beside = Some((app.to_string(), now_ms));
        }
    }

    /// A yes or a no about `app`: it ends any wait for it.
    fn settle(&mut self, app: &str, answer: Answer) {
        self.answers
            .insert(app.to_string(), Standing::Answered(answer));
        self.silent_at.remove(app);
        if self.waiting.as_deref() == Some(app) {
            self.waiting = None;
        }
        if answer == Answer::Shared {
            self.targets.insert(app.to_string());
            self.latest = Some(app.to_string());
            // The person just named it: the panel goes there at once.
            self.beside = None;
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
