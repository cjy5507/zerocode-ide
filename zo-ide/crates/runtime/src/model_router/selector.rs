use super::inventory::{ModelDescriptor, ModelInventory};
use super::policy::{freshness_allows, FreshnessPolicy, ModelCapability, ModelTier};
use super::tiering::{ImplRung, ModelBand};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoleSelector {
    pub(crate) provider: Option<String>,
    pub(crate) family: Option<String>,
    pub(crate) class: Option<String>,
    pub(crate) capability: Option<ModelCapability>,
    pub(crate) tier: Option<ModelTier>,
    /// An implementation rung of the provider ladder (see `tiering`); a
    /// selector with a rung matches the models the classifier stamped for it.
    pub(crate) impl_rung: Option<ImplRung>,
    /// A band of the provider ladder (see `tiering`): `Top` is the checker
    /// bench, reached by implementation only through escalation.
    pub(crate) band: Option<ModelBand>,
    pub(crate) freshness: FreshnessPolicy,
}

impl RoleSelector {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn provider(mut self, provider: impl Into<String>) -> Self { self.provider = Some(provider.into()); self }

    #[must_use]
    pub fn family(mut self, family: impl Into<String>) -> Self { self.family = Some(family.into()); self }

    #[must_use]
    pub fn class(mut self, class: impl Into<String>) -> Self { self.class = Some(class.into()); self }

    #[must_use]
    pub fn capability(mut self, capability: ModelCapability) -> Self { self.capability = Some(capability); self }

    #[must_use]
    pub fn tier(mut self, tier: ModelTier) -> Self { self.tier = Some(tier); self }

    #[must_use]
    pub fn impl_rung(mut self, rung: ImplRung) -> Self { self.impl_rung = Some(rung); self }

    #[must_use]
    pub fn band(mut self, band: ModelBand) -> Self { self.band = Some(band); self }

    #[must_use]
    pub fn freshness(mut self, freshness: FreshnessPolicy) -> Self { self.freshness = freshness; self }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleOverride { Auto, Pin(String), Family(RoleSelector) }

pub(crate) fn select_model<'a>(
    selector: &RoleSelector,
    inventory: &'a ModelInventory,
    allowed: impl Fn(&ModelDescriptor) -> bool,
) -> Option<&'a ModelDescriptor> {
    inventory
        .models()
        .iter()
        .filter(|model| selector_matches(model, selector) && allowed(model))
        .max_by_key(|model| score_model(model, selector))
}

fn selector_matches(model: &ModelDescriptor, selector: &RoleSelector) -> bool {
    selector.provider.as_ref().is_none_or(|provider| model.provider() == provider)
        && selector.family.as_ref().is_none_or(|family| model.family() == family)
        && selector.class.as_ref().is_none_or(|class| model.class_matches(class))
        && selector.capability.is_none_or(|capability| model.has_capability(capability))
        && selector.tier.is_none_or(|tier| model.has_tier(tier))
        && selector.impl_rung.is_none_or(|rung| model.serves_rung(rung))
        && selector.band.is_none_or(|band| model.band() == band)
        && freshness_allows(model.status_value(), selector.freshness)
}

fn score_model(model: &ModelDescriptor, selector: &RoleSelector) -> (u32, u32) {
    let mut score = 0;
    if selector.capability.is_some_and(|capability| model.has_capability(capability)) { score += 100; }
    if selector.tier.is_some_and(|tier| model.has_tier(tier)) { score += 50; }
    if selector.impl_rung.is_some_and(|rung| model.serves_rung(rung)) { score += 50; }
    if selector.band.is_some_and(|band| model.band() == band) { score += 50; }
    if selector.class.as_ref().is_some_and(|class| model.class_matches(class)) { score += 25; }
    (score, model.release_rank_value())
}
