//! The model tier classifier — bands and implementation rungs read off the
//! connected inventory, never written down.
//!
//! The rule it serves: the most capable model of each provider plans,
//! verifies and takes the hard escalation; the next-best model of each
//! provider does the hard implementation; everything below does ordinary and
//! easy work, easy work on the cheapest model of the current release. Nothing
//! here names a model. A model's place comes from the catalog's own signals —
//! whether it reaches the Deep tier (a declared frontier class or an Ultra
//! effort ceiling), its effort ceiling, the flagship token the vendor put in
//! its name, its release recency, its declared class — so a model shipped
//! tomorrow lands in its band without an edit, and two providers' second-best
//! models are peers because they are each second, not because a list says so.

use std::collections::BTreeSet;

use super::inventory::ModelDescriptor;
use super::policy::{ModelCapability, ModelStatus, ModelTier};

/// Where a model stands within its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ModelBand {
    /// The provider's most capable model, and it reaches the Deep tier: it
    /// plans, verifies and takes the hard escalation, never ordinary
    /// implementation.
    Top,
    /// The next-best model of the provider: hard implementation.
    Second,
    /// Everything else: ordinary and easy implementation.
    #[default]
    Rest,
    /// An older release of a lineage whose newer release is connected
    /// (`claude-fable-5` beside `claude-fable-5-1`, `opus-4-8` beside
    /// `opus-5`): filtered out — it takes no band and serves no rung.
    Superseded,
}

/// The implementation rungs a task's complexity is routed through; a model
/// may serve more than one (a provider with one non-top model serves all).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImplRung {
    /// Trivial and small work: the cheapest model of the current release.
    Easy,
    /// Ordinary work: the best model below the second band, current release.
    Medium,
    /// Large work: the provider's second band.
    Hard,
}

/// One model's classification, for audit and display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTierAssignment {
    pub id: String,
    pub provider: String,
    /// 1-based capability rank within the provider.
    pub rank: usize,
    pub band: ModelBand,
    pub rungs: BTreeSet<ImplRung>,
}

/// The capability order within a provider, most capable first. Every term is
/// a catalog fact carried on the descriptor; the vendor's flagship token is
/// the one name-derived term and it comes from the catalog's prior table.
fn capability_key(model: &ModelDescriptor) -> impl Ord {
    let class_rank = match model.class_label() {
        Some("frontier") => 3_u8,
        Some("balanced") => 2,
        Some("fast") => 0,
        _ => 1,
    };
    (
        model.has_tier(ModelTier::Deep),
        model.effort_ceiling_value(),
        has_flagship_token(model.id()),
        model.release_rank_value(),
        class_rank,
    )
}

fn has_flagship_token(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    api::router_priors()
        .deep_flagship_tokens
        .iter()
        .any(|token| lower.contains(token.as_str()))
}

fn usable(model: &ModelDescriptor) -> bool {
    !matches!(model.status_value(), ModelStatus::Deprecated | ModelStatus::Retired)
}

/// The lineage a release belongs to — the vendor's codename (`fable`, `opus`,
/// `sol`, `flash`) read off the id — so releases of one line compete for one
/// place and only the newest of them stands.
fn lineage(model: &ModelDescriptor) -> String {
    api::family_from_id(api::detect_provider_kind(model.id()), model.id())
        .unwrap_or_else(|| api::resolve_catalog_alias(model.id()))
        .to_ascii_lowercase()
}

/// Classify every usable model of `models` by provider. The result is in
/// input order so a caller can zip it back onto the descriptors.
#[must_use]
pub fn classify_model_tiers(models: &[ModelDescriptor]) -> Vec<ModelTierAssignment> {
    let mut providers: Vec<&str> = Vec::new();
    for model in models.iter().filter(|model| usable(model)) {
        if !providers.contains(&model.provider()) {
            providers.push(model.provider());
        }
    }
    let mut assignments: Vec<ModelTierAssignment> = Vec::with_capacity(models.len());
    for provider in providers {
        // One place per lineage: of the releases of one line the newest
        // stands (release recency, then the capability key) and the older
        // ones are superseded — an old model is filtered out, never a
        // second-band twin of its own successor.
        let mut candidates: Vec<&ModelDescriptor> = models
            .iter()
            .filter(|model| usable(model) && model.provider() == provider)
            .collect();
        candidates.sort_by(|left, right| {
            (right.release_rank_value(), capability_key(right)).cmp(&(left.release_rank_value(), capability_key(left)))
        });
        let mut ordered: Vec<&ModelDescriptor> = Vec::new();
        let mut superseded: Vec<&ModelDescriptor> = Vec::new();
        for model in candidates {
            if ordered.iter().any(|kept| lineage(kept) == lineage(model)) {
                superseded.push(model);
            } else {
                ordered.push(model);
            }
        }
        ordered.sort_by_key(|model| std::cmp::Reverse(capability_key(model)));
        let top_is_deep = ordered.first().is_some_and(|best| best.has_tier(ModelTier::Deep));
        let bands: Vec<ModelBand> = ordered
            .iter()
            .enumerate()
            .map(|(index, _)| match (index, top_is_deep) {
                (0, true) => ModelBand::Top,
                (0, false) | (1, true) => ModelBand::Second,
                _ => ModelBand::Rest,
            })
            .collect();
        let rungs = implementation_rungs(&ordered, &bands);
        for ((rank, model), (band, rungs)) in ordered.iter().enumerate().zip(bands.into_iter().zip(rungs)) {
            assignments.push(ModelTierAssignment {
                id: model.id().to_string(),
                provider: provider.to_string(),
                rank: rank + 1,
                band,
                rungs,
            });
        }
        for model in superseded {
            assignments.push(ModelTierAssignment {
                id: model.id().to_string(),
                provider: provider.to_string(),
                rank: ordered.len() + 1,
                band: ModelBand::Superseded,
                rungs: BTreeSet::new(),
            });
        }
    }
    let mut in_input_order = Vec::with_capacity(assignments.len());
    for model in models {
        if let Some(position) = assignments.iter().position(|entry| entry.id == model.id()) {
            in_input_order.push(assignments.remove(position));
        }
    }
    in_input_order
}

/// Rungs for one provider's ladder (`ordered` most capable first, `bands`
/// aligned): hard is the second band; the current release is the newest
/// release among the non-top implementers; medium is the best model of the
/// rest band on that release, easy the least capable non-top model on it.
/// A rung with no distinct model falls onto the rung above it, so a provider
/// with one implementer serves every rung with it.
fn implementation_rungs(ordered: &[&ModelDescriptor], bands: &[ModelBand]) -> Vec<BTreeSet<ImplRung>> {
    let implementers: Vec<usize> = ordered
        .iter()
        .enumerate()
        .filter(|(index, model)| bands[*index] != ModelBand::Top && model.has_capability(ModelCapability::Coding))
        .map(|(index, _)| index)
        .collect();
    let mut rungs: Vec<BTreeSet<ImplRung>> = vec![BTreeSet::new(); ordered.len()];
    let Some(&hard) = implementers.first() else {
        return rungs;
    };
    let current_release = implementers
        .iter()
        .map(|&index| ordered[index].release_rank_value())
        .max()
        .unwrap_or(0);
    let on_current = |index: &&usize| ordered[**index].release_rank_value() == current_release;
    let medium = implementers
        .iter()
        .filter(|index| bands[**index] == ModelBand::Rest)
        .find(on_current)
        .copied()
        .unwrap_or(hard);
    let easy = implementers.iter().rev().find(on_current).copied().unwrap_or(hard);
    rungs[hard].insert(ImplRung::Hard);
    rungs[medium].insert(ImplRung::Medium);
    rungs[easy].insert(ImplRung::Easy);
    rungs
}

/// Stamp each descriptor with its band and rungs. Called once when an
/// inventory is built, so every selector reads the same classification.
pub(super) fn stamp_bands_and_rungs(models: &mut [ModelDescriptor]) {
    let assignments = classify_model_tiers(models);
    for model in models.iter_mut() {
        match assignments.iter().find(|entry| entry.id == model.id()) {
            Some(entry) => model.set_band_and_rungs(entry.band, entry.rungs.clone()),
            None => model.set_band_and_rungs(ModelBand::Rest, BTreeSet::new()),
        }
    }
}
