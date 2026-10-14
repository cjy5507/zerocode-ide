//! The tier classifier read off catalog facts alone (t-45961).
//!
//! Every lineup here is one made-up provider whose models carry no vendor
//! word — `a`, `b`, `c`, `d` — so nothing but the facts on each descriptor
//! can place a model. A test that passed only because an id spelled `opus`
//! or `haiku` would prove nothing about a model shipped tomorrow.

use super::*;
use crate::model_router::policy::{EffortCeiling, ModelCapability, ModelTier, TiersProvenance};
use crate::model_router::tier_judgment::{classify_model_tiers_judged, JudgedPlace, TierJudgment};
use crate::model_router::{ModelInventory, ModelSource};

const FRONTIER: [ModelTier; 2] = [ModelTier::Deep, ModelTier::Strong];
const BALANCED: [ModelTier; 2] = [ModelTier::Balanced, ModelTier::Strong];
const FAST: [ModelTier; 2] = [ModelTier::Fast, ModelTier::Balanced];

/// One catalog row with only facts on it: the provider's declared class (as
/// the tier set it maps to), the release order and the effort ceiling.
fn row(id: &str, provider: &str, class: [ModelTier; 2], release: u32) -> ModelDescriptor {
    ModelDescriptor::new(id, provider, id)
        .source(ModelSource::EnabledBuiltinProvider)
        .capabilities([ModelCapability::Default, ModelCapability::ToolUse, ModelCapability::Coding])
        .tiers(class)
        .tiers_provenance(TiersProvenance::ProviderDeclared)
        .effort_ceiling(EffortCeiling::Max)
        .release_rank(release)
}

fn placed<'a>(assignments: &'a [ModelTierAssignment], id: &str) -> &'a ModelTierAssignment {
    assignments
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("{id} is classified: {assignments:?}"))
}

fn rungs(assignments: &[ModelTierAssignment], id: &str) -> Vec<ImplRung> {
    placed(assignments, id).rungs.iter().copied().collect()
}

/// The defect the person saw (10-11): a fast model released after a
/// balanced one stood above it, took the hard rung and left the balanced
/// model nothing. A later release is a newer model, not a stronger one.
#[test]
fn a_later_fast_release_never_stands_above_a_balanced_model() {
    let models = [row("a", "p", FRONTIER, 1), row("b", "p", BALANCED, 1), row("c", "p", FAST, 2)];
    let tiers = classify_model_tiers(&models);
    assert_eq!(placed(&tiers, "a").band, ModelBand::Top);
    assert_eq!(placed(&tiers, "b").band, ModelBand::Second, "{tiers:?}");
    assert_eq!(placed(&tiers, "c").band, ModelBand::Rest, "{tiers:?}");
    assert!(placed(&tiers, "b").rank < placed(&tiers, "c").rank, "{tiers:?}");
    assert_eq!(rungs(&tiers, "b"), vec![ImplRung::Hard], "{tiers:?}");
    assert_eq!(rungs(&tiers, "c"), vec![ImplRung::Easy, ImplRung::Medium], "{tiers:?}");
}

/// Four implementers' worth of lineup: the ordinary rung is the better of
/// the two balanced models below the second band, the easy rung the fast
/// one — even though the fast one is the newest release of the provider.
#[test]
fn the_rungs_follow_the_fact_order_not_the_newest_release() {
    let models = [
        row("d", "p", FAST, 2),
        row("c", "p", BALANCED, 1).list_price_micro_usd(3_000_000, 15_000_000),
        row("b", "p", BALANCED, 1).list_price_micro_usd(5_000_000, 25_000_000),
        row("a", "p", FRONTIER, 1).list_price_micro_usd(10_000_000, 50_000_000),
    ];
    let tiers = classify_model_tiers(&models);
    assert_eq!(placed(&tiers, "a").band, ModelBand::Top, "{tiers:?}");
    assert_eq!(rungs(&tiers, "b"), vec![ImplRung::Hard], "{tiers:?}");
    assert_eq!(rungs(&tiers, "c"), vec![ImplRung::Medium], "{tiers:?}");
    assert_eq!(rungs(&tiers, "d"), vec![ImplRung::Easy], "{tiers:?}");
}

/// A point release of a stronger line ages nobody — a fast model of the
/// same generation keeps its rungs beside a frontier `.1` — while a newer
/// generation of a stronger class does (t-45961: `gpt-6-luna` lost its
/// rungs to `gpt-6.1-sol` on this machine's lineup).
#[test]
fn a_point_release_ages_nobody_and_a_new_generation_does() {
    let models = [row("a", "p", FRONTIER, 60), row("b", "p", FRONTIER, 61), row("c", "p", FAST, 60)];
    let tiers = classify_model_tiers(&models);
    assert_eq!(rungs(&tiers, "c"), vec![ImplRung::Easy, ImplRung::Medium], "{tiers:?}");
    let older = [row("a", "p", FRONTIER, 60), row("b", "p", FRONTIER, 61), row("c", "p", FAST, 50)];
    let tiers = classify_model_tiers(&older);
    assert!(rungs(&tiers, "c").is_empty(), "a past generation serves no rung: {tiers:?}");
}

/// The one rule that stands when no judgment does: declared class, then the
/// list price, then the context window, then the release order, then the
/// effort ceiling. Each provider below is a pair that differs in one fact
/// and loses in every later one, and the weaker model is listed first so a
/// tie (a fact the rule ignores) keeps the wrong order and fails.
#[test]
fn the_fact_rule_reads_class_then_price_then_context_then_release_then_effort() {
    let pair = |provider: &str, weaker: ModelDescriptor, stronger: ModelDescriptor| {
        let tiers = classify_model_tiers(&[weaker.clone(), stronger.clone()]);
        assert!(
            placed(&tiers, stronger.id()).rank < placed(&tiers, weaker.id()).rank,
            "{provider}: {} must stand above {}: {tiers:?}",
            stronger.id(),
            weaker.id()
        );
    };
    // Class beats price: the fast model costs more and still stands below.
    pair(
        "class",
        row("class-fast", "p1", FAST, 2)
            .list_price_micro_usd(9_000_000, 45_000_000)
            .context_window(1_000_000)
            .effort_ceiling(EffortCeiling::Ultra),
        row("class-balanced", "p1", BALANCED, 1)
            .list_price_micro_usd(1_000_000, 5_000_000)
            .context_window(200_000)
            .effort_ceiling(EffortCeiling::High),
    );
    // Price beats the context window.
    pair(
        "price",
        row("price-cheap", "p2", BALANCED, 2)
            .list_price_micro_usd(3_000_000, 15_000_000)
            .context_window(1_000_000)
            .effort_ceiling(EffortCeiling::Ultra),
        row("price-dear", "p2", BALANCED, 1)
            .list_price_micro_usd(5_000_000, 25_000_000)
            .context_window(200_000)
            .effort_ceiling(EffortCeiling::High),
    );
    // The context window beats the release order.
    pair(
        "context",
        row("context-small", "p3", BALANCED, 2)
            .list_price_micro_usd(3_000_000, 15_000_000)
            .context_window(200_000)
            .effort_ceiling(EffortCeiling::Ultra),
        row("context-large", "p3", BALANCED, 1)
            .list_price_micro_usd(3_000_000, 15_000_000)
            .context_window(1_000_000)
            .effort_ceiling(EffortCeiling::High),
    );
    // The release order beats the effort ceiling.
    pair(
        "release",
        row("release-old", "p4", BALANCED, 1)
            .list_price_micro_usd(3_000_000, 15_000_000)
            .context_window(200_000)
            .effort_ceiling(EffortCeiling::Ultra),
        row("release-new", "p4", BALANCED, 2)
            .list_price_micro_usd(3_000_000, 15_000_000)
            .context_window(200_000)
            .effort_ceiling(EffortCeiling::High),
    );
    // And the effort ceiling decides last.
    pair(
        "effort",
        row("effort-high", "p5", BALANCED, 1).effort_ceiling(EffortCeiling::High),
        row("effort-max", "p5", BALANCED, 1).effort_ceiling(EffortCeiling::Max),
    );
}

/// What a person sees through the inventory every selector reads: the same
/// order, stamped once.
#[test]
fn the_inventory_stamps_the_fact_order() {
    let inventory = ModelInventory::new(
        "a",
        vec![row("a", "p", FRONTIER, 1), row("c", "p", FAST, 2), row("b", "p", BALANCED, 1)],
    );
    assert_eq!(inventory.find("b").map(ModelDescriptor::band), Some(ModelBand::Second));
    assert!(inventory.find("c").is_some_and(|model| model.serves_rung(ImplRung::Easy)));
    assert!(!inventory.find("c").is_some_and(|model| model.serves_rung(ImplRung::Hard)));
}

fn judged(places: &[(&str, u16, u16)]) -> TierJudgment {
    TierJudgment {
        places: places
            .iter()
            .map(|(id, score_permille, confidence_permille)| {
                ((*id).to_string(), JudgedPlace { score_permille: *score_permille, confidence_permille: *confidence_permille })
            })
            .collect(),
    }
}

/// A judgment orders the implementers it covers, whatever the fact rule
/// says — here the facts put the dearer `b` first and the judgment, having
/// read the recorded outcomes, puts `c` first — and each place says who
/// gave it and how sure it was.
#[test]
fn a_judgment_orders_the_models_it_covers_and_says_how_sure_it_was() {
    let models = [
        row("a", "p", FRONTIER, 1).list_price_micro_usd(10_000_000, 50_000_000),
        row("b", "p", BALANCED, 1).list_price_micro_usd(5_000_000, 25_000_000),
        row("c", "p", BALANCED, 1).list_price_micro_usd(3_000_000, 15_000_000),
    ];
    let judgment = judged(&[("a", 950, 900), ("b", 400, 800), ("c", 700, 850)]);
    let tiers = classify_model_tiers_judged(&models, Some(&judgment));
    assert_eq!(placed(&tiers, "a").band, ModelBand::Top, "{tiers:?}");
    assert_eq!(rungs(&tiers, "c"), vec![ImplRung::Hard], "{tiers:?}");
    assert_eq!(rungs(&tiers, "b"), vec![ImplRung::Easy, ImplRung::Medium], "{tiers:?}");
    assert_eq!(placed(&tiers, "c").source, TierSource::Judged, "{tiers:?}");
    assert_eq!(placed(&tiers, "c").confidence_permille, Some(850), "{tiers:?}");
}

/// A model discovered after the judgment stands where the facts put it,
/// marked provisional and without a confidence; once a judgment covers it
/// (the next one, its outcomes behind it), it moves to the judged place.
#[test]
fn a_new_model_stands_provisionally_by_the_facts_and_moves_once_judged() {
    let lineup = |with_new: bool| {
        let mut models = vec![
            row("a", "p", FRONTIER, 1).list_price_micro_usd(10_000_000, 50_000_000),
            row("b", "p", BALANCED, 1).list_price_micro_usd(5_000_000, 25_000_000),
            row("c", "p", BALANCED, 1).list_price_micro_usd(1_000_000, 5_000_000),
        ];
        if with_new {
            models.push(row("d", "p", BALANCED, 1).list_price_micro_usd(3_000_000, 15_000_000));
        }
        models
    };
    let first = judged(&[("a", 950, 900), ("b", 700, 800), ("c", 400, 800)]);
    let tiers = classify_model_tiers_judged(&lineup(true), Some(&first));
    let new = placed(&tiers, "d");
    assert_eq!(new.source, TierSource::Provisional, "{tiers:?}");
    assert_eq!(new.confidence_permille, None, "{tiers:?}");
    assert!(placed(&tiers, "b").rank < new.rank && new.rank < placed(&tiers, "c").rank, "{tiers:?}");
    assert_eq!(placed(&tiers, "b").source, TierSource::Judged, "{tiers:?}");

    let second = judged(&[("a", 950, 900), ("b", 700, 800), ("c", 400, 800), ("d", 800, 600)]);
    let tiers = classify_model_tiers_judged(&lineup(true), Some(&second));
    assert_eq!(placed(&tiers, "d").source, TierSource::Judged, "{tiers:?}");
    assert_eq!(rungs(&tiers, "d"), vec![ImplRung::Hard], "{tiers:?}");
    assert_eq!(rungs(&tiers, "b"), vec![ImplRung::Medium], "{tiers:?}");
}

/// Without a judgment every place is the fact rule's, and says so.
#[test]
fn without_a_judgment_every_place_is_the_fact_rules() {
    let models = [row("a", "p", FRONTIER, 1), row("b", "p", BALANCED, 1), row("c", "p", FAST, 2)];
    for entry in classify_model_tiers_judged(&models, None) {
        assert_eq!(entry.source, TierSource::FactRule, "{entry:?}");
        assert_eq!(entry.confidence_permille, None, "{entry:?}");
    }
}

/// A provider whose rungs would all fall onto one implementer leaves the
/// hard rung to its top model, so no provider's hard work lands below where
/// it stood (t-45961, the person and ZO, 10-11: with the size word fixed,
/// Google's Pro became the top band and its one current Flash took easy,
/// ordinary and hard work alike). The lone implementer keeps the easy and
/// ordinary rungs; an older release of its class serves none. Where two
/// implementers serve, the top band stays out of the rungs.
#[test]
fn a_lone_implementer_leaves_the_hard_rung_to_the_top_model() {
    let lone = [row("a", "p", FRONTIER, 31), row("b", "p", FAST, 38), row("c", "p", FAST, 31)];
    let tiers = classify_model_tiers(&lone);
    assert_eq!(placed(&tiers, "a").band, ModelBand::Top, "{tiers:?}");
    assert_eq!(rungs(&tiers, "a"), vec![ImplRung::Hard], "{tiers:?}");
    assert_eq!(rungs(&tiers, "b"), vec![ImplRung::Easy, ImplRung::Medium], "{tiers:?}");
    assert!(rungs(&tiers, "c").is_empty(), "an older release of the same class serves no rung: {tiers:?}");

    let two = [row("a", "p", FRONTIER, 1), row("b", "p", BALANCED, 1), row("c", "p", FAST, 1)];
    let tiers = classify_model_tiers(&two);
    assert!(rungs(&tiers, "a").is_empty(), "two implementers serve: the top plans and verifies: {tiers:?}");
    assert_eq!(rungs(&tiers, "b"), vec![ImplRung::Hard], "{tiers:?}");
    assert_eq!(rungs(&tiers, "c"), vec![ImplRung::Easy, ImplRung::Medium], "{tiers:?}");

    let alone = [row("a", "p", FRONTIER, 1)];
    assert!(rungs(&classify_model_tiers(&alone), "a").is_empty(), "a top model with no implementer below it stays the top");
}

/// t-46882: an older release names the newer release of its line that stands
/// in its place — what the window resumes a session on instead of it — and
/// nothing else names one: not the release that stands, not a model of
/// another line, and not a twin of the same release (`acme-house[blue]`
/// beside `[green]`, two models a person configured: neither is older, and
/// moving one would move the person's choice).
#[test]
fn a_superseded_release_names_the_newer_release_of_its_line() {
    let models = [
        row("a-5-1", "p", FRONTIER, 51),
        row("a-5", "p", FRONTIER, 50),
        row("b-5", "p", BALANCED, 50),
        row("acme-house[blue]", "p", FAST, 50),
        row("acme-house[green]", "p", FAST, 50),
    ];
    let tiers = classify_model_tiers(&models);
    assert_eq!(placed(&tiers, "a-5").band, ModelBand::Superseded, "{tiers:?}");
    assert_eq!(placed(&tiers, "a-5").successor.as_deref(), Some("a-5-1"), "{tiers:?}");
    for current in ["a-5-1", "b-5"] {
        assert_eq!(placed(&tiers, current).successor, None, "{current}: {tiers:?}");
    }
    let twins: Vec<&ModelTierAssignment> =
        tiers.iter().filter(|entry| entry.id.starts_with("acme-house")).collect();
    assert!(twins.iter().any(|twin| twin.band == ModelBand::Superseded), "one twin folds: {tiers:?}");
    assert!(twins.iter().all(|twin| twin.successor.is_none()), "a twin is no older release: {tiers:?}");
}
