//! Runtime adapter that builds Smart Router model inventories from provider state.
//!
//! This module is intentionally outside `model_router`: the router core stays
//! pure, while this edge adapter may inspect provider configuration and custom
//! provider catalogs.

use api::ProviderKind;

use crate::model_router::{
    EffortCeiling, ModelCapability, ModelDescriptor, ModelInventory, ModelSource, ModelStatus, ModelTier,
    TiersProvenance,
};

#[must_use]
pub fn connected_model_inventory(default_model: &str) -> ModelInventory {
    // Probe each provider KIND once, not once per catalog entry: a probe can
    // cost a credentials-file read + parse (OpenAI OAuth, Google ADC), and the
    // catalog carries many model aliases per provider — probing per entry did
    // ~40 redundant file reads on every inventory build.
    let mut probed: Vec<(ProviderKind, bool)> = Vec::new();
    let mut usable = |kind: ProviderKind| -> bool {
        if let Some((_, ok)) = probed.iter().find(|(probed_kind, _)| *probed_kind == kind) {
            return *ok;
        }
        let ok = api::provider_usable_for_smart_inventory(kind);
        probed.push((kind, ok));
        ok
    };
    let providers: Vec<ProviderKind> = api::provider_catalog()
        .iter()
        .filter_map(|entry| usable(entry.provider).then_some(entry.provider))
        .collect();
    let custom_models = api::custom_provider_usable_catalog()
        .into_iter()
        .map(|(provider, models)| (provider.to_string(), models))
        .collect::<Vec<_>>();
    let catalog = crate::model_catalog::ModelCatalog::load().ok();
    model_inventory_from_authorized_providers_with_catalog(
        default_model,
        &providers,
        &custom_models,
        catalog.as_ref(),
    )
}

#[must_use]
pub fn model_inventory_from_authorized_providers(
    default_model: &str,
    authorized_providers: &[ProviderKind],
    custom_models: &[(String, Vec<String>)],
) -> ModelInventory {
    model_inventory_from_authorized_providers_with_catalog(
        default_model,
        authorized_providers,
        custom_models,
        None,
    )
}

fn model_inventory_from_authorized_providers_with_catalog(
    default_model: &str,
    authorized_providers: &[ProviderKind],
    custom_models: &[(String, Vec<String>)],
    catalog: Option<&crate::model_catalog::ModelCatalog>,
) -> ModelInventory {
    let mut models = Vec::new();
    for entry in api::provider_catalog() {
        let hidden = catalog.is_some_and(|catalog| {
            catalog.builtin_hidden(entry.provider, entry.canonical_model_id)
        });
        if authorized_providers.contains(&entry.provider) && !hidden {
            models.push(descriptor_for_catalog_entry(entry));
        }
    }
    for (provider, model_ids) in custom_models {
        for model_id in model_ids {
            models.push(descriptor_for_custom_model(provider, model_id));
        }
    }
    ModelInventory::new(default_model, models)
}

fn descriptor_for_catalog_entry(entry: &api::ProviderCatalogEntry) -> ModelDescriptor {
    let id = entry.canonical_model_id;
    let provider = provider_key(entry.provider);
    let family = family_for_model(id);
    let class = class_for_model(id);
    let (tiers, tiers_provenance) = tiers_for_model(id);
    let mut descriptor = ModelDescriptor::new(id, provider, family)
        .source(ModelSource::EnabledBuiltinProvider)
        .class(class)
        .capabilities(capabilities_for_model(id))
        .tiers(tiers)
        .tiers_provenance(tiers_provenance)
        .status(status_for_model(id, true))
        .release_rank(release_rank_for_model(id))
        .effort_ceiling(effort_ceiling_for_model(id));
    if let Some(context_window) = context_window_for_descriptor(id) {
        descriptor = descriptor.context_window(context_window);
    }
    if id == "ollama" {
        descriptor = descriptor
            .source(ModelSource::LocalProvider)
            .capabilities([ModelCapability::ToolUse]);
    }
    descriptor
}

fn descriptor_for_custom_model(provider: &str, id: &str) -> ModelDescriptor {
    let family = custom_family_for_model(provider, id);
    let (tiers, tiers_provenance) = custom_tiers_for_model(id);
    let mut descriptor = ModelDescriptor::new(id.to_string(), provider.to_string(), family)
        .source(ModelSource::CustomProvider)
        .class(class_for_model(id))
        .capabilities(custom_capabilities_for_model(id))
        .tiers(tiers)
        .tiers_provenance(tiers_provenance)
        .status(status_for_model(id, false))
        .release_rank(release_rank_for_model(id))
        .effort_ceiling(effort_ceiling_for_model(id));
    if let Some(context_window) = context_window_for_descriptor(id) {
        descriptor = descriptor.context_window(context_window);
    }
    descriptor
}

fn custom_family_for_model(provider: &str, id: &str) -> &'static str {
    let provider = provider.to_ascii_lowercase();
    if provider.contains("deepseek") || id.to_ascii_lowercase().contains("deepseek") {
        "deepseek"
    } else {
        let family = family_for_model(id);
        if family == "unknown" { "custom" } else { family }
    }
}

fn provider_key(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Google => "google",
        ProviderKind::Xai => "xai",
        ProviderKind::Ollama => "ollama",
    }
}

fn family_for_model(id: &str) -> &'static str {
    let id = id.to_ascii_lowercase();
    if id.contains("claude") {
        "claude"
    } else if id.contains("gpt") || id.contains("codex") {
        "gpt"
    } else if id.contains("gemini") {
        "gemini"
    } else if id.contains("grok") {
        "grok"
    } else if id.contains("deepseek") {
        "deepseek"
    } else if id.contains("qwen") {
        "qwen"
    } else if id.contains("llama") {
        "llama"
    } else if id.contains("glm") {
        "glm"
    } else if id.contains("mistral") || id.contains("mixtral") {
        "mistral"
    } else if id.contains("ornith") {
        "ornith"
    } else if id.contains("ollama") {
        "ollama"
    } else {
        "unknown"
    }
}

/// Free-form marketing-name/flavor label (`opus`/`sonnet`/`fable`/`strong`/
/// `fast`/...) used ONLY for `RoleSelector::class` string matching — a
/// mechanism no `auto_selectors_for_role` selector currently populates.
/// Deliberately left untouched by the Phase 8 provider-declared-class work:
/// this string is a different axis from [`api::ModelClass`] (frontier/
/// balanced/fast lineup POSITIONING, consumed by `tiers_for_model` via
/// [`declared_tiers_for_model`]) — e.g. `claude-fable-5` gets `class_label()
/// == "fable"` here regardless of whether its declared positioning is
/// frontier or something else. Merging the two would conflate "which model
/// family/flavor is this" with "where did the provider rank it," which are
/// orthogonal questions.
fn class_for_model(id: &str) -> &'static str {
    // `[fast]` 같은 서빙 티어 브래킷은 능력 축과 직교(같은 모델, 우선
    // 서빙일 뿐)이므로 분류 전에 벗긴다 — 안 벗기면 "fast" substring이
    // gpt-5.6-terra[fast]를 소형/fast 클래스로 오분류한다(gpt-5.5-fast
    // 골든 노트에 문서화된 것과 같은 클래스의 함정).
    let id = strip_service_tier_suffix(id).to_ascii_lowercase();
    if id.contains("opus") {
        "opus"
    } else if id.contains("sonnet") {
        "sonnet"
    } else if id.contains("fable") {
        "fable"
    } else if id.contains("flash") || id.contains("fast") || id.contains("mini") || id.contains("spark") {
        "fast"
    } else if id.contains("pro") {
        "pro"
    } else if id.contains("reasoner") {
        "reasoner"
    } else if id.contains("chat") {
        "chat"
    } else if id.contains("gpt") || id.contains("codex") {
        "strong"
    } else {
        "balanced"
    }
}

/// A small/fast model variant — the cheap tier of any provider (haiku, mini,
/// flash, nano, lite, and the `-fast` priority-serving variants — the words
/// are the catalog's `priors.small_tokens`). These serve
/// the Fast role (Explore, statusline) and balanced work, never the Strong/Deep
/// flagship roles. Detection is by size token, not by provider, so every
/// provider's small model is recognized uniformly — an Anthropic-primary
/// inventory's `haiku`, an OpenAI one's `mini`/`-fast`, a Gemini one's `flash`.
fn is_small_model(id: &str) -> bool {
    // 서빙 티어 브래킷(`[fast]`)은 사이즈 신호가 아니다 — class_for_model과
    // 같은 이유로 벗기고 판정한다.
    let id = strip_service_tier_suffix(id);
    let priors = api::router_priors();
    has_any_token(id, &priors.small_tokens)
        || largest_parameter_billion(id)
            .is_some_and(|size| size < priors.small_parameter_billion_below)
}

/// The cold-start tables are word lists in the catalog's `priors` section
/// ([`api::router_priors`]); membership is a substring test on the bare id.
fn has_any_token(id: &str, tokens: &[String]) -> bool {
    tokens.iter().any(|token| id.contains(token.as_str()))
}

/// `gpt-5.6-terra[fast]` → `gpt-5.6-terra`: 브래킷 서빙 티어 접미사를 벗긴
/// 캐파빌리티 id. 브래킷 표기는 wire 직전(`chatgpt_backend`)에만 의미가
/// 있고, 클래스/사이즈/티어 분류는 전부 bare id 기준이어야 한다.
fn strip_service_tier_suffix(id: &str) -> &str {
    id.split('[').next().unwrap_or(id)
}

fn largest_parameter_billion(id: &str) -> Option<u32> {
    let bytes = id.as_bytes();
    let mut index = 0;
    let mut largest = None;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let Some(unit) = bytes.get(index).copied() else {
            continue;
        };
        if !matches!(unit, b'b' | b'B') {
            continue;
        }
        let value = id[start..index].parse::<u32>().unwrap_or(0);
        largest = Some(largest.map_or(value, |current: u32| current.max(value)));
    }
    largest
}

/// A model from a recognized frontier provider family. Membership grants the
/// full specialist capability set **uniformly** — every modern frontier model
/// can code, debug, verify, analyze, write, and design — so routing is
/// generalized across providers instead of cherry-picking per family. The
/// small-vs-flagship QUALITY distinction is carried by tier ([`tiers_for_model`]),
/// not by withholding a capability, so which model actually serves a role is a
/// tier decision and a GPT- or Gemini-primary inventory is as first-class as an
/// Anthropic one.
fn is_frontier_family(id: &str) -> bool {
    has_any_token(id, &api::router_priors().frontier_family_tokens)
}

/// The heaviest reasoning lines across providers — they carry the Deep tier that
/// the Analysis/Research/Judge/Synthesizer roles try first. When no Deep model is
/// connected, the router now walks down to Strong/Balanced candidates before the
/// main-model fallback, so providers without a Deep line can still participate.
///
/// **Cold-start fallback, not a capability fact.** This matches on marketing-name
/// tokens (`opus`, `pro`, `reasoner`, ...) with zero effort-ceiling data behind
/// it — it exists ONLY for models `tiers_for_model` cannot otherwise place via
/// `effort_ceiling` (a real, provider-declared capability signal). Recorded
/// with `TiersProvenance::Fallback` wherever it fires; a model whose
/// `effort_ceiling` is `Ultra` is promoted to Deep by that stronger signal
/// before this name-token guess is ever consulted.
fn is_deep_flagship(id: &str) -> bool {
    has_any_token(id, &api::router_priors().deep_flagship_tokens)
}

/// `ModelCapability::Fast` grant predicate: true for a small/cheap-token
/// model ([`is_small_model`]) OR a model whose provider has explicitly
/// declared it `fast` ([`api::declared_model_class`]). The two signals can
/// disagree — `gpt-5.6-luna` is declared fast but its id carries no
/// small-model TOKEN (`is_small_model` alone would miss it, since "luna"
/// matches none of `fast`/`flash`/`mini`/`haiku`/`nano`/`lite`/`spark`) — so
/// this keeps the capability axis in lockstep with [`declared_tiers_for_model`]'s
/// tier axis: a model cannot end up with `ModelTier::Fast` but no
/// `ModelCapability::Fast`, which would silently exclude it from the Fast
/// role's `capability(Fast)+tier(Fast)` selector (the fan-out triage
/// regression this fixes — see `tools::fanout::decompose_model`).
fn is_fast_capable(id: &str) -> bool {
    is_small_model(&id.to_ascii_lowercase())
        || matches!(api::declared_model_class(id), Some(api::ModelClass::Fast))
}

fn capabilities_for_model(id: &str) -> Vec<ModelCapability> {
    let lower = id.to_ascii_lowercase();
    let mut capabilities = vec![ModelCapability::Default, ModelCapability::ToolUse];
    if is_fast_capable(id) {
        capabilities.push(ModelCapability::Fast);
    }
    // Uniform specialist grant for every frontier family. The previous table
    // cherry-picked per provider (Writing/Design were claude/gemini-only; Analysis
    // was a partial id set), which made non-Anthropic inventories second-class.
    // All frontier models can perform these tasks; TIER decides which one a role
    // actually selects, so granting the full set here is safe (a Fast-tier model
    // with the Coding capability still loses the Coding role to a Strong-tier one).
    if is_frontier_family(&lower) {
        capabilities.extend([
            ModelCapability::Coding,
            ModelCapability::Debugging,
            ModelCapability::Verification,
            ModelCapability::StructuredOutput,
            ModelCapability::Analysis,
            ModelCapability::Writing,
            ModelCapability::Design,
        ]);
    }
    capabilities
}

fn specialist_capabilities() -> [ModelCapability; 7] {
    [
        ModelCapability::Coding,
        ModelCapability::Debugging,
        ModelCapability::Verification,
        ModelCapability::StructuredOutput,
        ModelCapability::Analysis,
        ModelCapability::Writing,
        ModelCapability::Design,
    ]
}

/// Whether a user-registered custom-provider model may compete for the
/// specialist roles (Coding, Verification, Analysis, …) rather than being
/// limited to Default/Fast/Balanced work.
///
/// Registering a custom provider IS the operator's statement that the model is
/// worth routing to — nobody configures an endpoint and a credential for a
/// model they do not want used. The rule additionally used to demand a
/// recognized frontier family name or a spelled-out `>=30B` parameter count,
/// which made the id STRING the arbiter of competence: `qwen3-coder-480b`
/// competed, while an equally capable `kimi-k2`, `minimax-m2`, or a gateway's
/// `default` alias could not hold [`ModelCapability::Coding`] and therefore
/// lost every specialist rung to a builtin model the user may not even have
/// provisioned. Since the rungs AND-filter capability against tier, such a
/// model matched nothing and the role silently fell back to the main model —
/// exactly the "Smart ignores the models I provisioned" failure.
///
/// Small models stay excluded: an 8B/mini/flash-class endpoint really is the
/// wrong place for the Verifier and Judge rungs, and that judgment reads a size
/// token rather than a brand allowlist. Past this gate the router still ranks
/// candidates by tier, release recency, and recorded outcomes, so a weak
/// endpoint loses on merit instead of being excluded by its name.
fn custom_model_has_specialist_signal(id: &str) -> bool {
    !is_small_model(&id.to_ascii_lowercase())
}

fn custom_capabilities_for_model(id: &str) -> Vec<ModelCapability> {
    let mut capabilities = vec![ModelCapability::Default, ModelCapability::ToolUse];
    if is_fast_capable(id) {
        capabilities.push(ModelCapability::Fast);
    }
    // A custom provider entry is an explicit, user-configured LLM endpoint, so it
    // competes for specialist roles unless its id declares it small — see
    // `custom_model_has_specialist_signal`. Small models stay useful for
    // Default/Fast/Balanced work and remain reachable by an explicit pin.
    if custom_model_has_specialist_signal(id) {
        for capability in specialist_capabilities() {
            if !capabilities.contains(&capability) {
                capabilities.push(capability);
            }
        }
    }
    capabilities
}

/// Rollback switch for the `effort_ceiling == Ultra ⇒ Deep` promotion below
/// (Phase 1 design principle: every static rule must be killable without a
/// code revert). Disabling it does not remove Deep tier entitlement outright —
/// it just stops the ceiling-derived promotion, falling through to the
/// existing `is_deep_flagship` name-token fallback (or the generic frontier
/// grant) exactly as before this phase.
const DISABLE_ULTRA_DEEP_PROMOTION_ENV: &str = "ZO_DISABLE_ULTRA_DEEP_PROMOTION";

fn ultra_deep_promotion_disabled() -> bool {
    std::env::var(DISABLE_ULTRA_DEEP_PROMOTION_ENV)
        .is_ok_and(|value| value.trim() == "1")
}

/// The model's declared effort ceiling, bridged from the `api` crate's
/// provider-declared capability fact ([`api::max_supported_effort`]) into the
/// router's local, api-independent [`EffortCeiling`] axis. This file is the
/// edge adapter (see module doc) that is allowed to depend on `api`; the pure
/// `model_router` core never sees `api` types directly.
fn effort_ceiling_for_model(id: &str) -> EffortCeiling {
    match api::max_supported_effort(id) {
        api::EffortLevel::Ultra => EffortCeiling::Ultra,
        api::EffortLevel::Max => EffortCeiling::Max,
        api::EffortLevel::Xhigh => EffortCeiling::Xhigh,
        api::EffortLevel::High | api::EffortLevel::Medium | api::EffortLevel::Low => EffortCeiling::High,
    }
}

/// The model's declared context window in tokens, bridged from
/// [`api::context_window_for_model`] (always returns a best-effort value, so
/// this is `None` only if it somehow overflows `u32`, which never happens for
/// any real catalog entry).
fn context_window_for_descriptor(id: &str) -> Option<u32> {
    u32::try_from(api::context_window_for_model(id)).ok()
}

/// The tier vector a provider-declared [`api::ModelClass`] maps to. This is
/// the FIRST check in [`tiers_for_model`] — ahead of `is_small_model`, the
/// `effort_ceiling == Ultra` promotion, and `is_deep_flagship` — because a
/// provider stating its own lineup position (OpenAI's Codex model cache,
/// Anthropic's public tier positioning) is strictly stronger evidence than
/// any name-token guess or capability side-effect this file derives on its
/// own. `frontier`/`balanced`/`fast` map onto the SAME three-tier vectors the
/// undeclared derivation below already produces for its analogous cases, so
/// a declared model's tier set lines up with what an equivalent undeclared
/// model would get — only the [`TiersProvenance`] differs.
fn declared_tiers_for_model(id: &str) -> Option<(Vec<ModelTier>, TiersProvenance)> {
    let tiers = match api::declared_model_class(id)? {
        api::ModelClass::Frontier => vec![ModelTier::Deep, ModelTier::Strong],
        api::ModelClass::Balanced => vec![ModelTier::Balanced, ModelTier::Strong],
        api::ModelClass::Fast => vec![ModelTier::Fast, ModelTier::Balanced],
    };
    Some((tiers, TiersProvenance::ProviderDeclared))
}

/// Tier assignment plus its [`TiersProvenance`] audit marker (Phase 7 consumes
/// the marker; today it is recorded on every descriptor for observability).
fn tiers_for_model(id: &str) -> (Vec<ModelTier>, TiersProvenance) {
    if let Some(declared) = declared_tiers_for_model(id) {
        return declared;
    }
    let lower = id.to_ascii_lowercase();
    if is_small_model(&lower) {
        // Cheap tier: Fast role + balanced work. Checked first so a `-fast`/`flash`
        // variant of a flagship line is never promoted to Strong/Deep.
        return (vec![ModelTier::Fast, ModelTier::Balanced], TiersProvenance::Fallback);
    }
    // Capability-derived promotion: a model whose provider-declared effort
    // ceiling reaches Ultra (today: GPT-5.6 Sol/Terra) earns Deep on that real
    // signal, ahead of the name-token `is_deep_flagship` guess below — this is
    // what lets sol/terra compete for the Analysis/Research/Judge/Synthesizer
    // Deep rung instead of it being an Anthropic-opus/-pro-token monopoly.
    // Kill-switched via `ZO_DISABLE_ULTRA_DEEP_PROMOTION` for a code-revert-
    // free rollback.
    if !ultra_deep_promotion_disabled() && effort_ceiling_for_model(id) == EffortCeiling::Ultra {
        return (vec![ModelTier::Deep, ModelTier::Strong], TiersProvenance::ColdStartPrior);
    }
    if is_deep_flagship(&lower) {
        return (vec![ModelTier::Deep, ModelTier::Strong], TiersProvenance::Fallback);
    }
    if is_frontier_family(&lower) {
        // Any other frontier flagship (claude-fable/sonnet, gpt, codex, grok,
        // deepseek-chat): Balanced + Strong. Generalized from the previous
        // hardcoded `sonnet||gpt||codex||deepseek||grok` set, which dropped
        // `claude-fable-5` — the newest Anthropic flagship — into the else branch
        // as Balanced-only, so it could not serve the Coding/Debugging (Strong)
        // roles at all even though it is the session's primary model.
        return (vec![ModelTier::Balanced, ModelTier::Strong], TiersProvenance::Fallback);
    }
    // Unknown / custom: a single conservative tier; the main-model fallback
    // covers it rather than over-claiming Strong/Deep for an unverified model.
    (vec![ModelTier::Balanced], TiersProvenance::Fallback)
}

fn custom_tiers_for_model(id: &str) -> (Vec<ModelTier>, TiersProvenance) {
    let (mut tiers, provenance) = tiers_for_model(id);
    if tiers.contains(&ModelTier::Deep) || tiers.contains(&ModelTier::Strong) {
        return (tiers, provenance);
    }
    let id_lower = id.to_ascii_lowercase();
    if is_small_model(&id_lower) {
        if !tiers.contains(&ModelTier::Fast) {
            tiers.push(ModelTier::Fast);
        }
        if !tiers.contains(&ModelTier::Balanced) {
            tiers.push(ModelTier::Balanced);
        }
    } else if custom_model_has_specialist_signal(id) {
        // A provisioned, non-small custom model competes as a Strong generalist.
        // Pairing this with the matching capability grant is what lets it reach a
        // specialist rung at all — the rungs AND-filter capability against tier,
        // so granting one without the other leaves the model matching nothing.
        if !tiers.contains(&ModelTier::Balanced) {
            tiers.push(ModelTier::Balanced);
        }
        tiers.push(ModelTier::Strong);
    }
    (tiers, provenance)
}

/// Routing freshness status.
///
/// A `preview` token in the id is a GUESS about stability, and for the shipped
/// catalog it is a wrong one: Google ships its flagship Gemini pro line under
/// preview-suffixed ids — the `gemini-pro` alias resolves to
/// `gemini-3.1-pro-preview`. Because [`FreshnessPolicy::LatestStable`] is the
/// default for every AUTO rung and HARD-excludes `Preview` (a filter, not a
/// penalty), that one substring made Google's entire flagship line unreachable
/// by AUTO routing. Not deprioritized — invisible, with no operator escape
/// short of a manual pin.
///
/// Catalog membership is the declaration that supersedes the guess: a row in
/// the shipped provider catalog is a model zo states it supports, whatever the
/// id happens to spell. A custom-provider endpoint keeps the name heuristic —
/// there the token is the only signal available, and staying conservative about
/// an endpoint zo knows nothing about is the right default.
fn status_for_model(id: &str, catalog_declared: bool) -> ModelStatus {
    if catalog_declared {
        return ModelStatus::Stable;
    }
    if id.to_ascii_lowercase().contains("preview") {
        ModelStatus::Preview
    } else {
        ModelStatus::Stable
    }
}

/// Recency rank derived from the **major.minor** version embedded in the model
/// id, so a newer major outranks an older one, and — within the same major — a
/// newer minor outranks an older one (`gpt-5.6-*` → 56 outranks `gpt-5.5` → 55,
/// both of which previously tied at 50). The original implementation summed
/// every digit, which is not monotonic with recency (`claude-3-7-sonnet` →
/// 3+7=10 wrongly outranked `claude-sonnet-4-5` → 4+5=9).
///
/// Returns `major * 10 + min(minor, 9)` for the first numeric run that looks
/// like a version, else 0. A candidate run (major OR minor) is rejected as
/// non-version when it is (a) > 99 (8-digit `YYYYMMDD` stamps, bare years),
/// (b) a zero-padded multi-digit run (`08` — a date month or day), or (c)
/// immediately followed by a unit/architecture suffix (`8x7b`, `70b`, `7m`,
/// `13k`). The minor is only read from the digit run immediately following the
/// major across a `.` or `-` separator (`5.6`, `4-8`); anything else (a
/// letter, as in `4o`; a rejected run, as in a trailing date) means there is
/// no minor and the rank stays major-only, exactly as before this became
/// minor-aware. A model with no minor segment (`claude-fable-5`, `gpt-5`)
/// gets `minor = 0`, so its rank is unchanged from the major-only scheme.
///
/// This is STILL a coarse, version-**recency** tie-breaker among
/// capability/tier-equal candidates (callers cap at 100) — it is NOT a
/// cross-line quality ranking. A higher major.minor from one line
/// (`gpt-5.6-sol` → 56) outranks a lower one from another
/// (`claude-sonnet-4-6` → 46); quality-aware ranking within a tier is a
/// separate concern (see `EffortCeiling`/tier assignment, which IS
/// capability-derived). Same-rank models tie, falling to
/// capability/tier/anchor and inventory order.
#[must_use]
pub fn release_rank_for_model(id: &str) -> u32 {
    let bytes = id.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let run = &id[start..index];
        if let Some(major) = version_component_value(run, bytes.get(index).copied()) {
            let minor = minor_after_major(bytes, index).unwrap_or(0);
            return major * 10 + minor.min(9);
        }
    }
    0
}

/// Shared major/minor false-positive guard: a digit `run` is a version
/// component when it is not zero-padded, not immediately followed by a
/// unit/architecture-suffix byte, and its value is `<= 99`.
fn version_component_value(run: &str, next_byte: Option<u8>) -> Option<u32> {
    let leading_zero = run.len() > 1 && run.starts_with('0');
    let unit_suffix = matches!(next_byte, Some(b'x' | b'X' | b'b' | b'B' | b'm' | b'M' | b'k' | b'K'));
    let value: u32 = run.parse().unwrap_or(u32::MAX);
    (!leading_zero && !unit_suffix && value <= 99).then_some(value)
}

/// The minor version immediately following a captured major run, when the
/// major is directly followed by a `.`/`-` separator and a digit run that
/// itself passes [`version_component_value`]. `major_end` is the byte index
/// right after the major run.
fn minor_after_major(bytes: &[u8], major_end: usize) -> Option<u32> {
    let separator = *bytes.get(major_end)?;
    if separator != b'.' && separator != b'-' {
        return None;
    }
    let start = major_end + 1;
    let mut index = start;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == start {
        return None;
    }
    let run = std::str::from_utf8(&bytes[start..index]).ok()?;
    version_component_value(run, bytes.get(index).copied())
}

#[cfg(test)]
mod release_rank_tests {
    use super::release_rank_for_model;

    #[test]
    fn newer_major_outranks_older_across_schemes() {
        // The bug the old digit-sum had: 3.7 (sum 10) beat 4.5 (sum 9).
        assert!(release_rank_for_model("claude-sonnet-4-5") > release_rank_for_model("claude-3-7-sonnet"));
        assert!(release_rank_for_model("claude-opus-4-1") > release_rank_for_model("claude-3-5-sonnet"));
        assert!(release_rank_for_model("gpt-5") > release_rank_for_model("gpt-4o"));
        assert!(release_rank_for_model("gemini-3-pro") > release_rank_for_model("gemini-2-5-pro"));
    }

    #[test]
    fn date_stamp_does_not_inflate_rank() {
        // A trailing YYYYMMDD (8-digit or hyphenated) must not change the rank.
        assert_eq!(
            release_rank_for_model("claude-opus-4-1-20250805"),
            release_rank_for_model("claude-opus-4-1"),
        );
        // A newer major still outranks a dated older major.
        assert!(
            release_rank_for_model("gpt-5-2026-01-01")
                > release_rank_for_model("claude-sonnet-4-20250514"),
        );
    }

    #[test]
    fn parameter_count_and_dates_do_not_inflate_rank() {
        // Regression: a "minor" parser captured param counts and date fragments,
        // pushing custom/dated ids to the 100 ceiling. Major-only keeps them modest.
        assert_eq!(release_rank_for_model("llama-3-70b"), 30);
        assert_eq!(release_rank_for_model("gpt-4o-2024-08-06"), 40);
        // A flagship major-4 must outrank a custom major-3 with a huge param count.
        assert!(release_rank_for_model("claude-opus-4-8") > release_rank_for_model("llama-3-70b"));
    }

    #[test]
    fn leading_param_or_date_tokens_are_not_versions() {
        // Mixture-of-experts arch ("8x7b") and a leading date month ("08") are NOT
        // versions: they must not be read as the major (which would inflate to the
        // ceiling and let a custom model beat the main model on the tier-less route).
        assert_eq!(release_rank_for_model("mixtral-8x7b"), 0);
        assert_eq!(release_rank_for_model("mixtral-8x22b"), 0);
        assert_eq!(release_rank_for_model("command-r-08-2024"), 0);
        // A real flagship must outrank these version-less custom ids.
        assert!(release_rank_for_model("gpt-5") > release_rank_for_model("mixtral-8x7b"));
    }

    #[test]
    fn minor_aware_rank_breaks_the_5_6_vs_5_5_tie() {
        // The headline Phase 1 fix: gpt-5.6-* must outrank gpt-5.5 instead of
        // tying at the major-only rank 50, which previously let inventory
        // insertion order (last-of-equal-maxima) resolve every cold-start
        // 5.6-vs-5.5 route to the older model.
        assert_eq!(release_rank_for_model("gpt-5.6-sol"), 56);
        assert_eq!(release_rank_for_model("gpt-5.6-terra"), 56);
        assert_eq!(release_rank_for_model("gpt-5.6-luna"), 56);
        assert_eq!(release_rank_for_model("gpt-5.5-2026-04-23"), 55);
        assert_eq!(release_rank_for_model("gpt-5.5-fast"), 55);
        assert!(release_rank_for_model("gpt-5.6-sol") > release_rank_for_model("gpt-5.5-2026-04-23"));
        // Dated/@/[service-tier] suffixed ids inherit the same minor-aware rank.
        assert_eq!(release_rank_for_model("gpt-5.6-sol-2026-07-09"), 56);
        assert_eq!(release_rank_for_model("gpt-5.6-terra@openai"), 56);
        assert_eq!(release_rank_for_model("gpt-5.6-terra[fast]"), 56);
    }

    #[test]
    fn families_with_no_minor_segment_keep_the_major_only_rank() {
        // "claude-fable-5" and "claude-sonnet-5" are a bare major with nothing
        // after it (no `.` or `-` followed by a digit run), so minor stays 0 —
        // unchanged from the pre-Phase-1 major-only rank.
        assert_eq!(release_rank_for_model("claude-fable-5"), 50);
        assert_eq!(release_rank_for_model("claude-sonnet-5"), 50);
        assert_eq!(release_rank_for_model("gpt-5"), 50);
    }

    #[test]
    fn minor_is_capped_at_nine() {
        assert_eq!(release_rank_for_model("gpt-4-15"), 49);
    }

    #[test]
    fn no_version_is_zero() {
        assert_eq!(release_rank_for_model("ollama"), 0);
        assert_eq!(release_rank_for_model("custom-model"), 0);
    }
}

#[cfg(test)]
mod capability_table_tests {
    use super::{capabilities_for_model, is_small_model, tiers_for_model as tiers_for_model_with_provenance};
    use crate::model_router::{ModelCapability, ModelTier};

    /// Test-only convenience: most existing assertions only care about the
    /// tier set, not the [`crate::model_router::TiersProvenance`] marker.
    fn tiers_for_model(id: &str) -> Vec<ModelTier> {
        tiers_for_model_with_provenance(id).0
    }

    /// The size/family/flagship words are catalog data, not literals: an
    /// override that publishes a `priors` field changes the router's answer
    /// without a rebuild, and a publish that declares none restores the
    /// shipped words. Serialized with the env-lock because the api registry
    /// is process-global.
    #[test]
    fn prior_tables_come_from_the_catalog() {
        let _lock = crate::test_env_lock();
        assert!(is_small_model("claude-haiku-4-5"), "shipped small word");
        assert!(!is_small_model("acme-tiny-1"), "not a shipped small word");
        api::refresh_model_registry_from_json(
            r#"{"models":[],"aliases":[],"priors":{"small_tokens":["tiny"]}}"#,
        );
        assert!(is_small_model("acme-tiny-1"), "the published word is read");
        assert!(!is_small_model("claude-haiku-4-5"), "the field was replaced, not extended");
        assert!(
            tiers_for_model("acme-tiny-1").contains(&ModelTier::Fast),
            "and the tier follows the word"
        );
        api::refresh_model_registry_from_json(r#"{"models":[],"aliases":[]}"#);
        assert!(is_small_model("claude-haiku-4-5"), "shipped words are back");
        assert!(!is_small_model("acme-tiny-1"));
    }

    #[test]
    fn grok_is_a_first_class_coder() {
        let caps = capabilities_for_model("grok-4");
        assert!(caps.contains(&ModelCapability::Coding), "grok should code");
        assert!(caps.contains(&ModelCapability::Debugging));
        assert!(caps.contains(&ModelCapability::Verification));
        // Strong tier is the routing-relevant win: it lets grok actually be
        // selected for the Coding/Debugging roles (capability without the tier
        // would still fall back to main).
        assert!(tiers_for_model("grok-4").contains(&ModelTier::Strong));
        // grok also gains the Analysis capability, but the Analysis ROLE
        // additionally requires the Deep tier, which grok does not have — so this
        // capability is latent until/unless grok is granted Deep (parity with gpt).
        assert!(caps.contains(&ModelCapability::Analysis));
        assert!(!tiers_for_model("grok-4").contains(&ModelTier::Deep));
    }

    #[test]
    fn fast_grok_variant_stays_fast_tier() {
        // The fast/mini branch precedes the grok branch, so a fast variant is NOT
        // promoted to Strong (it can verify at Balanced, like gpt-mini, but is not
        // a Coding/Debugging candidate).
        let tiers = tiers_for_model("grok-4-fast");
        assert!(tiers.contains(&ModelTier::Fast));
        assert!(!tiers.contains(&ModelTier::Strong));
    }

    #[test]
    fn anthropic_haiku_is_recognized_as_fast() {
        // Without this, an Anthropic-primary inventory has no Fast-capable model,
        // so the Fast role (Explore/statusline) is forced cross-provider to gpt/gemini.
        let id = "claude-haiku-4-5-20251001";
        assert!(capabilities_for_model(id).contains(&ModelCapability::Fast), "haiku is fast");
        assert!(tiers_for_model(id).contains(&ModelTier::Fast));
        // It is NOT promoted to Strong/Deep — haiku is fast/small, not a flagship.
        assert!(!tiers_for_model(id).contains(&ModelTier::Strong));
        assert!(!tiers_for_model(id).contains(&ModelTier::Deep));
    }

    #[test]
    fn gemini_gains_specialist_capabilities() {
        // Capability-level grant (gemini supports coding/verification/structured
        // output). Note: the Coding ROLE needs Strong tier, which among Gemini ids
        // only the (preview) pro models have, so under default freshness this is
        // mostly latent — but correct, and active when previews are allowed.
        let caps = capabilities_for_model("gemini-3-pro");
        assert!(caps.contains(&ModelCapability::Coding));
        assert!(caps.contains(&ModelCapability::Verification));
        assert!(caps.contains(&ModelCapability::StructuredOutput));
        assert!(caps.contains(&ModelCapability::Analysis));
    }

    #[test]
    fn claude_fable_is_a_first_class_flagship() {
        // Regression: `claude-fable-5` (the newest Anthropic flagship and a common
        // session main) matched none of the old tier substrings and fell into the
        // Balanced-only else branch, so it could not serve premium escalated
        // Coding/Debugging (Strong) routes. Ordinary Coding is filtered separately
        // by `implementation_route_model_allowed`; keeping the capability here lets
        // Large/repeated-failure escalation opt in without lying about model fit.
        let id = "claude-fable-5";
        assert!(!is_small_model(id), "the flagship is not a small/fast variant");
        let tiers = tiers_for_model(id);
        assert!(tiers.contains(&ModelTier::Strong), "fable must remain escalation-capable");
        let caps = capabilities_for_model(id);
        assert!(caps.contains(&ModelCapability::Coding));
        assert!(caps.contains(&ModelCapability::Writing));
    }

    #[test]
    fn writing_and_design_are_granted_to_every_frontier_family() {
        // Generalization: Writing/Design were previously claude/gemini-only, so a
        // GPT- or grok-primary inventory had no same-provider Writing/Design model
        // and (under provider anchoring) fell back to the main model for those
        // roles. Every frontier family now carries them uniformly.
        for id in ["gpt-5.5-2026-04-23", "grok-3", "deepseek-chat", "gemini-3-pro"] {
            let caps = capabilities_for_model(id);
            assert!(caps.contains(&ModelCapability::Writing), "{id} should write");
            assert!(caps.contains(&ModelCapability::Design), "{id} should design");
            assert!(caps.contains(&ModelCapability::Analysis), "{id} should analyze");
        }
    }

    #[test]
    fn openai_flagship_is_strong_but_not_deep() {
        // OpenAI's catalog has no opus/pro/reasoner line, so its flagship is Strong
        // (serves Coding/Debugging same-provider) but not Deep. The Analysis/Deep
        // roles for a GPT main are served by the same-provider main fallback under
        // hard provider anchoring — never by crossing to a claude/gemini Deep model.
        let tiers = tiers_for_model("gpt-5.5-2026-04-23");
        assert!(tiers.contains(&ModelTier::Strong));
        assert!(!tiers.contains(&ModelTier::Deep));
    }

    #[test]
    fn declared_class_now_decides_sol_terra_luna_tiers() {
        // Phase 8 supersedes the Phase 1 Ultra-ceiling proxy with the
        // provider's OWN stated lineup position (OpenAI's Codex model cache,
        // fetched 2026-07-09): sol="Latest frontier agentic coding model"
        // (priority 1), terra="Balanced ... for everyday work" (priority 2),
        // luna="Fast and affordable" (priority 3). Declared class is checked
        // FIRST in `tiers_for_model`, so:
        // - sol keeps Deep — now via `ProviderDeclared`, not the Ultra-ceiling
        //   `ColdStartPrior` (still true too, but no longer the reason).
        // - terra LOSES Deep — its declared class is balanced, which beats its
        //   still-Ultra effort ceiling (see the precedence test below).
        // - luna GAINS Fast (and correspondingly loses Strong) — its declared
        //   class is fast, not the Balanced+Strong it fell into by default.
        assert!(tiers_for_model("gpt-5.6-sol").contains(&ModelTier::Deep));
        assert!(tiers_for_model("gpt-5.6-sol").contains(&ModelTier::Strong));
        assert!(!tiers_for_model("gpt-5.6-terra").contains(&ModelTier::Deep));
        assert!(tiers_for_model("gpt-5.6-terra").contains(&ModelTier::Balanced));
        assert!(tiers_for_model("gpt-5.6-terra").contains(&ModelTier::Strong));
        assert!(tiers_for_model("gpt-5.6-luna").contains(&ModelTier::Fast));
        assert!(tiers_for_model("gpt-5.6-luna").contains(&ModelTier::Balanced));
        assert!(!tiers_for_model("gpt-5.6-luna").contains(&ModelTier::Strong));
        assert!(!tiers_for_model("gpt-5.6-luna").contains(&ModelTier::Deep));
    }

    #[test]
    fn declared_fast_class_grants_the_fast_capability_even_without_a_small_model_token() {
        // Regression: `gpt-5.6-luna`'s id carries no `is_small_model` token
        // ("luna" matches none of fast/flash/mini/haiku/nano/lite/spark), so
        // before this fix it gained `ModelTier::Fast` (via declared class)
        // but NOT `ModelCapability::Fast` (still gated on `is_small_model`) —
        // a split-brain that silently excluded it from the Fast role's
        // `capability(Fast)+tier(Fast)` selector (caught by
        // `tools::fanout::decompose_model_follows_active_provider_not_hardcoded_haiku`,
        // which regressed to `gpt-5.3-codex-spark` instead of picking the
        // newer, provider-declared-fast `gpt-5.6-luna`).
        assert!(capabilities_for_model("gpt-5.6-luna").contains(&ModelCapability::Fast));
        assert!(tiers_for_model("gpt-5.6-luna").contains(&ModelTier::Fast));
        // gpt-5.6-sol/terra are NOT declared fast, so this does not
        // over-grant the capability to the rest of the family.
        assert!(!is_small_model("gpt-5.6-luna"), "luna carries no small-model token by itself");
    }

    #[test]
    fn ultra_deep_promotion_kill_switch_disables_the_rule_for_undeclared_models_only() {
        // Drop guards so a failing assertion cannot leak either env override
        // into other tests in this binary (env pollution cascades misdiagnose
        // the real failure).
        struct RemoveOnDrop(&'static str);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) { std::env::remove_var(self.0); }
        }
        struct EffortCeilingGuard;
        impl Drop for EffortCeilingGuard {
            fn drop(&mut self) { std::env::remove_var(api::MODEL_EFFORT_CEILINGS_ENV); }
        }
        let _lock = crate::test_env_lock();

        // A synthetic, undeclared model (no catalog `class` entry) granted an
        // Ultra ceiling via the same zero-rebuild override the `ColdStartPrior`
        // rule reads — isolates that rule from Phase 8's now-declared sol.
        std::env::set_var(api::MODEL_EFFORT_CEILINGS_ENV, r#"{"gpt-99-testonly": "ultra"}"#);
        let _ceiling_guard = EffortCeilingGuard;
        assert!(
            tiers_for_model("gpt-99-testonly").contains(&ModelTier::Deep),
            "an undeclared model's Ultra ceiling still promotes via ColdStartPrior"
        );

        let guard = RemoveOnDrop("ZO_DISABLE_ULTRA_DEEP_PROMOTION");
        std::env::set_var(guard.0, "1");
        // With the promotion disabled, the undeclared Ultra model falls through
        // to the name-token `is_deep_flagship` fallback (no match), landing on
        // the generic frontier grant — Balanced+Strong, no Deep — exactly the
        // pre-Phase-1 behavior.
        assert!(!tiers_for_model("gpt-99-testonly").contains(&ModelTier::Deep));
        assert!(tiers_for_model("gpt-99-testonly").contains(&ModelTier::Strong));
        // Sol's Deep tier comes from its PROVIDER-DECLARED class (Phase 8), a
        // stronger, independent signal this kill switch does not gate — it
        // stays Deep even while the Ultra-ceiling promotion is disabled.
        assert!(tiers_for_model("gpt-5.6-sol").contains(&ModelTier::Deep));
        drop(guard);
        // Restored: the undeclared model's promotion is back on too.
        assert!(tiers_for_model("gpt-99-testonly").contains(&ModelTier::Deep));
    }
}

#[cfg(test)]
mod effort_ceiling_and_context_window_tests {
    use super::{context_window_for_descriptor, effort_ceiling_for_model};
    use crate::model_router::EffortCeiling;

    #[test]
    fn effort_ceiling_bridges_the_api_ssot() {
        assert_eq!(effort_ceiling_for_model("gpt-5.6-sol"), EffortCeiling::Ultra);
        assert_eq!(effort_ceiling_for_model("gpt-5.6-terra"), EffortCeiling::Ultra);
        assert_eq!(effort_ceiling_for_model("gpt-5.6-luna"), EffortCeiling::Max);
        assert_eq!(effort_ceiling_for_model("gpt-5.5-2026-04-23"), EffortCeiling::Xhigh);
        // Anthropic's real ceiling is Max for every model (see
        // `api::max_supported_effort` doc); this axis does not distinguish
        // opus/fable from sonnet/haiku — that split lives in `is_deep_flagship`.
        assert_eq!(effort_ceiling_for_model("claude-opus-4-8"), EffortCeiling::Max);
        assert_eq!(effort_ceiling_for_model("claude-sonnet-5"), EffortCeiling::Max);
        assert_eq!(effort_ceiling_for_model("gemini-3.5-flash"), EffortCeiling::High);
        assert_eq!(effort_ceiling_for_model("deepseek-chat"), EffortCeiling::High);
    }

    #[test]
    fn context_window_bridges_the_api_ssot_for_the_whole_repo_bonus() {
        // SSOT bridge into `model_context_windows.json`. User-directed
        // 2026-07-14: the whole GPT family rides the 258k effective window,
        // so no builtin GPT model qualifies for the Phase 1 WholeRepo >=300k
        // context bonus any more (the bonus path itself stays, exercised by
        // synthetic descriptors in `model_router::tests`).
        assert_eq!(context_window_for_descriptor("gpt-5.6-sol"), Some(258_000));
        assert_eq!(context_window_for_descriptor("gpt-5.5-2026-04-23"), Some(258_000));
        assert!(context_window_for_descriptor("gpt-5.6-sol").unwrap() < 300_000);
    }
}

#[cfg(test)]
mod model_catalog_overlay_tests {
    use super::model_inventory_from_authorized_providers_with_catalog;
    use crate::model_catalog::{CatalogProvider, ModelCatalog};
    use api::ProviderKind;

    #[test]
    fn hidden_builtins_leave_smart_inventory_and_user_rows_do_not_enter_it() {
        let home = std::env::temp_dir().join(format!(
            "zo-model-inventory-catalog-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        catalog
            .add(CatalogProvider::Google, "future-flash-2027", "Future Flash")
            .unwrap();
        let builtin = catalog
            .rows(&[CatalogProvider::Google], false)
            .into_iter()
            .find(|row| row.id == "gemini-3.5-flash")
            .unwrap();
        catalog.delete_or_hide(&builtin).unwrap();

        let inventory = model_inventory_from_authorized_providers_with_catalog(
            "gemini-3.1-pro-preview",
            &[ProviderKind::Google],
            &[],
            Some(&catalog),
        );
        assert!(
            !inventory
                .models()
                .iter()
                .any(|model| model.id() == "gemini-3.5-flash")
        );
        assert!(
            !inventory
                .models()
                .iter()
                .any(|model| model.id() == "future-flash-2027")
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}

/// Golden parity gate: every id in the `api` crate's `MODEL_REGISTRY` (exposed
/// via `api::provider_catalog()`) is snapshotted for (family, class, tiers,
/// `release_rank`, `effort_ceiling`), with
/// every intended flip vs. pre-Phase-1 behavior enumerated explicitly. Any
/// entry not listed as an intended flip is asserted unchanged — a silent
/// classification drift on ANY catalog id fails this test loudly.
///
/// INTENDED FLIPS (pre-Phase-1 -> post-Phase-1), all from two changes: (a)
/// minor-aware `release_rank_for_model`, (b) `effort_ceiling == Ultra` Deep
/// promotion in `tiers_for_model`:
///
/// | id | rank before | rank after | tiers before | tiers after |
/// |---|---|---|---|---|
/// | gpt-5.6-sol | 50 | **56** | [Balanced,Strong] | **[Deep,Strong]** |
/// | gpt-5.6-terra | 50 | **56** | [Balanced,Strong] | **[Deep,Strong]** |
/// | gpt-5.6-luna | 50 | **56** | [Balanced,Strong] | [Balanced,Strong] (unchanged — Max ceiling, not Ultra) |
/// | gpt-5.5-2026-04-23 | 50 | **55** | [Balanced,Strong] | unchanged |
/// | gpt-5.5-fast | 50 | **55** | [Fast,Balanced] | unchanged |
/// | gpt-5.3-codex-spark | 50 | **53** | [Fast,Balanced] (`spark` is a small-model token) | unchanged |
/// | claude-opus-4-8 | 40 | **48** | [Deep,Strong] | unchanged |
/// | claude-haiku-4-5-20251001 | 40 | **45** | [Fast,Balanced] | unchanged |
/// | gemini-3.1-pro-preview (+ aliases gemini-pro, gemini-3.5-pro) | 30 | **31** | [Fast,Balanced] (pre-existing quirk: "gemini" contains "mini") | unchanged |
/// | gemini-3.1-pro-preview-customtools | 30 | **31** | [Fast,Balanced] | unchanged |
/// | gemini-3.5-flash (+ alias gemini-flash) | 30 | **35** | [Fast,Balanced] | unchanged |
/// | gemini-3.1-flash-lite (+ alias gemini-flash-lite) | 30 | **31** | [Fast,Balanced] | unchanged |
///
/// These are cross-family rank deltas (design principle 1's accepted risk:
/// bounded — every delta here is < the release-rank cap of 100, and rank is
/// itself capped below the specialty seed (60) and feedback bound (120) at
/// the scorer level) — none of them change WHICH tier a model is in except
/// the two GPT-5.6 Ultra-ceiling flips above.
///
/// UNCHANGED (no minor segment found, so rank stays major-only; tiers/class
/// unaffected by any Phase 1 change): claude-fable-5 (50), claude-sonnet-5
/// (50), gemini-3-pro-preview (30), gemini-3-flash (30),
/// gemini-3-flash-preview (30), grok-3 (30), ollama (0). class/family for
/// every id is unaffected by Phase 1 (no change to `family_for_model`/
/// `class_for_model`).
///
/// PHASE 8 ADDITIONAL FLIPS (pre-Phase-8 -> post-Phase-8), all from
/// [`declared_tiers_for_model`] now running FIRST — a real provider-stated
/// lineup position beats both the Ultra-ceiling `ColdStartPrior` proxy and
/// every name-token `Fallback` guess. `rank`/`class_for_model`/`family` are
/// untouched by Phase 8 (declared class is a `tiers_for_model`-only input):
///
/// | id | tiers before Phase 8 | tiers after Phase 8 | why |
/// |---|---|---|---|
/// | claude-fable-5 | [Balanced,Strong] | **[Deep,Strong]** | declared frontier (Anthropic: Mythos-class above Opus) |
/// | claude-opus-4-8 | [Deep,Strong] | **[Balanced,Strong]** | declared balanced (superseded flagship; executor per user hierarchy) |
/// | gpt-5.6-sol | [Deep,Strong] | [Deep,Strong] (unchanged — same tiers, provenance now `ProviderDeclared` not `ColdStartPrior`) | declared frontier (Codex model cache: "Latest frontier", priority 1) |
/// | gpt-5.6-terra | [Deep,Strong] | **[Balanced,Strong]** | declared balanced (Codex model cache: "Balanced ... for everyday work", priority 2) — beats its still-Ultra effort ceiling |
/// | gpt-5.6-luna | [Balanced,Strong] | **[Fast,Balanced]** | declared fast (Codex model cache: "Fast and affordable", priority 3) |
/// | gpt-5.5-2026-04-23 (+ alias gpt-5.5) | [Balanced,Strong] | [Balanced,Strong] (unchanged — same tiers, provenance now `ProviderDeclared`) | declared balanced (superseded as frontier by sol) |
/// | gpt-5.5-fast | [Fast,Balanced] | **[Balanced,Strong]** | declared balanced too — `-fast` is OpenAI's priority SERVICE tier of the same-quality gpt-5.5 model (`chatgpt_backend.rs`'s `service_tier: "priority"`), not a smaller/cheaper model; grouping it under gpt-5.5's declared class corrects the pre-Phase-8 `is_small_model` "fast"-substring misclassification, matching every other place in this codebase that already treats `-fast` as a serving-priority signal, not a capability tier |
/// | claude-sonnet-5, claude-haiku-4-5-20251001, gpt-5.3-codex-spark | unchanged | unchanged | declared class (sonnet=balanced, haiku=fast) maps onto the SAME tier vector the undeclared derivation already produced; spark has no declared class (OpenAI's cache does not cover it) |
///
/// Every other catalog id (gemini/deepseek/grok/ollama/...) has no declared
/// class and is untouched by Phase 8.
#[cfg(test)]
mod golden_parity_tests {
    use super::{class_for_model, effort_ceiling_for_model, family_for_model, release_rank_for_model, tiers_for_model};
    use crate::model_router::{EffortCeiling, ModelTier};

    struct Expected {
        id: &'static str,
        family: &'static str,
        class: &'static str,
        rank: u32,
        ceiling: EffortCeiling,
        tiers: &'static [ModelTier],
    }

    #[test]
    fn every_distinct_catalog_id_matches_the_golden_table() {
        use ModelTier::{Balanced, Deep, Fast, Strong};

        let table = [
            // Phase 8: `class` here is `class_for_model`'s free-form marketing-name
            // label (untouched by Phase 8 — see its doc comment) — NOT the same
            // axis as `api::declared_model_class`'s frontier/balanced/fast, which
            // now drives `tiers` for the ids noted below.
            Expected { id: "claude-fable-5-1", family: "claude", class: "fable", rank: 51, ceiling: EffortCeiling::Max, tiers: &[Deep, Strong] }, // declared frontier (5.1 heads the family)
            Expected { id: "claude-fable-5", family: "claude", class: "fable", rank: 50, ceiling: EffortCeiling::Max, tiers: &[Deep, Strong] }, // declared frontier
            Expected { id: "claude-opus-5", family: "claude", class: "opus", rank: 50, ceiling: EffortCeiling::Max, tiers: &[Balanced, Strong] }, // declared balanced
            Expected { id: "claude-opus-4-8", family: "claude", class: "opus", rank: 48, ceiling: EffortCeiling::Max, tiers: &[Balanced, Strong] }, // declared balanced
            Expected { id: "claude-sonnet-5", family: "claude", class: "sonnet", rank: 50, ceiling: EffortCeiling::Max, tiers: &[Balanced, Strong] }, // declared balanced (no flip)
            Expected { id: "claude-haiku-4-5-20251001", family: "claude", class: "balanced", rank: 45, ceiling: EffortCeiling::Max, tiers: &[Fast, Balanced] }, // declared fast (no flip)
            Expected { id: "gpt-5.6-sol", family: "gpt", class: "strong", rank: 56, ceiling: EffortCeiling::Ultra, tiers: &[Deep, Strong] }, // declared frontier (no flip; provenance changes)
            Expected { id: "gpt-5.6-terra", family: "gpt", class: "strong", rank: 56, ceiling: EffortCeiling::Ultra, tiers: &[Balanced, Strong] }, // declared balanced — LOSES Deep
            Expected { id: "gpt-5.6-luna", family: "gpt", class: "strong", rank: 56, ceiling: EffortCeiling::Max, tiers: &[Fast, Balanced] }, // declared fast — GAINS Fast, loses Strong
            // gpt-5.5 계열은 카탈로그 퇴역(2026-07-11, terra 대체). 세션이 들고
            // 있는 legacy id·`[fast]` 브래킷 id(fast on 시 CurrentMainModel로
            // 인벤토리에 들어옴)의 분류는 strip_service_tier_suffix 정규화가
            // bare 기준으로 처리한다 — 카탈로그 행은 없다.
            Expected { id: "gpt-5.3-codex-spark", family: "gpt", class: "fast", rank: 53, ceiling: EffortCeiling::Xhigh, tiers: &[Fast, Balanced] }, // no declared class (unchanged)
            // Pre-existing quirk (unrelated to Phase 1, left as-is — out of
            // this phase's scope): "gemini" itself contains the substring
            // "mini", which both `is_small_model` and `class_for_model` treat
            // as the small/fast-model token. So every Gemini id — pro
            // variants included — is classified small: class "fast",
            // tiers [Fast,Balanced], never reaching the `is_deep_flagship`
            // "pro" check at all.
            Expected { id: "gemini-3.1-pro-preview", family: "gemini", class: "fast", rank: 31, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "gemini-3.1-pro-preview-customtools", family: "gemini", class: "fast", rank: 31, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "gemini-3-pro-preview", family: "gemini", class: "fast", rank: 30, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "gemini-3.5-flash", family: "gemini", class: "fast", rank: 35, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "gemini-3.6-flash", family: "gemini", class: "fast", rank: 36, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "gemini-3-flash", family: "gemini", class: "fast", rank: 30, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "gemini-3-flash-preview", family: "gemini", class: "fast", rank: 30, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "gemini-3.1-flash-lite", family: "gemini", class: "fast", rank: 31, ceiling: EffortCeiling::High, tiers: &[Fast, Balanced] },
            Expected { id: "grok-3", family: "grok", class: "balanced", rank: 30, ceiling: EffortCeiling::High, tiers: &[Balanced, Strong] },
            // Pre-existing quirk (unrelated to Phase 1): "ollama" contains the
            // substring "llama", which `family_for_model` checks first, and
            // `is_frontier_family`'s "llama" token also matches it — so its
            // family is "llama" (not "ollama") and it clears the frontier gate
            // (Balanced,Strong), though `descriptor_for_catalog_entry` still
            // overrides its capabilities/source for the special-cased local id.
            Expected { id: "ollama", family: "llama", class: "balanced", rank: 0, ceiling: EffortCeiling::High, tiers: &[Balanced, Strong] },
        ];

        // Every distinct `canonical_model_id` in the live catalog must appear
        // in the table exactly once — this fails loudly if a future catalog
        // addition/removal is not reflected here.
        let mut catalog_ids: Vec<&str> = api::provider_catalog()
            .iter()
            .map(|entry| entry.canonical_model_id)
            .collect();
        catalog_ids.sort_unstable();
        catalog_ids.dedup();
        let mut table_ids: Vec<&str> = table.iter().map(|expected| expected.id).collect();
        table_ids.sort_unstable();
        assert_eq!(
            catalog_ids, table_ids,
            "golden table is out of sync with api::provider_catalog(); update both together"
        );

        for expected in &table {
            let id = expected.id;
            assert_eq!(family_for_model(id), expected.family, "family drifted for {id}");
            assert_eq!(class_for_model(id), expected.class, "class drifted for {id}");
            assert_eq!(release_rank_for_model(id), expected.rank, "rank drifted for {id}");
            assert_eq!(
                effort_ceiling_for_model(id),
                expected.ceiling,
                "effort ceiling drifted for {id}"
            );
            let (tiers, _provenance) = tiers_for_model(id);
            for tier in expected.tiers {
                assert!(tiers.contains(tier), "{id} missing expected tier {tier:?}");
            }
            assert_eq!(
                tiers.len(),
                expected.tiers.len(),
                "{id} has unexpected extra/missing tiers: {tiers:?} vs {:?}",
                expected.tiers
            );
        }
    }
}
