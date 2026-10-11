//! A judged tier order — where each model of a provider stands, as the Jev
//! seat read it off the catalog facts and the recorded outcomes (t-45961).
//!
//! The classifier ([`super::tiering`]) orders a provider's models by one fact
//! rule when nothing better stands. A judgment replaces that order for the
//! models it covers; a model it does not cover (one discovered after the
//! judgment) takes a provisional place by the facts until the next judgment.

use std::collections::BTreeMap;

use super::inventory::ModelDescriptor;
use super::policy::ModelTier;
use super::tiering::{classify_model_tiers_with, FactRule, ModelTierAssignment};

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
    judgment: Option<&TierJudgment>,
) -> Vec<ModelTierAssignment> {
    classify_model_tiers_with(models, judgment)
}

/// One model's place in its provider's order, and who gave it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Place<'a> {
    pub model: &'a ModelDescriptor,
    pub source: TierSource,
    pub confidence_permille: Option<u16>,
}

/// One provider's models (`by_facts`, already in the fact rule's order) in
/// the order a judgment gives them.
///
/// - A provider the judgment covers none of keeps the fact order.
/// - The head of the fact order stays at the head when it reaches the Deep
///   tier: planning and verifying stand on that declaration, and the
///   judgment orders the implementers below it.
/// - The judged implementers stand by their judged score, the higher first,
///   a tie kept in the fact order.
/// - A model the judgment does not cover — discovered after it — is placed
///   by the facts: just above the first model already placed that the fact
///   rule ranks below it, else last. It is provisional until a judgment
///   covers it.
pub(super) fn judged_order<'a>(
    by_facts: Vec<&'a ModelDescriptor>,
    rule: &FactRule,
    judgment: Option<&TierJudgment>,
) -> Vec<Place<'a>> {
    let judged = |model: &ModelDescriptor| judgment.and_then(|judgment| judgment.places.get(model.id()).copied());
    if !by_facts.iter().any(|model| judged(model).is_some()) {
        return by_facts
            .into_iter()
            .map(|model| Place { model, source: TierSource::FactRule, confidence_permille: None })
            .collect();
    }
    let place = |model: &'a ModelDescriptor| match judged(model) {
        Some(found) => Place { model, source: TierSource::Judged, confidence_permille: Some(found.confidence_permille) },
        None => Place { model, source: TierSource::Provisional, confidence_permille: None },
    };
    let mut order: Vec<Place<'a>> = Vec::new();
    let mut implementers: Vec<&'a ModelDescriptor> = Vec::new();
    for (index, model) in by_facts.into_iter().enumerate() {
        if index == 0 && model.has_tier(ModelTier::Deep) {
            order.push(place(model));
        } else {
            implementers.push(model);
        }
    }
    let (mut covered, provisional): (Vec<&ModelDescriptor>, Vec<&ModelDescriptor>) =
        implementers.into_iter().partition(|model| judged(model).is_some());
    covered.sort_by_key(|model| std::cmp::Reverse(judged(model).map_or(0, |found| found.score_permille)));
    let mut below: Vec<Place<'a>> = covered.into_iter().map(place).collect();
    for model in provisional {
        let key = rule.key(model);
        let at = below.iter().position(|placed| key > rule.key(placed.model)).unwrap_or(below.len());
        below.insert(at, place(model));
    }
    order.extend(below);
    order
}
