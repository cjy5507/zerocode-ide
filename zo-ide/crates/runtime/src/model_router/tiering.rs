//! The model tier classifier — bands and implementation rungs read off the
//! connected inventory, never written down.
//!
//! The rule it serves: the most capable model of each provider plans,
//! verifies and takes the hard escalation; the next-best model of each
//! provider does the hard implementation; everything below does ordinary and
//! easy work, easy work on the least capable model that is still current. A
//! provider with one current implementer below its top leaves the hard work
//! to the top, so no provider's hard work lands on its only light model.
//! Nothing here names a model. A model's place comes from one of two
//! sources: the Jev seat's judgment of the provider's lineup, read off the
//! catalog facts and the recorded outcomes ([`super::tier_judgment`]), or —
//! where no judgment stands — one rule over the catalog's own facts
//! ([`FactRule`]): the provider's declared class, then the list price,
//! then the context window, then the release order, then the effort ceiling.
//! A model shipped tomorrow lands in its band without an edit, and two
//! providers' second-best models are peers because they are each second, not
//! because a list says so (t-45961: the rule read a marketing label before,
//! and a fast model whose name the label table did not know outranked the
//! provider's balanced one).

use std::collections::BTreeSet;

use super::inventory::{ModelDescriptor, RELEASE_RANKS_PER_MAJOR};
use super::policy::{ModelCapability, ModelStatus, ModelTier};
use super::tier_judgment::{judged_order, TierJudgment, TierSource};

#[cfg(test)]
mod tests;

/// Where a model stands within its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ModelBand {
    /// The provider's most capable model, and it reaches the Deep tier: it
    /// plans, verifies and takes the hard escalation, never ordinary
    /// implementation — and the hard rung as well when one implementer below
    /// it would serve every rung.
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

impl ModelBand {
    /// The word a report prints for the band (`zo models --json`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Second => "second",
            Self::Rest => "rest",
            Self::Superseded => "superseded",
        }
    }
}

/// The implementation rungs a task's complexity is routed through; a model
/// may serve more than one (a provider with one non-top model serves all).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImplRung {
    /// Trivial and small work: the cheapest model of the current release.
    Easy,
    /// Ordinary work: the best model below the second band, current release.
    Medium,
    /// Large work: the provider's second band, or its top model when the
    /// second band is its one current implementer.
    Hard,
}

impl ImplRung {
    /// The word a report prints for the rung (`zo models --json`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }
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
    /// Who placed it: the Jev seat's judgment, the fact rule, or the facts
    /// for a model that arrived after its provider's judgment.
    pub source: TierSource,
    /// How sure the judgment was, in thousandths; `None` when no judgment
    /// placed it.
    pub confidence_permille: Option<u16>,
    /// For a [`ModelBand::Superseded`] release, the newer release of its own
    /// line that stands in its place (`claude-fable-5` → `claude-fable-5-1`)
    /// — what a session still running the older one is moved to (t-46882).
    /// `None` for every other band, and for a twin of the same release
    /// (`acme[blue]` beside `acme[green]`): a twin is no older, the same rule
    /// as `policy::automatic_choice_allowed`.
    pub successor: Option<String>,
}

/// A declared class as a rank in [`FactRule`], strongest first. The
/// class is read off the tier set the catalog gave the model — the
/// provider's own declaration where there is one, else the labelled
/// cold-start derivation (an Ultra ceiling, a size token, a flagship token)
/// — so a name-derived prior is one fact among the others, never a term of
/// its own.
const CLASS_FRONTIER: u8 = 3;
/// A provider's everyday model.
const CLASS_BALANCED: u8 = 2;
/// Nothing declared and nothing derived: a custom endpoint zo knows nothing
/// about stands between the balanced and the fast class.
const CLASS_UNDECLARED: u8 = 1;
/// A provider's fast, affordable model.
const CLASS_FAST: u8 = 0;

/// The word the tier seat's state names a class by ([`class_rank`]).
pub(super) fn class_word(model: &ModelDescriptor) -> &'static str {
    match class_rank(model) {
        CLASS_FRONTIER => "frontier",
        CLASS_BALANCED => "balanced",
        CLASS_FAST => "fast",
        _ => "undeclared",
    }
}

fn class_rank(model: &ModelDescriptor) -> u8 {
    if model.has_tier(ModelTier::Deep) {
        CLASS_FRONTIER
    } else if model.has_tier(ModelTier::Fast) {
        CLASS_FAST
    } else if model.has_tier(ModelTier::Strong) {
        CLASS_BALANCED
    } else {
        CLASS_UNDECLARED
    }
}

/// The fact rule: the capability order within one provider's lineup when no
/// judgment stands, compared most capable first. Each term is a catalog
/// fact, and an earlier term always outweighs every later one:
///
/// 1. the declared class ([`class_rank`]);
/// 2. the list price, input plus output — a vendor prices its stronger
///    model higher;
/// 3. the context window;
/// 4. the release order — a newer model, not a stronger one, so it never
///    outweighs the class;
/// 5. the effort ceiling.
///
/// A fact some model of a class lacks (a release the price table does not
/// name yet) is compared within that class by nobody: one missing price must
/// not sink a new flagship below its priced elders, and comparing it only
/// where both sides have it would not be an order at all (a > b > c > a).
pub(super) struct FactRule {
    /// The classes every model of which carries a list price.
    priced: BTreeSet<u8>,
    /// The classes every model of which carries a context window.
    windowed: BTreeSet<u8>,
}

impl FactRule {
    /// The rule for the lineup `models` (one provider's).
    pub(super) fn of(models: &[&ModelDescriptor]) -> Self {
        let every = |has: fn(&ModelDescriptor) -> bool| -> BTreeSet<u8> {
            let classes: BTreeSet<u8> = models.iter().map(|model| class_rank(model)).collect();
            classes
                .into_iter()
                .filter(|class| models.iter().filter(|model| class_rank(model) == *class).all(|model| has(model)))
                .collect()
        };
        Self {
            priced: every(|model| model.list_price_value().is_some()),
            windowed: every(|model| model.context_window_value().is_some()),
        }
    }

    /// `model`'s place under the rule; the greater key is the more capable.
    pub(super) fn key(&self, model: &ModelDescriptor) -> impl Ord {
        let class = class_rank(model);
        (
            class,
            model
                .list_price_value()
                .filter(|_| self.priced.contains(&class))
                .map(|price| price.input_micro_usd.saturating_add(price.output_micro_usd)),
            model.context_window_value().filter(|_| self.windowed.contains(&class)),
            model.release_rank_value(),
            model.effort_ceiling_value(),
        )
    }
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

/// Whether `left` and `right` are releases of one line of one provider — the
/// pair the classifier keeps one place for, the newest release standing.
pub(super) fn same_line(left: &ModelDescriptor, right: &ModelDescriptor) -> bool {
    left.provider() == right.provider() && lineage(left) == lineage(right)
}

/// Classify every usable model of `models` by provider, by the fact rule.
/// The result is in input order so a caller can zip it back onto the
/// descriptors.
#[must_use]
pub fn classify_model_tiers(models: &[ModelDescriptor]) -> Vec<ModelTierAssignment> {
    classify_model_tiers_with(models, None)
}

/// [`classify_model_tiers`] with a judgment applied to every provider it
/// covers ([`super::tier_judgment::judged_order`]); `None` is the fact rule
/// throughout.
#[must_use]
pub(super) fn classify_model_tiers_with(
    models: &[ModelDescriptor],
    judgment: Option<&TierJudgment>,
) -> Vec<ModelTierAssignment> {
    let mut providers: Vec<&str> = Vec::new();
    for model in models.iter().filter(|model| usable(model)) {
        if !providers.contains(&model.provider()) {
            providers.push(model.provider());
        }
    }
    let mut assignments: Vec<ModelTierAssignment> = Vec::with_capacity(models.len());
    for provider in providers {
        // One place per lineage: of the releases of one line the newest
        // stands (release recency, then the fact rule) and the older ones are
        // superseded — an old model is filtered out, never a second-band twin
        // of its own successor.
        let mut candidates: Vec<&ModelDescriptor> = models
            .iter()
            .filter(|model| usable(model) && model.provider() == provider)
            .collect();
        let rule = FactRule::of(&candidates);
        candidates.sort_by(|left, right| {
            (right.release_rank_value(), rule.key(right)).cmp(&(left.release_rank_value(), rule.key(left)))
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
        ordered.sort_by_key(|model| std::cmp::Reverse(rule.key(model)));
        let placed = judged_order(ordered, &rule, judgment);
        let ordered: Vec<&ModelDescriptor> = placed.iter().map(|place| place.model).collect();
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
        for ((rank, place), (band, rungs)) in placed.iter().enumerate().zip(bands.into_iter().zip(rungs)) {
            assignments.push(ModelTierAssignment {
                id: place.model.id().to_string(),
                provider: provider.to_string(),
                rank: rank + 1,
                band,
                rungs,
                source: place.source,
                confidence_permille: place.confidence_permille,
                successor: None,
            });
        }
        for model in superseded {
            // The release its line keeps in its place — only one that is
            // newer: a twin of the same release folds here too, and is no
            // older than the one that stands.
            let successor = ordered
                .iter()
                .find(|kept| lineage(kept) == lineage(model))
                .filter(|kept| kept.release_rank_value() > model.release_rank_value())
                .map(|kept| kept.id().to_string());
            assignments.push(ModelTierAssignment {
                id: model.id().to_string(),
                provider: provider.to_string(),
                rank: ordered.len() + 1,
                band: ModelBand::Superseded,
                rungs: BTreeSet::new(),
                source: TierSource::FactRule,
                confidence_permille: None,
                successor,
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
/// aligned): hard is the second band; medium is the best current model of
/// the rest band, easy the least capable current implementer. A model is
/// current unless an implementer of its own class is a newer release, or one
/// of a stronger class is a newer generation (major version) — so an older
/// line (`haiku-4-5` beside `sonnet-5`, `flash-lite` 3.1 beside `flash` 3.8)
/// serves no rung, a point release of a stronger line (6.1 beside 6) ages
/// nobody, and a newer release of a weaker class never pushes a stronger
/// class off the ladder. A rung with no distinct model falls onto the rung
/// above it, so a provider with one implementer would serve every rung with
/// it — and there the top band, when it can code, takes the hard rung, so a
/// provider's hard work never falls to its one light model (t-45961, the
/// person and ZO, 10-11: Google's Pro became the top band once a size word
/// stopped matching inside `gemini`, and its one current Flash took hard
/// work from it).
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
    let generation = |model: &ModelDescriptor| model.release_rank_value() / RELEASE_RANKS_PER_MAJOR;
    let current = |index: &&usize| {
        let model = ordered[**index];
        !implementers.iter().map(|&other| ordered[other]).any(|other| {
            let (theirs, mine) = (class_rank(other), class_rank(model));
            (theirs == mine && other.release_rank_value() > model.release_rank_value())
                || (theirs > mine && generation(other) > generation(model))
        })
    };
    let medium = implementers
        .iter()
        .filter(|index| bands[**index] == ModelBand::Rest)
        .find(current)
        .copied()
        .unwrap_or(hard);
    let easy = implementers.iter().rev().find(current).copied().unwrap_or(hard);
    let hard = bands
        .iter()
        .position(|band| *band == ModelBand::Top)
        .filter(|&top| ordered[top].has_capability(ModelCapability::Coding) && hard == medium && medium == easy)
        .unwrap_or(hard);
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
