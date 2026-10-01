//! The assign moment's one request (t-15554).
//!
//! A summons that leaves a dial open asks two seats: the difficulty of the
//! work, and — when nobody named a model — the model and the effort it runs
//! at ([`crate::summon_model`]). Both read the same state, the task's words
//! and history (the model question adds today's models beside them), so
//! they ride ONE request, the state's words sent once, behind one wall
//! ([`crate::summon_difficulty::APPLY_DEADLINE_MS`]). Each seat keeps its own
//! row, rubric and outcome; the rows name the request they shared
//! ([`SHARED_REQUEST_KEY`]).
//!
//! **The agent question joins them (t-16578).** A summons that leaves the
//! agent open asks a third seat, which agent runs the work
//! ([`crate::summon_choice`]), over the same state's words. What the pair
//! question offers depends on which agent runs it — its lineup, its efforts —
//! so by the design rule (a question whose state is built from another's
//! answer is a second request) it would wait for the agent's answer. It does
//! not here, because the cost of joining is small against a wait: the pair
//! is asked once for EACH agent the answer may choose, every question named
//! and keyed for its agent ([`crate::summon_model::ask_for`]), and code
//! reads only the chosen agent's. The pair stays one agent's models at one
//! agent's efforts, so no question's option count grows; what grows is the
//! state, by every other summonable agent's model list ([`PAIR_JOIN_NOTE`]);
//! [`AssignAsk::join_bytes`] says how many bytes that is.
use serde_json::{Map, Value};

use crate::summon_choice::SummonAsk;
use crate::summon_difficulty::{self, Look};
use crate::summon_model::ModelAsk;

/// The row key naming the one request several seats' rows were answered by.
pub const SHARED_REQUEST_KEY: &str = "sharedRequest";

/// What joining the pair questions costs, in words for the report and the
/// code that reads it: each agent the answer may choose adds its own model
/// list to the state, whether or not it is chosen; [`AssignAsk::join_bytes`] counts it.
pub const PAIR_JOIN_NOTE: &str =
    "each summonable agent's model list rides the one request, chosen or not";

/// The questions one request carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssignAsk {
    /// Which agent runs the work, when the summons left the agent open.
    pub agent: Option<SummonAsk>,
    /// The difficulty question, over the task it reads, when it rides.
    pub difficulty: Option<Look>,
    /// The model question of the agent the summons already has, when it
    /// rides.
    pub model: Option<ModelAsk>,
    /// While the agent is open, the model question of each agent the answer
    /// may choose ([`crate::summon_model::ask_for`]); only the chosen agent's
    /// is read.
    pub pairs: Vec<ModelAsk>,
}

impl AssignAsk {
    /// Nothing rides: no request.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seats() == 0
    }

    /// How many seats' questions ride.
    #[must_use]
    pub fn seats(&self) -> usize {
        usize::from(self.agent.is_some())
            + usize::from(self.difficulty.is_some())
            + usize::from(self.model.is_some() || !self.pairs.is_empty())
    }

    /// Two or more seats' questions ride.
    #[must_use]
    pub fn shared(&self) -> bool {
        self.seats() > 1
    }

    /// The model question of `agent`'s own that rides, whichever way it was
    /// asked.
    #[must_use]
    pub fn pair_of(&self, agent: &str) -> Option<&ModelAsk> {
        self.pairs
            .iter()
            .find(|pair| pair.scope() == Some(agent))
            .or(self.model.as_ref())
    }

    /// The one state: every riding question's keys, each once. Questions that
    /// read the same fact spell it the same (the core's test holds it), so
    /// the first to name a key names it for all.
    #[must_use]
    pub fn state(&self) -> Value {
        let mut all = Map::new();
        for part in [
            self.agent.as_ref().map(|agent| agent.state.clone()),
            self.model.as_ref().map(|model| model.state.clone()),
            self.difficulty.as_ref().map(Look::state),
        ]
        .into_iter()
        .flatten()
        .chain(self.pairs.iter().map(|pair| pair.state.clone()))
        {
            if let Value::Object(one) = part {
                for (key, value) in one {
                    all.entry(key).or_insert(value);
                }
            }
        }
        if all.is_empty() {
            Value::Null
        } else {
            Value::Object(all)
        }
    }

    /// Every riding question in one object, as the endpoint takes several.
    #[must_use]
    pub fn questions(&self) -> Value {
        let mut all = Map::new();
        for asked in [
            self.agent.as_ref().map(|agent| agent.questions.clone()),
            self.difficulty
                .as_ref()
                .map(|_| summon_difficulty::questions()),
            self.model.as_ref().map(|model| model.questions.clone()),
        ]
        .into_iter()
        .flatten()
        .chain(self.pairs.iter().map(|pair| pair.questions.clone()))
        {
            if let Value::Object(one) = asked {
                all.extend(one);
            }
        }
        Value::Object(all)
    }

    /// This ask with only the questions `agent`, `difficulty` and `model`
    /// keep.
    #[must_use]
    pub fn only(&self, agent: bool, difficulty: bool, model: bool) -> Self {
        Self {
            agent: self.agent.clone().filter(|_| agent),
            difficulty: self.difficulty.clone().filter(|_| difficulty),
            model: self.model.clone().filter(|_| model),
            pairs: if model {
                self.pairs.clone()
            } else {
                Vec::new()
            },
        }
    }

    /// The bytes joining the pair questions put in the request, over a
    /// request that asks the agent and the difficulty alone: every pair
    /// question's words and its model list.
    #[must_use]
    pub fn join_bytes(&self) -> usize {
        self.pairs
            .iter()
            .map(|pair| pair.questions.to_string().len() + pair.state.to_string().len())
            .sum()
    }
}

/// Whether an agent's pair question can be asked: the pair is a model and
/// the effort it runs at, so an agent whose command line takes no effort has
/// none to offer (t-14437). The agent question offers it all the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// The agent question and the pair question both ride for it.
    Pair,
    /// Only the agent question rides for it.
    AgentOnly,
}

impl Reach {
    /// `agent`'s reach, from the same ladder the plan reads.
    #[must_use]
    pub fn of(agent: &str) -> Self {
        if crate::orchestration::ladder_efforts(agent).is_empty() {
            Self::AgentOnly
        } else {
            Self::Pair
        }
    }

    /// Why an agent is only asked about as an agent, in the row's words.
    #[must_use]
    pub const fn why_not(self) -> Option<&'static str> {
        match self {
            Self::Pair => None,
            Self::AgentOnly => {
                Some("its command line takes no effort, so there is no pair to offer")
            }
        }
    }
}

/// Each riding seat's receipt row; `None` for a seat not asked here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Receipts {
    pub agent: Option<Value>,
    pub difficulty: Option<Value>,
    pub model: Option<Value>,
}

#[cfg(test)]
mod join_tests;
#[cfg(test)]
mod tests;
