//! The requests an explanation has in flight, and what a pane's life does to
//! each of them (t-32787).
//!
//! A request for a page is made at one moment and finished at another, and in
//! between the pane it went to lives its own life: its agent is in a turn and
//! cannot be spoken to yet, a person starts typing on the line, a question
//! parks on the screen, the agent publishes a page, the turn ends, the pane
//! closes. This is the table those events are read against. It has no clock, no
//! thread and no window — every call is one event handed in with the facts it
//! carries — so that what the window does at each of them can be tried without
//! a pane behind it.
//!
//! **Nothing here polls.** A request that waits for a busy pane is released by
//! the event that says the pane is between turns ([`Desk::release_waiting`]); a
//! page is found by the publication that names it ([`Desk::page_published`]);
//! a request that got no page is found by the turn that ended without one
//! ([`Desk::turn_ended`]). A pane that never reports any of these leaves its
//! request standing, and a person's cancel is what removes it.
//!
//! **The table is small on purpose.** A request is removed the moment it
//! finishes — ready, failed, cancelled — so what stands is only what is still
//! in flight, held to [`crate::explain::ACTIVE_MAX`], and the words of a
//! request are kept only while its pane is busy.

use crate::explain::{ACTIVE_MAX, why};
use crate::hook::HookState;

/// A terminal's number, as the window numbers them.
pub type Term = u32;

/// Where a request stands. The first four are what the desk holds; the last
/// two are what a request ends as, and are only ever said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The pane's agent is in a turn; the words wait for it to end.
    Waiting,
    /// The words are on their way into the pane — a delivery is registered.
    Sent,
    /// The pane took the words; the agent is making the page.
    Asked,
    /// A one-shot is running; it has no pane.
    Running,
    /// The page exists and is open.
    Ready,
    /// It did not come to a page; the reason travels with it.
    Failed,
}

impl State {
    /// The wire word the window reads the state by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Sent => "sent",
            Self::Asked => "asked",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    /// Every state, for a table that must word each one.
    pub const ALL: [Self; 6] = [
        Self::Waiting,
        Self::Sent,
        Self::Asked,
        Self::Running,
        Self::Ready,
        Self::Failed,
    ];
}

/// One request the desk holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// The name the window gave it — what every event about it carries.
    pub id: String,
    /// The pane it concerns; `None` for a one-shot, which has none.
    pub term: Option<Term>,
    /// The agent that makes the page, as the catalog spells it.
    pub agent: String,
    pub state: State,
    /// When it was made, so that a publication from before it is not taken for
    /// its answer.
    pub made_ms: i64,
    /// The words to send — kept only while the request waits for a busy pane.
    pub prompt: Option<String>,
}

/// What the window knows about a pane at the moment a request is made for it,
/// or released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaneFacts<'a> {
    /// The terminal exists.
    pub present: bool,
    /// The agent that sits in it, as the catalog spells it.
    pub agent: Option<&'a str>,
    /// The agent says it is in a turn.
    pub busy: bool,
    /// A question or an approval is on the screen, waiting for a person.
    pub parked: bool,
    /// The person has typed on the line and nothing has taken it.
    pub draft: bool,
}

/// What to do with a request for a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Register the delivery now.
    Send,
    /// Hold the request until the agent is between turns.
    Wait,
    /// Do not touch the pane; say why.
    Refuse(&'static str),
}

/// What the facts of a pane allow, in the order a person would care: a pane
/// that is not there, an agent that is not, a question that is theirs to answer,
/// words that are theirs on the line — all of these are refusals, because the
/// door never types over a person — and only then whether to wait.
#[must_use]
pub fn decide(pane: &PaneFacts<'_>) -> Decision {
    if !pane.present {
        return Decision::Refuse(why::NO_PANE);
    }
    if pane.agent.is_none() {
        return Decision::Refuse(why::NO_AGENT);
    }
    if pane.parked {
        return Decision::Refuse(why::PARKED);
    }
    if pane.draft {
        return Decision::Refuse(why::HOLDS_A_DRAFT);
    }
    if pane.busy {
        return Decision::Wait;
    }
    Decision::Send
}

/// What one report of a pane's state means to the requests that went to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// A turn going on, a question parked, a session starting: nothing a
    /// request is waiting for.
    Nothing,
    /// The agent ended a turn: a request that waited for it is released, and one
    /// that was asked and got no page is finished.
    TurnEnded,
    /// The agent is no longer in the pane: every request of it is finished.
    AgentGone,
}

/// One report of a pane's state, read for the requests in flight. A session
/// boundary wearing `done` is a session starting — a compaction in the middle
/// of a turn writes one — and not a turn ending, so it is not heard.
#[must_use]
pub fn heard(state: HookState, session_boundary: bool) -> Heard {
    match state {
        HookState::Working | HookState::NeedsAttention => Heard::Nothing,
        HookState::Done if session_boundary => Heard::Nothing,
        HookState::Done => Heard::TurnEnded,
        HookState::Idle => Heard::AgentGone,
    }
}

/// The requests in flight.
#[derive(Debug, Default)]
pub struct Desk {
    held: Vec<Held>,
}

impl Desk {
    /// How many requests stand.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether nothing stands — what the pane-state road asks first, so that a
    /// window with no request pays nothing for the question.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The request named `id`, if it stands.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Held> {
        self.held.iter().find(|one| one.id == id)
    }

    /// Take a request in, or say why not: the table is full, or its pane
    /// already has a request that has not finished.
    ///
    /// # Errors
    ///
    /// The [`why`] token for the refusal.
    pub fn admit(&mut self, held: Held) -> Result<(), &'static str> {
        if self.held.len() >= ACTIVE_MAX {
            return Err(why::TOO_MANY);
        }
        if held.term.is_some() && self.held.iter().any(|one| one.term == held.term) {
            return Err(why::IN_FLIGHT);
        }
        self.held.push(held);
        Ok(())
    }

    /// Move a standing request to another live state. `false` when no such
    /// request stands.
    pub fn move_to(&mut self, id: &str, state: State) -> bool {
        match self.held.iter_mut().find(|one| one.id == id) {
            Some(one) => {
                one.state = state;
                true
            }
            None => false,
        }
    }

    /// The pane's agent is between turns: the request that waited for it comes
    /// out — now `Sent` — with its words, once. The desk keeps no copy of them.
    pub fn release_waiting(&mut self, term: Term) -> Option<Held> {
        let one = self
            .held
            .iter_mut()
            .find(|one| one.term == Some(term) && one.state == State::Waiting)?;
        let words = one.prompt.take();
        one.state = State::Sent;
        let mut released = one.clone();
        released.prompt = words;
        Some(released)
    }

    /// The pane published a page at `at_ms`: the request that asked for one is
    /// finished, and its id is answered. A request still waiting has not asked
    /// anything yet, and a page from before the request was made is not its
    /// answer.
    pub fn page_published(&mut self, term: Term, at_ms: i64) -> Option<String> {
        let at = self.held.iter().position(|one| {
            one.term == Some(term)
                && matches!(one.state, State::Sent | State::Asked)
                && one.made_ms <= at_ms
        })?;
        Some(self.held.remove(at).id)
    }

    /// The pane's agent ended a turn: a request that was asked and got no page
    /// is finished, and its id is answered. A request that is only sent has not
    /// been taken yet, so the turn that ended was not its own.
    pub fn turn_ended(&mut self, term: Term) -> Option<String> {
        let at = self
            .held
            .iter()
            .position(|one| one.term == Some(term) && one.state == State::Asked)?;
        Some(self.held.remove(at).id)
    }

    /// The pane is gone, or its agent: every request of it is finished, and
    /// their ids are answered.
    pub fn pane_gone(&mut self, term: Term) -> Vec<String> {
        let mut gone = Vec::new();
        self.held.retain(|one| {
            if one.term == Some(term) {
                gone.push(one.id.clone());
                false
            } else {
                true
            }
        });
        gone
    }

    /// Take a request out — it finished, failed or was cancelled — as it stood.
    pub fn finish(&mut self, id: &str) -> Option<Held> {
        let at = self.held.iter().position(|one| one.id == id)?;
        Some(self.held.remove(at))
    }
}

#[cfg(test)]
mod tests;
