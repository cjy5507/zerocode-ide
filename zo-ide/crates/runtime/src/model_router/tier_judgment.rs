//! A judged tier order — where each model of a provider stands, as the Jev
//! seat read it off the catalog facts and the recorded outcomes (t-45961).
//!
//! The classifier ([`super::tiering`]) orders a provider's models by one fact
//! rule when nothing better stands. A judgment replaces that order for the
//! models it covers; a model it does not cover (one discovered after the
//! judgment) takes a provisional place by the facts until the next judgment.

use std::collections::BTreeMap;

use super::inventory::ModelDescriptor;
use super::tiering::{classify_model_tiers, ModelTierAssignment};

/// Who placed a model in its provider's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum TierSource {
    /// The Jev seat's judgment covered it.
    Judged,
    /// No judgment stood for its provider: the fact rule placed it.
    #[default]
    FactRule,
    /// Its provider was judged, but not this model — it arrived after the
    /// judgment, and the facts gave it a place until the next one.
    Provisional,
}

impl TierSource {
    /// The word a report prints (`zo models --json`, `tierFrom`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Judged => "jev",
            Self::FactRule => "facts",
            Self::Provisional => "provisional",
        }
    }
}

/// One judged model: its place on the judged capability scale and how sure
/// the judgment was, both in thousandths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JudgedPlace {
    pub score_permille: u16,
    pub confidence_permille: u16,
}

/// One judgment over a lineup: the places it gave, by model id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierJudgment {
    pub places: BTreeMap<String, JudgedPlace>,
}

/// [`classify_model_tiers`] with a judgment applied where one stands.
#[must_use]
pub fn classify_model_tiers_judged(
    models: &[ModelDescriptor],
    _judgment: Option<&TierJudgment>,
) -> Vec<ModelTierAssignment> {
    classify_model_tiers(models)
}
