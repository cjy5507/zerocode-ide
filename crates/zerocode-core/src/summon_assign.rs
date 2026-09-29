//! The assign moment's one request (t-15554).
//!
//! A summons that leaves a dial open asks two seats: the difficulty of the
//! work, and — when nobody named a model — the model and the effort it runs
//! at ([`crate::summon_model`]). Both read the same state, the task's words
//! and history (the model question adds today's models beside them), so
//! they ride ONE request, the state's words sent once, behind one wall
//! ([`crate::summon_difficulty::APPLY_DEADLINE_MS`]). Each seat keeps its own
//! row, rubric and outcome; the two rows name the request they shared
//! ([`SHARED_REQUEST_KEY`]).
use serde_json::{Map, Value};

use crate::summon_difficulty::{self, Look};
use crate::summon_model::ModelAsk;

/// The row key naming the one request two seats' rows were answered by.
pub const SHARED_REQUEST_KEY: &str = "sharedRequest";

/// The questions one request carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignAsk {
    /// The difficulty question, over the task it reads, when it rides.
    pub difficulty: Option<Look>,
    /// The model question, when it rides.
    pub model: Option<ModelAsk>,
}

impl AssignAsk {
    /// Nothing rides: no request.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.difficulty.is_none() && self.model.is_none()
    }

    /// Both questions ride.
    #[must_use]
    pub fn shared(&self) -> bool {
        self.difficulty.is_some() && self.model.is_some()
    }

    /// The one state: the model question's when it rides — the difficulty's
    /// own keys with today's models beside them — else the difficulty's.
    #[must_use]
    pub fn state(&self) -> Value {
        match (&self.model, &self.difficulty) {
            (Some(model), _) => model.state.clone(),
            (None, Some(look)) => look.state(),
            (None, None) => Value::Null,
        }
    }

    /// Every riding question in one object, as the endpoint takes several.
    #[must_use]
    pub fn questions(&self) -> Value {
        let mut all = Map::new();
        for asked in [
            self.difficulty
                .as_ref()
                .map(|_| summon_difficulty::questions()),
            self.model.as_ref().map(|model| model.questions.clone()),
        ]
        .into_iter()
        .flatten()
        {
            if let Value::Object(one) = asked {
                all.extend(one);
            }
        }
        Value::Object(all)
    }

    /// This ask with only the questions `difficulty` and `model` keep.
    #[must_use]
    pub fn only(&self, difficulty: bool, model: bool) -> Self {
        Self {
            difficulty: self.difficulty.clone().filter(|_| difficulty),
            model: self.model.clone().filter(|_| model),
        }
    }
}

/// Each riding seat's receipt row; `None` for a seat not asked here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Receipts {
    pub difficulty: Option<Value>,
    pub model: Option<Value>,
}

#[cfg(test)]
mod tests;
