use std::collections::BTreeSet;

use super::policy::{EffortCeiling, ModelCapability, ModelStatus, ModelTier, TiersProvenance};
use super::tiering::{ImplRung, ModelBand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelSource {
    CurrentMainModel,
    EnabledBuiltinProvider,
    CustomProvider,
    LocalProvider,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    id: String,
    provider: String,
    family: String,
    class: Option<String>,
    source: ModelSource,
    capabilities: BTreeSet<ModelCapability>,
    tiers: BTreeSet<ModelTier>,
    status: ModelStatus,
    release_rank: u32,
    effort_ceiling: EffortCeiling,
    context_window: Option<u32>,
    tiers_provenance: TiersProvenance,
    /// Stamped by the tier classifier when the inventory is built.
    band: ModelBand,
    rungs: BTreeSet<ImplRung>,
}

pub type UsableModel = ModelDescriptor;

impl ModelDescriptor {
    #[must_use]
    pub fn new(id: impl Into<String>, provider: impl Into<String>, family: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            family: family.into(),
            class: None,
            source: ModelSource::CustomProvider,
            capabilities: BTreeSet::new(),
            tiers: BTreeSet::new(),
            status: ModelStatus::Stable,
            release_rank: 0,
            effort_ceiling: EffortCeiling::default(),
            context_window: None,
            tiers_provenance: TiersProvenance::default(),
            band: ModelBand::default(),
            rungs: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.class = Some(class.into());
        self
    }

    #[must_use]
    pub fn source(mut self, source: ModelSource) -> Self {
        self.source = source;
        self
    }

    #[must_use]
    pub fn capabilities(mut self, capabilities: impl IntoIterator<Item = ModelCapability>) -> Self {
        self.capabilities.extend(capabilities);
        self
    }

    #[must_use]
    pub fn tiers(mut self, tiers: impl IntoIterator<Item = ModelTier>) -> Self {
        self.tiers.extend(tiers);
        self
    }

    #[must_use]
    pub fn status(mut self, status: ModelStatus) -> Self {
        self.status = status;
        self
    }

    #[must_use]
    pub fn release_rank(mut self, release_rank: u32) -> Self {
        self.release_rank = release_rank;
        self
    }

    #[must_use]
    pub fn effort_ceiling(mut self, effort_ceiling: EffortCeiling) -> Self {
        self.effort_ceiling = effort_ceiling;
        self
    }

    #[must_use]
    pub fn context_window(mut self, context_window: u32) -> Self {
        self.context_window = Some(context_window);
        self
    }

    #[must_use]
    pub fn tiers_provenance(mut self, tiers_provenance: TiersProvenance) -> Self {
        self.tiers_provenance = tiers_provenance;
        self
    }

    #[must_use]
    pub fn id(&self) -> &str { &self.id }

    #[must_use]
    pub fn provider(&self) -> &str { &self.provider }

    #[must_use]
    pub fn family(&self) -> &str { &self.family }

    #[must_use]
    pub fn class_label(&self) -> Option<&str> { self.class.as_deref() }

    #[must_use]
    pub fn source_value(&self) -> ModelSource { self.source }

    #[must_use]
    pub fn status_value(&self) -> ModelStatus { self.status }

    pub(crate) fn has_capability(&self, capability: ModelCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub(crate) fn has_tier(&self, tier: ModelTier) -> bool {
        self.tiers.contains(&tier)
    }

    /// Highest tier this model declares (`ModelTier`'s derived `Ord`:
    /// Fast < Balanced < Strong < Deep), or `None` for a tierless descriptor.
    /// Anchors `policy`'s one-rung-up learned-promotion rule.
    pub(crate) fn strongest_tier(&self) -> Option<ModelTier> {
        self.tiers.iter().copied().max()
    }

    pub(crate) fn class_matches(&self, class: &str) -> bool {
        self.class.as_deref() == Some(class)
    }

    pub(crate) fn release_rank_value(&self) -> u32 { self.release_rank }

    #[must_use]
    pub fn effort_ceiling_value(&self) -> EffortCeiling { self.effort_ceiling }

    /// Declared context window in tokens, when known. `None` for descriptors
    /// built without capability data (e.g. test fixtures, the genuinely
    /// unknown-family main-model fallback).
    #[must_use]
    pub fn context_window_value(&self) -> Option<u32> { self.context_window }

    #[must_use]
    pub fn tiers_provenance_value(&self) -> TiersProvenance { self.tiers_provenance }
    /// The band the tier classifier placed this model in within its provider.
    #[must_use]
    pub fn band(&self) -> ModelBand { self.band }
    /// The implementation rungs this model serves (empty for the top band).
    #[must_use]
    pub fn rungs(&self) -> &BTreeSet<ImplRung> { &self.rungs }
    #[must_use]
    pub fn serves_rung(&self, rung: ImplRung) -> bool { self.rungs.contains(&rung) }
    pub(super) fn set_band_and_rungs(&mut self, band: ModelBand, rungs: BTreeSet<ImplRung>) {
        self.band = band;
        self.rungs = rungs;
    }
}

/// `true` when `candidate_id` is `known_id` itself, or `known_id` followed by
/// a segment boundary (`-`, `@`, `[`) — the same dated/explicit-provider/
/// service-tier suffix shape `api::types::model_id_matches_family` uses on the
/// provider side. Duplicated here (rather than imported) so `model_router`
/// stays free of a dependency on the `api` crate; it is a generic string
/// predicate, not provider-specific logic, so the small duplication is a
/// worthwhile trade for keeping the router core's zero-api-dependency
/// invariant intact.
fn shares_known_family_prefix(candidate_id: &str, known_id: &str) -> bool {
    candidate_id == known_id
        || candidate_id
            .strip_prefix(known_id)
            .is_some_and(|suffix| matches!(suffix.as_bytes().first(), Some(b'-' | b'@' | b'[')))
}

/// Resolve a main-model id that has no exact catalog entry (a dated pin such
/// as `gpt-5.6-sol-2026-07-09`, or an `@`/`[`-suffixed variant) against the
/// already-classified descriptors in `known`, so it inherits the real
/// family/provider/capabilities/tiers/effort-ceiling/context-window instead of
/// degrading to the zero-capability `unknown`/`custom` fallback. Longest
/// matching known id wins (most specific family). Returns `None` for a
/// genuinely unknown id — one that shares no known family prefix — which
/// still degrades exactly as before.
fn inherit_known_family_descriptor(main_model: &str, known: &[ModelDescriptor]) -> Option<ModelDescriptor> {
    let best = known
        .iter()
        .filter(|candidate| shares_known_family_prefix(main_model, &candidate.id))
        .max_by_key(|candidate| candidate.id.len())?;
    Some(ModelDescriptor {
        id: main_model.to_string(),
        provider: best.provider.clone(),
        family: best.family.clone(),
        class: best.class.clone(),
        source: best.source,
        capabilities: best.capabilities.clone(),
        tiers: best.tiers.clone(),
        status: best.status,
        release_rank: best.release_rank,
        effort_ceiling: best.effort_ceiling,
        context_window: best.context_window,
        tiers_provenance: best.tiers_provenance,
        band: ModelBand::default(),
        rungs: BTreeSet::new(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsableModelInventory {
    main_model: String,
    models: Vec<ModelDescriptor>,
}

pub type ModelInventory = UsableModelInventory;

impl UsableModelInventory {
    #[must_use]
    pub fn new(main_model: impl Into<String>, models: Vec<ModelDescriptor>) -> Self {
        let main_model = main_model.into();
        let mut seen = BTreeSet::new();
        let mut deduped = Vec::new();
        for model in models {
            if seen.insert(model.id.clone()) {
                deduped.push(model);
            }
        }
        if !seen.contains(&main_model) {
            let fallback = inherit_known_family_descriptor(&main_model, &deduped).unwrap_or_else(|| {
                ModelDescriptor::new(main_model.clone(), "unknown", "custom")
            });
            deduped.push(fallback.source(ModelSource::CurrentMainModel));
        }
        // One classification per inventory: every selector and gate reads the
        // same bands and rungs.
        super::tiering::stamp_bands_and_rungs(&mut deduped);
        Self { main_model, models: deduped }
    }

    #[must_use]
    pub fn main_model(&self) -> &str { &self.main_model }

    #[must_use]
    pub fn models(&self) -> &[ModelDescriptor] { &self.models }

    #[must_use]
    pub fn find(&self, model_id: &str) -> Option<&ModelDescriptor> {
        self.models.iter().find(|candidate| candidate.id == model_id)
    }
}
