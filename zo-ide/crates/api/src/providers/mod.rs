use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::{ContentBlockDelta, EffortLevel, StreamEvent};
use anthropic::keychain::KeychainAnswer;

pub mod anthropic;
pub(crate) mod aws_sigv4;
pub mod chatgpt_backend;
pub mod cli_sessions;
pub(crate) mod cloud_gateway;
pub mod gemini_code_assist;
pub(crate) mod google_auth;
pub mod openai_compat;
pub mod openai_oauth;
pub(crate) mod refresh_gate;
mod retry_backoff;

pub const EXPERIMENTAL_PROVIDERS_ENV: &str = "ZO_EXPERIMENTAL_PROVIDERS";
pub const NON_CLAUDE_ADAPTERS_ENV: &str = "ZO_EXPERIMENTAL_PROVIDER_ADAPTERS";

/// Env var containing a JSON model-context capability catalog. This is an
/// operational escape hatch for providers whose `/models` endpoint does not
/// expose context-window metadata; it lets deployments update model limits
/// without recompiling. Shape: `{ "models": [{ "ids": ["model"],
/// "context_window": 1000000 }] }`.
pub const MODEL_CONTEXT_WINDOWS_ENV: &str = "ZO_MODEL_CONTEXT_WINDOWS";

const BUILTIN_MODEL_CONTEXT_WINDOWS_JSON: &str = include_str!("model_context_windows.json");

#[derive(Debug, Default, Deserialize)]
struct ModelContextCatalog {
    #[serde(default)]
    models: Vec<ModelContextEntry>,
    /// Alias → canonical id rows. Order is meaningful: the first row that names
    /// a token wins, and `latest_model_for_provider` returns a provider's first
    /// row, so this array IS the catalog's precedence.
    #[serde(default)]
    aliases: Vec<AliasRow>,
    /// The router's cold-start name-token tables — see [`RouterPriors`].
    #[serde(default)]
    priors: RouterPriors,
}

/// The router's cold-start priors: name tokens that place a model no provider
/// has positioned. Read only after a declared `class` and the effort ceiling
/// have had their say, and recorded as a fallback wherever they fire — they
/// are guesses, kept as catalog data so a new family or size word is a
/// catalog edit, not a rebuild. Each field is layered on its own: the first
/// published layer that fills it wins, and [`router_priors`] fills whatever
/// stays empty from the shipped catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RouterPriors {
    /// Size words of a provider's cheap tier (`haiku`, `mini`, `flash`, …).
    pub small_tokens: Vec<String>,
    /// A parameter count in the id (`8b`) below this many billions reads as small.
    pub small_parameter_billion_below: u32,
    /// Lineage words of the frontier families the router treats as full
    /// specialists (`claude`, `gpt`, `gemini`, …).
    pub frontier_family_tokens: Vec<String>,
    /// Words of a family's heaviest reasoning line (`opus`, `pro`, `reasoner`, …).
    pub deep_flagship_tokens: Vec<String>,
    /// The router's cold-start specialty seed: for each route role key
    /// (`coding`, `analysis`, …) the lineage families that get the seed's
    /// small preference before any verified outcome speaks. A table the
    /// learned specialty score retires as its confidence ramps; kept as
    /// catalog data so a preference is an edit, never a rebuild.
    pub specialty_seed: BTreeMap<String, Vec<String>>,
    /// The plan scorer's cold-start figures — see [`PlanPriorsTable`].
    pub plan: PlanPriorsTable,
}

/// The figures the plan scorer (`runtime::score_plan`) falls back to before
/// the verified record speaks: a first-try pass rate per band, the size of a
/// work turn per complexity, and the fixed costs of judging, checking,
/// classifying, briefing a lane and handing a conversation to another model.
/// Every figure is a prior — the outcome store replaces it as evidence
/// accumulates — and lives here so that a prior is a catalog row, never a
/// literal in the scorer. Integers only (percent, tokens, milliseconds) so the
/// section stays `Eq` with the rest of the priors; the JSON `source` says
/// which figures were measured and which are estimates awaiting a measurement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanPriorsTable {
    /// First-try verified pass rate, percent, by band.
    pub pass_percent_top: u8,
    pub pass_percent_second: u8,
    pub pass_percent_rest: u8,
    /// One model-judge verification: its output and wall time.
    pub judge_output_tokens: u64,
    pub judge_duration_ms: u64,
    /// One objective check's wall time.
    pub objective_check_ms: u64,
    /// One classification call (probe or decomposition).
    pub classify_output_tokens: u64,
    pub classify_ms: u64,
    /// The brief a lane or delegate reads instead of the parent's context.
    pub lane_brief_tokens: u64,
    /// The handoff note a switched-to model reads on top of the context.
    pub handover_tokens: u64,
    /// One work turn per complexity label (`trivial`, `small`, `medium`,
    /// `large`, `unknown` — the lowercase labels the outcome store writes).
    pub by_complexity: BTreeMap<String, WorkTurnPrior>,
}

/// The size of one work turn at a complexity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkTurnPrior {
    pub output_tokens: u64,
    pub requests: u32,
    pub duration_ms: u64,
}

impl PlanPriorsTable {
    /// Whether any figure is set — an absent section deserializes to all-zero.
    #[must_use]
    pub fn declares_anything(&self) -> bool {
        *self != Self::default()
    }

    /// Take `fallback`'s value for every figure this one leaves at zero, and
    /// every complexity label this one does not list.
    pub fn fill_from(&mut self, fallback: &Self) {
        macro_rules! fill_zero {
            ($($field:ident),* $(,)?) => {$(
                if self.$field == 0 {
                    self.$field = fallback.$field;
                }
            )*};
        }
        fill_zero!(
            pass_percent_top,
            pass_percent_second,
            pass_percent_rest,
            judge_output_tokens,
            judge_duration_ms,
            objective_check_ms,
            classify_output_tokens,
            classify_ms,
            lane_brief_tokens,
            handover_tokens,
        );
        for (label, prior) in &fallback.by_complexity {
            self.by_complexity
                .entry(label.clone())
                .or_insert(*prior);
        }
    }

    /// The work-turn prior for a complexity label, when the table lists it.
    #[must_use]
    pub fn work_turn(&self, complexity_label: &str) -> Option<WorkTurnPrior> {
        self.by_complexity.get(complexity_label).copied()
    }
}

/// The plan scorer's priors in force — the `plan` section of [`router_priors`].
#[must_use]
pub fn plan_priors() -> &'static PlanPriorsTable {
    &router_priors().plan
}

impl RouterPriors {
    /// Whether any field is set — an absent section deserializes to all-empty.
    #[must_use]
    pub fn declares_anything(&self) -> bool {
        !self.small_tokens.is_empty()
            || self.small_parameter_billion_below != 0
            || !self.frontier_family_tokens.is_empty()
            || !self.deep_flagship_tokens.is_empty()
            || !self.specialty_seed.is_empty()
            || self.plan.declares_anything()
    }

    /// The families the cold-start seed prefers for a route role key; empty
    /// when the table names none.
    #[must_use]
    pub fn specialty_families(&self, role_key: &str) -> &[String] {
        self.specialty_seed
            .get(role_key)
            .map_or(&[], Vec::as_slice)
    }

    /// Take `fallback`'s value for every field this one leaves empty. Applied
    /// layer after layer in precedence order, so the first layer to fill a
    /// field keeps it.
    pub fn fill_from(&mut self, fallback: &Self) {
        if self.small_tokens.is_empty() {
            self.small_tokens.clone_from(&fallback.small_tokens);
        }
        if self.small_parameter_billion_below == 0 {
            self.small_parameter_billion_below = fallback.small_parameter_billion_below;
        }
        if self.frontier_family_tokens.is_empty() {
            self.frontier_family_tokens
                .clone_from(&fallback.frontier_family_tokens);
        }
        if self.deep_flagship_tokens.is_empty() {
            self.deep_flagship_tokens.clone_from(&fallback.deep_flagship_tokens);
        }
        if self.specialty_seed.is_empty() {
            self.specialty_seed.clone_from(&fallback.specialty_seed);
        }
        self.plan.fill_from(&fallback.plan);
    }

    /// The priors an override document declares, if it declares any.
    fn declared_in(raw: &str) -> Option<Self> {
        serde_json::from_str::<ModelContextCatalog>(raw)
            .ok()
            .map(|catalog| catalog.priors)
            .filter(Self::declares_anything)
    }
}

/// The priors the last override publish declared, already filled from the
/// shipped catalog; `None` until a publish declares some.
static ROUTER_PRIORS_STORE: RwLock<Option<&'static RouterPriors>> = RwLock::new(None);

/// The router's cold-start priors in force: the last published override's,
/// each empty field filled from the shipped catalog — or the shipped section
/// alone when no override declares any. Cheap enough to call per model: the
/// answer is built once per publish, not per call.
#[must_use]
pub fn router_priors() -> &'static RouterPriors {
    if let Some(priors) = *ROUTER_PRIORS_STORE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return priors;
    }
    &builtin_model_context_catalog().priors
}

fn install_router_priors(raw: &str) {
    let installed = RouterPriors::declared_in(raw).map(|mut priors| {
        priors.fill_from(&builtin_model_context_catalog().priors);
        &*Box::leak(Box::new(priors))
    });
    *ROUTER_PRIORS_STORE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = installed;
}

/// An on-disk alias field that is either a single alias or an ordered list of
/// them, so the catalog can spell `"opus"` or `["opus", "openai-latest"]` for
/// the same key. [`Self::into_vec`] normalises both to the list the code reads.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum AliasList {
    One(String),
    Many(Vec<String>),
}

impl AliasList {
    fn to_vec(&self) -> Vec<String> {
        match self {
            Self::One(alias) => vec![alias.clone()],
            Self::Many(aliases) => aliases.clone(),
        }
    }
}

/// An optional string-or-list alias field as the list the code reads: absent →
/// empty, string → one, array → many. Trims each entry, dropping blanks.
fn clone_alias_list(field: Option<&AliasList>) -> Vec<String> {
    field
        .map(AliasList::to_vec)
        .unwrap_or_default()
        .into_iter()
        .map(|alias| alias.trim().to_string())
        .filter(|alias| !alias.is_empty())
        .collect()
}

/// One alias row as the catalog file states it. Kept separate from
/// [`ProviderCatalogEntry`] because that type is what the rest of the codebase
/// consumes (`&'static str` fields, provider metadata helpers) while this is
/// only the on-disk shape.
#[derive(Debug, Deserialize)]
struct AliasRow {
    alias: String,
    canonical: String,
    provider: String,
    #[serde(default)]
    orchestration_rank: Option<u8>,
    /// One-step fallback alias when this model is starved by rate limits.
    /// Absent means "this is the bottom rung" — see [`starvation_demotion_model`].
    #[serde(default)]
    demotes_to: Option<String>,
    /// Where a safety-classifier refusal on this lineup goes next: one alias or
    /// an ordered list of candidates. Absent means a refusal is surfaced (after
    /// one same-model retry) — see [`refusal_fallback_candidates`].
    #[serde(default)]
    refusal_fallback: Option<AliasList>,
}

fn provider_kind_from_key(key: &str) -> Option<ProviderKind> {
    match key.trim().to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => Some(ProviderKind::Anthropic),
        "openai" | "chatgpt" | "codex" => Some(ProviderKind::OpenAi),
        "google" | "gemini" => Some(ProviderKind::Google),
        "xai" | "grok" => Some(ProviderKind::Xai),
        "ollama" => Some(ProviderKind::Ollama),
        _ => None,
    }
}

/// On-the-wire model id(s) for an entry, when the id a user selects is not the
/// id the provider serves.
///
/// Gemini is the case that forces this: the Antigravity backend serves
/// `gemini-3.6-flash-low|-medium|-high` and has no `gemini-3.6-flash` at all,
/// so the reasoning tier is part of the model id rather than a separate field.
/// Expressing that as data keeps a new release a catalog edit instead of a code
/// change — the same rule the rest of this catalog already follows.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireIds {
    /// One id for every effort. The common case: the selection id and the wire
    /// id differ, but the model does not tier.
    Fixed(String),
    /// Distinct ids per reasoning tier. A tier the provider does not publish is
    /// left out rather than filled in — see [`WireIds::resolve`] for how an
    /// absent tier is bridged.
    ByEffort {
        #[serde(default)]
        low: Option<String>,
        #[serde(default)]
        medium: Option<String>,
        #[serde(default)]
        high: Option<String>,
    },
}

fn non_blank_wire_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

impl WireIds {
    /// The wire id to send for `effort`.
    ///
    /// An absent tier resolves UP first and only then down. Up-first matters:
    /// Gemini Pro publishes `low` and `high` with no middle rung, and a medium
    /// request there is a request for more thinking than low — resolving it
    /// down would quietly serve less reasoning than the caller asked for, which
    /// is the failure that cannot be seen from the outside. Resolving down is
    /// still the last resort so a model that publishes only `low` stays usable.
    fn resolve(&self, effort: EffortLevel) -> Option<String> {
        match self {
            Self::Fixed(id) => non_blank_wire_id(id),
            Self::ByEffort { low, medium, high } => {
                let rungs = [low.as_deref(), medium.as_deref(), high.as_deref()];
                let tier = match effort {
                    EffortLevel::Low => 0,
                    EffortLevel::Medium => 1,
                    EffortLevel::High
                    | EffortLevel::Xhigh
                    | EffortLevel::Max
                    | EffortLevel::Ultra => 2,
                };
                rungs[tier]
                    .or_else(|| rungs[tier + 1..].iter().copied().flatten().next())
                    .or_else(|| rungs[..tier].iter().rev().copied().flatten().next())
                    .and_then(non_blank_wire_id)
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct ModelContextEntry {
    #[serde(default)]
    ids: Vec<String>,
    /// Declared context window in tokens. Defaults to 0 — "undeclared" — so an
    /// entry may carry only a `wire` mapping without inventing a token limit
    /// the provider never published. [`ModelContextCatalog::context_window_for`]
    /// already filters 0 out, so an undeclared window stays absent rather than
    /// becoming a zero-token ceiling.
    #[serde(default)]
    context_window: u64,
    #[serde(default)]
    max_output_tokens: Option<u64>,
    /// The id(s) this model is actually served under, when they differ from the
    /// id used to select it. Absent means the selection id IS the wire id.
    #[serde(default)]
    wire: Option<WireIds>,
    /// Maximum serialized request BODY size in bytes the endpoint accepts, when
    /// the provider documents one. A different ceiling from `context_window`:
    /// bytes, not tokens. A request can sit well inside the token budget and
    /// still be refused with HTTP 413 — images and pasted binaries cost many
    /// bytes per token, so an attachment-heavy turn hits this first.
    ///
    /// Absent for every model whose provider has not published a figure, and
    /// there is deliberately NO built-in default: an invented limit would
    /// either reject valid requests or fail to catch real ones. When absent the
    /// preflight simply does not run and the reactive 413 path handles it.
    #[serde(default)]
    max_request_bytes: Option<u64>,
    /// Provider-declared positioning class (`"frontier"|"balanced"|"fast"`),
    /// when the provider (or an operator, via [`MODEL_CLASSES_ENV`]) has
    /// actually stated one. Absent for every entry whose provider has not
    /// published a positioning signal (Gemini/DeepSeek/Grok/etc. today) —
    /// see [`declared_model_class`]'s doc for the no-invented-quality-tables
    /// rule this enforces. Provenance for the declaration itself lives in
    /// this entry's existing `source` string rather than a parallel field:
    /// each entry already carries exactly one context-window/class fact set,
    /// so one provenance string covers both.
    #[serde(default)]
    class: Option<String>,
    /// The lineup family a family alias follows (`fable`, `sol`, `flash`),
    /// when the id's own grammar would not say. Absent means
    /// [`family_from_id`] derives it.
    #[serde(default)]
    family: Option<String>,
    /// The human name, when it is not what [`display_name_from_id`] spells.
    #[serde(default)]
    display_name: Option<String>,
    /// The reasoning-effort levels the provider declares this model accepts
    /// (`low|medium|high|xhigh|max|ultra`). Empty means undeclared — the
    /// provider-wide set of [`model_accepts_effort`] applies.
    #[serde(default)]
    effort_levels: Vec<String>,
    /// Priority-serving tiers the provider declares (Codex
    /// `additional_speed_tiers`); `fast` here is what makes `/fast` real.
    #[serde(default)]
    speed_tiers: Vec<String>,
    /// The base/priority spelling pair when it is not `<id>` / `<id>[fast]`.
    #[serde(default)]
    fast_pair: Option<FastPair>,
    /// Capability facts (`imagegen`, `streaming_only`) read by
    /// [`model_has_capability`]: a tool only one lineup can serve keys off
    /// these instead of naming the lineup.
    #[serde(default)]
    capabilities: Vec<String>,
    /// One-step rate-limit fallback for an id that is not a family alias's
    /// canonical (a retired lineup); alias rows carry theirs on the alias.
    #[serde(default)]
    demotes_to: Option<String>,
    /// Where a safety-classifier refusal on this id goes next (one alias or an
    /// ordered list), for an id that is not a family alias's canonical; alias
    /// rows carry theirs on the alias.
    #[serde(default)]
    refusal_fallback: Option<AliasList>,
    /// Extended OpenAI prompt-cache retention the provider verified for this
    /// model (`24h`), or absent when only the routing key may be sent.
    #[serde(default)]
    prompt_cache_retention: Option<String>,
}

/// A release-specific priority-tier spelling (`gpt-5.5` / `gpt-5.5-fast`).
#[derive(Debug, Clone, Deserialize)]
struct FastPair {
    base: String,
    fast: String,
}

impl ModelContextCatalog {
    fn find_entry(&self, raw_model: &str, canonical_model: &str) -> Option<&ModelContextEntry> {
        let raw = raw_model.trim().to_ascii_lowercase();
        let canonical = canonical_model.trim().to_ascii_lowercase();

        // Prefer exact ids, then the longest segment-delimited family prefix.
        // Specificity matters because the catalog contains both `gpt` and
        // narrower families such as `gpt-5.6-sol`; a generic entry must not
        // shadow a dated/qualified member of the narrower family.
        if let Some(exact) = self.exact_entry(raw_model, canonical_model) {
            return Some(exact);
        }

        let mut best = None;
        for entry in &self.models {
            for id in &entry.ids {
                let id = id.trim().to_ascii_lowercase();
                // Bare aliases such as `gpt` are exact-only: treating them as
                // families would fabricate 258k/128k capabilities for every
                // unknown future GPT id. Versioned/model-specific ids retain
                // segment-delimited suffix matching.
                if id != "gpt"
                    && (model_id_matches_prefix_segment(&raw, &id)
                        || model_id_matches_prefix_segment(&canonical, &id))
                {
                    let specificity = id.len();
                    if best.is_none_or(|(best_specificity, _)| specificity > best_specificity) {
                        best = Some((specificity, entry));
                    }
                }
            }
        }
        best.map(|(_, entry)| entry)
    }

    fn context_window_for(&self, raw_model: &str, canonical_model: &str) -> Option<u64> {
        self.find_entry(raw_model, canonical_model)
            .map(|entry| entry.context_window)
            .filter(|&cw| cw > 0)
    }

    /// The docs-verified per-model synchronous output cap, when the catalog
    /// declares one. Previously dropped on the floor — `ModelContextEntry` only
    /// read `context_window`, so a non-Anthropic model's real cap (GPT-5.5 128k,
    /// `DeepSeek` V4 384k) never reached the wire and every such model was pinned
    /// to the conservative 64k default.
    fn max_output_tokens_for(&self, raw_model: &str, canonical_model: &str) -> Option<u64> {
        self.find_entry(raw_model, canonical_model)
            .and_then(|entry| entry.max_output_tokens)
    }

    /// The declared request-body byte ceiling, via the same matcher every other
    /// field on this entry uses.
    fn max_request_bytes_for(&self, raw_model: &str, canonical_model: &str) -> Option<u64> {
        self.find_entry(raw_model, canonical_model)
            .and_then(|entry| entry.max_request_bytes)
    }

    /// The catalog's declared [`ModelClass`] for a model, via the exact same
    /// matcher [`Self::context_window_for`] uses (exact id first, then
    /// longest segment-delimited family prefix) — so a dated/`@`/`[`-suffixed
    /// id (`gpt-5.6-sol-2026-07-09`) resolves its declared class through the
    /// identical machinery as its context window.
    fn class_for(&self, raw_model: &str, canonical_model: &str) -> Option<ModelClass> {
        self.find_entry(raw_model, canonical_model)
            .and_then(|entry| entry.class.as_deref())
            .and_then(parse_model_class)
    }

    /// The entry naming `raw_model` or `canonical_model` exactly — an identity
    /// lookup, as opposed to [`Self::find_entry`]'s family inheritance.
    fn exact_entry(&self, raw_model: &str, canonical_model: &str) -> Option<&ModelContextEntry> {
        let raw = raw_model.trim().to_ascii_lowercase();
        let canonical = canonical_model.trim().to_ascii_lowercase();
        self.models.iter().find(|entry| {
            entry.ids.iter().any(|id| {
                let id = id.trim().to_ascii_lowercase();
                id == raw || id == canonical
            })
        })
    }

    /// The catalog id `raw_model`/`canonical_model` is a qualified member of
    /// (`gpt-5.6-sol-2026-07-09` → `gpt-5.6-sol`), or `None` when the id is
    /// itself a catalog id or matches no family — the family half of
    /// [`Self::find_entry`], answered as an id rather than an entry.
    fn family_prefix_id_for(&self, raw_model: &str, canonical_model: &str) -> Option<String> {
        if self.exact_entry(raw_model, canonical_model).is_some() {
            return None;
        }
        let raw = raw_model.trim().to_ascii_lowercase();
        let canonical = canonical_model.trim().to_ascii_lowercase();
        let mut best: Option<String> = None;
        for entry in &self.models {
            for id in &entry.ids {
                let id = id.trim().to_ascii_lowercase();
                if id != "gpt"
                    && (model_id_matches_prefix_segment(&raw, &id)
                        || model_id_matches_prefix_segment(&canonical, &id))
                    && best.as_ref().is_none_or(|current| id.len() > current.len())
                {
                    best = Some(id);
                }
            }
        }
        best
    }

    /// The declared lineup family, inherited like every family property.
    fn family_for(&self, raw_model: &str, canonical_model: &str) -> Option<String> {
        self.find_entry(raw_model, canonical_model)
            .and_then(|entry| entry.family.clone())
    }

    /// The declared human name. Exact ids only, like [`Self::wire_for`]: a
    /// name is an identity, and a dated sibling has its own.
    fn display_name_for(&self, raw_model: &str, canonical_model: &str) -> Option<String> {
        self.exact_entry(raw_model, canonical_model)
            .and_then(|entry| entry.display_name.clone())
    }

    /// The declared accepted effort levels; `None` when the row declares none.
    fn effort_levels_for(&self, raw_model: &str, canonical_model: &str) -> Option<Vec<EffortLevel>> {
        self.find_entry(raw_model, canonical_model)
            .map(|entry| {
                entry
                    .effort_levels
                    .iter()
                    .filter_map(|level| parse_effort_level(level))
                    .collect::<Vec<_>>()
            })
            .filter(|levels| !levels.is_empty())
    }

    /// The declared priority-serving tiers; `None` when the row declares none.
    fn speed_tiers_for(&self, raw_model: &str, canonical_model: &str) -> Option<Vec<String>> {
        self.find_entry(raw_model, canonical_model)
            .map(|entry| entry.speed_tiers.clone())
            .filter(|tiers| !tiers.is_empty())
    }

    fn fast_pair_for(&self, raw_model: &str, canonical_model: &str) -> Option<(String, String)> {
        self.find_entry(raw_model, canonical_model)
            .and_then(|entry| entry.fast_pair.as_ref())
            .map(|pair| (pair.base.clone(), pair.fast.clone()))
    }

    /// The declared capability facts; `None` when the row declares none.
    fn capabilities_for(&self, raw_model: &str, canonical_model: &str) -> Option<Vec<String>> {
        self.find_entry(raw_model, canonical_model)
            .map(|entry| entry.capabilities.clone())
            .filter(|capabilities| !capabilities.is_empty())
    }

    fn demotes_to_for(&self, raw_model: &str, canonical_model: &str) -> Option<String> {
        self.find_entry(raw_model, canonical_model)
            .and_then(|entry| entry.demotes_to.clone())
    }

    fn refusal_fallback_for(&self, raw_model: &str, canonical_model: &str) -> Vec<String> {
        self.find_entry(raw_model, canonical_model)
            .map(|entry| clone_alias_list(entry.refusal_fallback.as_ref()))
            .unwrap_or_default()
    }

    fn prompt_cache_retention_for(&self, raw_model: &str, canonical_model: &str) -> Option<String> {
        self.find_entry(raw_model, canonical_model)
            .and_then(|entry| entry.prompt_cache_retention.clone())
    }

    /// The declared wire id for a model at `effort`.
    ///
    /// Exact ids only — deliberately NOT [`Self::find_entry`]'s family-prefix
    /// matcher. Every other field here is a family property that a qualified
    /// sibling legitimately inherits (a dated `gpt-5.6-sol-2026-07-09` has its
    /// family's context window). A wire id is the opposite: it is an identity,
    /// and a sibling that merely shares a prefix is a DIFFERENT served model.
    /// Inheriting one would route `gemini-3.6-flash-image` to the text model's
    /// id and quietly answer an image request with prose.
    fn wire_for(
        &self,
        raw_model: &str,
        canonical_model: &str,
        effort: EffortLevel,
    ) -> Option<String> {
        self.exact_entry(raw_model, canonical_model)
            .and_then(|entry| entry.wire.as_ref())
            .and_then(|wire| wire.resolve(effort))
    }
}

/// Provider-declared positioning class for a model — the ONLY static
/// quality-like signal this codebase permits (design principle: no invented
/// quality tables; only provider-declared capability facts or explicitly
/// labeled cold-start priors). A model with no declared class returns `None`;
/// callers fall back to their own capability-derived heuristics rather than
/// guessing a class.
///
/// This is a distinct axis from `ModelDescriptor`-style marketing-name
/// classification (`opus`/`sonnet`/`fast`/... — see `model_inventory::
/// class_for_model` in the `runtime` crate): that string is a free-form
/// family/flavor label used for `RoleSelector` matching, while `ModelClass`
/// is the provider's own stated POSITIONING within its lineup (frontier vs.
/// balanced vs. fast), used to seed tier precedence. The two are not merged —
/// see `class_for_model`'s doc comment for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelClass {
    /// Provider states this is (one of) their top/latest agentic model(s) —
    /// e.g. OpenAI's Codex model cache "Latest frontier agentic coding
    /// model", or Anthropic's public Mythos-class-above-Opus positioning.
    Frontier,
    /// Provider states this is their everyday/mid-tier model.
    Balanced,
    /// Provider states this is their fast/affordable model.
    Fast,
}

fn parse_model_class(value: &str) -> Option<ModelClass> {
    match value.trim().to_ascii_lowercase().as_str() {
        "frontier" => Some(ModelClass::Frontier),
        "balanced" => Some(ModelClass::Balanced),
        "fast" => Some(ModelClass::Fast),
        _ => None,
    }
}

/// Env var containing a JSON object of `{"model-or-family-prefix":
/// "frontier|balanced|fast"}` declared-class overrides, consulted before the
/// catalog in [`declared_model_class`] — the same zero-rebuild escape hatch
/// pattern as [`MODEL_EFFORT_CEILINGS_ENV`] (longest matching prefix wins,
/// segment-boundary aware via `crate::types::model_id_matches_family`).
pub const MODEL_CLASSES_ENV: &str = "ZO_MODEL_CLASSES";

/// Read fresh (not cached) so tests can set/unset the env var per-case —
/// mirrors [`env_effort_ceiling_override`].
fn env_model_class_override(canonical_lower: &str) -> Option<ModelClass> {
    let raw = std::env::var(MODEL_CLASSES_ENV).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let overrides: std::collections::HashMap<String, String> = serde_json::from_str(&raw).ok()?;
    let mut best: Option<(usize, ModelClass)> = None;
    for (prefix, value) in &overrides {
        let prefix_lower = prefix.trim().to_ascii_lowercase();
        if prefix_lower.is_empty() {
            continue;
        }
        if !crate::types::model_id_matches_family(canonical_lower, &prefix_lower) {
            continue;
        }
        let Some(class) = parse_model_class(value) else {
            continue;
        };
        if best.is_none_or(|(best_len, _)| prefix_lower.len() > best_len) {
            best = Some((prefix_lower.len(), class));
        }
    }
    best.map(|(_, class)| class)
}

/// Catalog-declared class lookup, mirroring [`catalog_max_output_tokens`]'s
/// env-catalog-then-builtin resolution order.
fn catalog_model_class(raw_model: &str, canonical_model: &str) -> Option<ModelClass> {
    catalog_fact(|catalog| catalog.class_for(raw_model, canonical_model))
}

/// Single source of truth for a model's provider-declared [`ModelClass`],
/// when one exists. Resolution order: [`MODEL_CLASSES_ENV`] override
/// (longest matching prefix wins) → [`MODEL_CONTEXT_WINDOWS_ENV`] catalog
/// override (if it happens to carry a `class` field) → the built-in
/// `model_context_windows.json` catalog. Aliases resolve through
/// [`resolve_model_alias`] first, then dated/`@`/`[`-suffixed ids resolve
/// through the same family-prefix matcher [`context_window_for_model`] uses.
///
/// Returns `None` for every model the provider (and no operator override) has
/// not declared a position for — e.g. Gemini/DeepSeek/Grok today. Callers
/// MUST treat `None` as "undeclared", not as any particular class; they fall
/// back to their own capability-derived heuristics (design principle:
/// no-hardcoding end-state — only provider-declared facts or labeled
/// cold-start priors are allowed as static data).
#[must_use]
pub fn declared_model_class(model: &str) -> Option<ModelClass> {
    let canonical = resolve_model_alias(model);
    let canonical_lower = canonical.to_ascii_lowercase();
    if let Some(class) = env_model_class_override(&canonical_lower) {
        return Some(class);
    }
    catalog_model_class(model, &canonical)
}

fn builtin_model_context_catalog() -> &'static ModelContextCatalog {
    static CATALOG: OnceLock<ModelContextCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(BUILTIN_MODEL_CONTEXT_WINDOWS_JSON).unwrap_or_else(|error| {
            eprintln!("[zo] built-in model context catalog is invalid: {error}");
            ModelContextCatalog::default()
        })
    })
}

/// One model fact, resolved the way every catalog fact is: the
/// [`MODEL_CONTEXT_WINDOWS_ENV`] override catalog first (which is how a
/// settings-declared or discovered model reaches this crate), then the
/// built-in [`model_context_windows.json`]. The override is read fresh, not
/// cached, so a test can set and unset it per case.
fn catalog_fact<T>(pick: impl Fn(&ModelContextCatalog) -> Option<T>) -> Option<T> {
    if let Ok(raw) = std::env::var(MODEL_CONTEXT_WINDOWS_ENV) {
        if !raw.trim().is_empty() {
            if let Ok(catalog) = serde_json::from_str::<ModelContextCatalog>(&raw) {
                if let Some(fact) = pick(&catalog) {
                    return Some(fact);
                }
            }
        }
    }
    pick(builtin_model_context_catalog())
}

/// The catalog's declared family for `model`, when a row states one.
#[must_use]
pub fn declared_model_family(model: &str) -> Option<String> {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    catalog_fact(|catalog| catalog.family_for(model, &lower))
}

/// The lineup family `model` belongs to — the token a family alias follows
/// (`fable`, `sol`, `astra`, `flash-lite`): the catalog's declaration, else
/// [`family_from_id`] on the canonical id. `None` for an id no first-party
/// lineup grammar covers.
#[must_use]
pub fn model_family(model: &str) -> Option<String> {
    let canonical = resolve_catalog_alias(model);
    declared_model_family(&canonical).or_else(|| {
        catalog_provider_of(&canonical).and_then(|provider| family_from_id(provider, &canonical))
    })
}

/// The human name for `model`: the catalog's declaration, else
/// [`display_name_from_id`] on the canonical id, else the id itself.
#[must_use]
pub fn model_display_name(model: &str) -> String {
    let canonical = resolve_catalog_alias(model);
    let lower = canonical.to_ascii_lowercase();
    catalog_fact(|catalog| catalog.display_name_for(model, &lower))
        .or_else(|| {
            catalog_provider_of(&canonical).map(|provider| display_name_from_id(provider, &canonical))
        })
        .unwrap_or(canonical)
}

/// The effort levels `model`'s catalog row declares, when it declares any.
#[must_use]
pub fn declared_effort_levels(model: &str) -> Option<Vec<EffortLevel>> {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    catalog_fact(|catalog| catalog.effort_levels_for(model, &lower))
}

/// The priority-serving tiers `model`'s catalog row declares (`fast`).
#[must_use]
pub fn declared_speed_tiers(model: &str) -> Vec<String> {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    catalog_fact(|catalog| catalog.speed_tiers_for(model, &lower)).unwrap_or_default()
}

/// Whether `model`'s catalog row declares `capability` (`imagegen`,
/// `streaming_only`).
#[must_use]
pub fn model_has_capability(model: &str, capability: &str) -> bool {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    catalog_fact(|catalog| catalog.capabilities_for(model, &lower))
        .is_some_and(|declared| declared.iter().any(|fact| fact.eq_ignore_ascii_case(capability)))
}

/// Whether `model` accepts multimodal image (vision) inputs.
///
/// Models declaring `no_vision` in their catalog capabilities (e.g. Codex Spark)
/// do not accept image inputs.
#[must_use]
pub fn model_supports_vision(model: &str) -> bool {
    !model_has_capability(model, "no_vision")
}

/// Text stand-in for an image omitted because the target model does not support
/// vision inputs or because images were shed to fit request limits.
#[must_use]
pub fn image_omitted_placeholder(media_type: &str) -> String {
    format!("[image omitted: {media_type} not sent because this model does not accept images]")
}

/// The provider that serves `model`: its registry row, else its id's naming.
/// `None` for an id no first-party lineup claims.
fn catalog_provider_of(model: &str) -> Option<ProviderKind> {
    let lower = model.trim().to_ascii_lowercase();
    if let Some(entry) = catalog_entry_for_token(model_registry(), &lower) {
        return Some(entry.provider);
    }
    // A bare family name with a release tacked on (`opus-4.6`, `sol-2`): the
    // family alias it starts with says which provider serves it.
    let first = id_tokens(&lower).into_iter().next()?;
    if let Some(entry) = model_registry()
        .iter()
        .find(|entry| entry.alias.eq_ignore_ascii_case(&first))
    {
        return Some(entry.provider);
    }
    provider_kind_for_canonical(&lower, &lower)
}

/// The catalog id `model` is a qualified member of — a dated, `@`- or
/// `[`-suffixed spelling of a shipped or discovered id (`gpt-5.6-sol-2026-07-09`
/// → `gpt-5.6-sol`) — for a backend that wants the family id on the wire.
/// `None` when `model` is itself a catalog id or matches none.
#[must_use]
pub fn catalog_family_id(model: &str) -> Option<String> {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    catalog_fact(|catalog| catalog.family_prefix_id_for(model, &lower))
}

/// The capability facts `model`'s catalog row declares.
#[must_use]
pub fn declared_capabilities(model: &str) -> Vec<String> {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    catalog_fact(|catalog| catalog.capabilities_for(model, &lower)).unwrap_or_default()
}

/// The token a provider's first-party ids start with (`claude-…`,
/// `gemini-…`), which its family aliases may also carry (`claude-opus`,
/// `gemini-flash`). OpenAI ids start with a lineup word instead (`gpt-`,
/// `o3-`, `codex-`), so its aliases are bare.
const fn provider_id_prefix(provider: ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::Anthropic => Some("claude"),
        ProviderKind::Google => Some("gemini"),
        ProviderKind::Xai => Some("grok"),
        ProviderKind::OpenAi | ProviderKind::Ollama => None,
    }
}

/// `gpt-5.6-terra[fast]` → `["gpt", "5.6", "terra"]`: the id's dash-separated
/// tokens, with a serving-tier bracket or explicit-provider suffix cut off.
fn id_tokens(id: &str) -> Vec<String> {
    let lower = id.trim().to_ascii_lowercase();
    let bare = lower.split(['[', '@']).next().unwrap_or(&lower);
    bare.split('-')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

/// A release number (`5`, `5.6`) or a date stamp (`20251001`).
fn is_version_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().any(|c| c.is_ascii_digit())
        && token.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// A `YYYYMMDD` release stamp — versioning, but not a version a name shows.
fn is_date_token(token: &str) -> bool {
    token.len() == 8 && token.chars().all(|c| c.is_ascii_digit())
}

/// Tokens that spell a serving tier, a preview label or a wire-only suffix
/// rather than a lineup: Antigravity's tiered ids (`-low`, `-high`,
/// `-tiered`, `-agent`), Google's `-preview`/`-customtools`, and OpenAI's
/// legacy `-fast` spelling. Id grammar, not a model list.
const ID_NOISE_TOKENS: &[&str] = &[
    "preview", "customtools", "low", "medium", "high", "extra", "tiered", "agent", "fast",
];

/// The lineup family an id spells under its provider's grammar: the tokens
/// left once the provider prefix, release numbers, date stamps and
/// tier/label suffixes are removed — `claude-fable-5-1` → `fable`,
/// `gpt-6-astra` → `astra`, `gpt-5.3-codex-spark` → `spark`,
/// `gemini-3.1-flash-lite` → `flash-lite`. An OpenAI id with no codename
/// (`gpt-5.5`) is its own lineup word (`gpt`). `None` for a provider whose ids
/// carry no lineup (Ollama) or an id that names nothing but a version.
#[must_use]
pub fn family_from_id(provider: ProviderKind, id: &str) -> Option<String> {
    let tokens = id_tokens(id);
    let prefix = provider_id_prefix(provider);
    let lineup: Vec<&str> = tokens
        .iter()
        .map(String::as_str)
        .filter(|token| !is_version_token(token) && !ID_NOISE_TOKENS.contains(token))
        .filter(|token| Some(*token) != prefix)
        .filter(|token| {
            provider != ProviderKind::OpenAi
                || (!is_openai_builtin_model_prefix(token) && *token != "codex")
        })
        .collect();
    match provider {
        ProviderKind::Anthropic | ProviderKind::Google | ProviderKind::Xai => {
            (!lineup.is_empty()).then(|| lineup.join("-"))
        }
        ProviderKind::OpenAi => Some(if lineup.is_empty() {
            tokens.first()?.clone()
        } else {
            lineup.join("-")
        }),
        ProviderKind::Ollama => None,
    }
}

fn capitalize(token: &str) -> String {
    let mut chars = token.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_ascii_uppercase().to_string() + chars.as_str()
    })
}

/// The human name an id spells under its provider's grammar, as the provider
/// itself spells names: Anthropic `claude-fable-5-1` → `Fable 5.1`
/// (`claude-haiku-4-5-20251001` → `Haiku 4.5`), OpenAI `gpt-5.6-sol` →
/// `GPT-5.6-Sol` (Codex's own `display_name` shape), Google
/// `gemini-3.1-flash-lite` → `Gemini 3.1 Flash Lite`.
#[must_use]
pub fn display_name_from_id(provider: ProviderKind, id: &str) -> String {
    let tokens = id_tokens(id);
    if tokens.is_empty() {
        return id.trim().to_string();
    }
    match provider {
        ProviderKind::Anthropic => {
            let prefix = provider_id_prefix(provider);
            let words: Vec<String> = tokens
                .iter()
                .filter(|token| Some(token.as_str()) != prefix && !is_version_token(token))
                .map(|token| capitalize(token))
                .collect();
            let version: Vec<&str> = tokens
                .iter()
                .filter(|token| is_version_token(token) && !is_date_token(token))
                .map(String::as_str)
                .collect();
            let mut name = words.join(" ");
            if !version.is_empty() {
                if !name.is_empty() {
                    name.push(' ');
                }
                name.push_str(&version.join("."));
            }
            name
        }
        ProviderKind::OpenAi => tokens
            .iter()
            .enumerate()
            .map(|(index, token)| {
                if index == 0 {
                    token.to_ascii_uppercase()
                } else {
                    capitalize(token)
                }
            })
            .collect::<Vec<_>>()
            .join("-"),
        ProviderKind::Google | ProviderKind::Xai | ProviderKind::Ollama => tokens
            .iter()
            .map(|token| capitalize(token))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn catalog_model_context_window(raw_model: &str, canonical_model: &str) -> Option<u64> {
    catalog_fact(|catalog| catalog.context_window_for(raw_model, canonical_model))
}

/// The id `model` is actually served under at `effort`, when the catalog
/// declares one.
///
/// Resolution order matches every other model fact: the
/// [`MODEL_CONTEXT_WINDOWS_ENV`] override catalog first (which is how a
/// settings-declared model reaches this crate), then the built-in
/// [`model_context_windows.json`].
///
/// `None` means "the selection id is the wire id" — the honest default. A
/// caller must send the model id unchanged rather than guessing a variant,
/// because inventing one turns a model zo does not know about into a 404
/// instead of a working request.
/// The reasoning rung a request is asking for, as the three tiers a wire
/// catalog expresses.
///
/// The legacy `thinkingBudget` path folds into the same rungs rather than
/// keeping a parallel scale: a budget is only ever an expression of "roughly
/// this much thinking", and the providers that tier their model ids offer
/// nothing finer to aim at.
#[must_use]
pub fn effort_rung(reasoning: crate::types::ReasoningRequest) -> EffortLevel {
    use crate::types::ReasoningRequest;
    match reasoning {
        ReasoningRequest::Effort(effort) => effort,
        ReasoningRequest::BudgetTokens(8_000..) => EffortLevel::High,
        ReasoningRequest::BudgetTokens(4_000..8_000) => EffortLevel::Medium,
        ReasoningRequest::BudgetTokens(_) | ReasoningRequest::Auto => EffortLevel::Low,
    }
}

/// The wire id for a request, or the request's own model id when the catalog
/// declares none. The form every provider's request builder wants.
#[must_use]
pub fn wire_model_for_request(model: &str, reasoning: crate::types::ReasoningRequest) -> String {
    let trimmed = model.trim();
    wire_model_for_effort(trimmed, effort_rung(reasoning)).unwrap_or_else(|| trimmed.to_string())
}

#[must_use]
pub fn wire_model_for_effort(model: &str, effort: EffortLevel) -> Option<String> {
    // Catalog-first resolution, not the provider-enable-gated resolver: a wire
    // id is a fact about the model, so `gemini-flash` must name the same served
    // model whether or not this machine has finished connecting Google.
    let canonical = resolve_catalog_alias(model);
    catalog_fact(|catalog| catalog.wire_for(model, &canonical, effort))
}

/// The docs-verified max synchronous output tokens for a model, from the env
/// override catalog first then the built-in — mirrors
/// [`catalog_model_context_window`] for the `max_output_tokens` field.
fn catalog_max_output_tokens(raw_model: &str, canonical_model: &str) -> Option<u64> {
    catalog_fact(|catalog| catalog.max_output_tokens_for(raw_model, canonical_model))
}

/// The declared maximum serialized request-body size (bytes) for a model, from
/// the env override catalog first then the built-in — mirrors
/// `catalog_model_context_window` for the `max_request_bytes` field.
///
/// `None` means "undeclared", never "unlimited": callers must skip the check
/// rather than substitute a guess. A custom provider declares its own ceiling
/// through the same `ZO_MODEL_CONTEXT_WINDOWS` catalog as every other model
/// fact, so no code change is needed to teach zo about a new endpoint.
#[must_use]
pub fn max_request_bytes_for_model(model: &str) -> Option<u64> {
    let canonical = resolve_model_alias(model);
    let canonical_lower = canonical.to_ascii_lowercase();
    catalog_fact(|catalog| catalog.max_request_bytes_for(model, &canonical_lower))
}

/// Process-wide shared `reqwest::Client` for every provider call.
///
/// 매 `ProviderClient` 가 `reqwest::Client::new()` 를 호출하면 connection
/// pool 이 인스턴스마다 분리되어 H2 multiplexing / TLS session resumption
/// 효과가 모두 소실된다. 단일 인스턴스로 일원화하면:
/// * 첫 호출 이후 TLS / TCP 재사용 → first-token 1-RTT 단축
/// * H2 stream multiplexing 으로 multi-agent burst 시 connection 폭증 방지
/// * `pool_idle_timeout` 5분으로 후속 turn 도 warm 유지
/// * H2 keep-alive 30s 로 NAT/방화벽이 idle 연결 끊는 것 차단
///
/// `reqwest::Client` 내부는 `Arc<Inner>` 이므로 `clone()` 비용 0. ALPN 으로
/// H2 자동 협상되므로 `http2_prior_knowledge` 는 사용하지 않는다 — 서버가
/// HTTP/1.1 만 지원해도 graceful fallback 보장.
#[must_use]
pub(crate) fn shared_http_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(tuned_http_client).clone()
}

/// Escape hatch for a poisoned shared pool: a brand-new client — fresh TCP,
/// fresh TLS, empty pool — with the same tuning as [`shared_http_client`].
/// Reached for only after consecutive transport-level send failures, where
/// connection REUSE is the prime suspect (a half-open keep-alive the H2 ping
/// hasn't declared dead yet fails every retry identically, while a new
/// process connects fine — measured live: zo-bench fix-off-by-one, 6/6
/// attempts dead on the shared pool while claude-code succeeded alongside).
#[must_use]
pub(crate) fn poison_escape_http_client() -> reqwest::Client {
    tuned_http_client()
}

fn tuned_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .tcp_nodelay(true)
        // Bound the connect phase only — a dead/blackholed host must not
        // wedge the SSE client forever. No blanket `.timeout()`: Anthropic
        // documents streaming as the timeout-avoidance path for large
        // `max_tokens` requests, and an active Opus/Fable stream can run
        // for minutes.
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Some(Duration::from_secs(300)))
        .http2_keep_alive_interval(Some(Duration::from_secs(30)))
        .http2_keep_alive_while_idle(true)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Xai,
    OpenAi,
    Google,
    Ollama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheStrategy {
    /// Anthropic prompt caching is expressed inside message/system content
    /// blocks with `cache_control` breakpoints plus local prompt-cache
    /// bookkeeping.
    AnthropicCacheControl,
    /// Official OpenAI request caching uses a stable `prompt_cache_key`.
    /// Extended retention is model-gated by [`openai_prompt_cache_retention`].
    OpenAiPromptCacheKey,
    /// OpenAI-compatible providers often reject unknown request fields, so
    /// zo sends no provider-specific cache controls unless a first-class
    /// strategy is known.
    NoRequestControls,
}

impl PromptCacheStrategy {
    /// Whether this strategy sends OpenAI's stable prompt-cache routing key.
    #[must_use]
    pub const fn sends_openai_prompt_cache_key(self) -> bool {
        matches!(self, Self::OpenAiPromptCacheKey)
    }

    /// Optional extended retention for strategies that support it.
    #[must_use]
    pub fn prompt_cache_retention(self, model: &str) -> Option<String> {
        match self {
            Self::OpenAiPromptCacheKey => openai_prompt_cache_retention(model),
            Self::AnthropicCacheControl | Self::NoRequestControls => None,
        }
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.profile().display_name)
    }
}

/// The complete, declarative description of one provider — its display name,
/// rate-limit namespace, connection wiring, and capability flags — in a single
/// place. Every [`ProviderKind`] accessor reads from this profile, so adding a
/// provider is exactly one [`ProviderKind`] variant plus one row in
/// [`ProviderKind::profile`], instead of an edit to six scattered `match` arms
/// (the old `connection` / `Display` / `supports_cache_tokens` /
/// `prompt_cache_strategy` / `supports_thinking` / `rate_limit_key` split, where
/// a forgotten arm silently gave a new provider the wrong capability).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderProfile {
    /// Human-facing provider name (used by `Display`).
    display_name: &'static str,
    /// Stable namespace key for rate-limit bucketing / telemetry.
    rate_limit_key: &'static str,
    /// Environment variable holding the provider credential.
    auth_env: &'static str,
    /// Environment variable overriding the provider base URL.
    base_url_env: &'static str,
    /// Default API endpoint when `base_url_env` is unset.
    default_base_url: &'static str,
    /// Whether the provider reports cache token counts in its usage payload.
    supports_cache_tokens: bool,
    /// Whether the provider streams extended-thinking / reasoning content.
    supports_thinking: bool,
    /// Provider-specific prompt-cache request strategy.
    prompt_cache_strategy: PromptCacheStrategy,
}

impl ProviderKind {
    /// The single declarative profile row for this provider. All other
    /// accessors derive from it, keeping provider data in one place.
    const fn profile(self) -> ProviderProfile {
        match self {
            Self::Anthropic => ProviderProfile {
                display_name: "Anthropic",
                rate_limit_key: "anthropic",
                auth_env: "ANTHROPIC_API_KEY",
                base_url_env: "ANTHROPIC_BASE_URL",
                default_base_url: anthropic::DEFAULT_BASE_URL,
                supports_cache_tokens: true,
                supports_thinking: true,
                prompt_cache_strategy: PromptCacheStrategy::AnthropicCacheControl,
            },
            Self::Xai => ProviderProfile {
                display_name: "xAI",
                rate_limit_key: "xai",
                auth_env: "XAI_API_KEY",
                base_url_env: "XAI_BASE_URL",
                default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
                supports_cache_tokens: false,
                supports_thinking: false,
                prompt_cache_strategy: PromptCacheStrategy::NoRequestControls,
            },
            Self::OpenAi => ProviderProfile {
                display_name: "OpenAI",
                rate_limit_key: "openai",
                auth_env: "OPENAI_API_KEY",
                base_url_env: "OPENAI_BASE_URL",
                default_base_url: openai_compat::DEFAULT_OPENAI_BASE_URL,
                supports_cache_tokens: true,
                supports_thinking: false,
                prompt_cache_strategy: PromptCacheStrategy::OpenAiPromptCacheKey,
            },
            Self::Google => ProviderProfile {
                display_name: "Google",
                rate_limit_key: "google",
                auth_env: "GOOGLE_API_KEY",
                base_url_env: "GOOGLE_BASE_URL",
                default_base_url: openai_compat::DEFAULT_GOOGLE_BASE_URL,
                supports_cache_tokens: false,
                supports_thinking: false,
                prompt_cache_strategy: PromptCacheStrategy::NoRequestControls,
            },
            Self::Ollama => ProviderProfile {
                display_name: "Ollama",
                rate_limit_key: "ollama",
                auth_env: "OLLAMA_API_KEY",
                base_url_env: "OLLAMA_BASE_URL",
                default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
                supports_cache_tokens: false,
                supports_thinking: false,
                prompt_cache_strategy: PromptCacheStrategy::NoRequestControls,
            },
        }
    }

    /// Connection metadata (credential env, base-url env, default endpoint).
    #[must_use]
    pub const fn metadata(self) -> ProviderMetadata {
        let profile = self.profile();
        ProviderMetadata {
            provider: self,
            auth_env: profile.auth_env,
            base_url_env: profile.base_url_env,
            default_base_url: profile.default_base_url,
        }
    }

    /// Whether the provider reports cache token counts in its usage payload.
    /// Anthropic reports explicit cache read/write fields; OpenAI reports
    /// prompt cache reads through `prompt_tokens_details.cached_tokens`.
    #[must_use]
    pub const fn supports_cache_tokens(self) -> bool {
        self.profile().supports_cache_tokens
    }

    /// Provider-specific prompt-cache request strategy.
    #[must_use]
    pub const fn prompt_cache_strategy(self) -> PromptCacheStrategy {
        self.profile().prompt_cache_strategy
    }

    /// Whether the provider streams extended-thinking / reasoning content.
    /// Only Anthropic surfaces thinking blocks through this client today.
    #[must_use]
    pub const fn supports_thinking(self) -> bool {
        self.profile().supports_thinking
    }

    /// Stable namespace key for rate-limit bucketing / telemetry.
    #[must_use]
    pub const fn rate_limit_key(self) -> &'static str {
        self.profile().rate_limit_key
    }
}

/// Extended OpenAI prompt-cache retention the catalog verified for `model`.
/// `None` means zo should still send `prompt_cache_key` for OpenAI cache
/// routing, but must not request extended retention.
#[must_use]
pub fn openai_prompt_cache_retention(model: &str) -> Option<String> {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    if !is_openai_model(&lower) {
        return None;
    }
    catalog_fact(|catalog| catalog.prompt_cache_retention_for(model, &lower))
}

/// The priority-serving tier Codex declares in `additional_speed_tiers`.
const FAST_SPEED_TIER: &str = "fast";

/// Base/priority pair for an OpenAI model whose catalog row declares the
/// `fast` speed tier: the row's own spelling when it states one
/// (`gpt-5.5` / `gpt-5.5-fast`), else `<id>` / `<id>[fast]`. `None` for a
/// model no row vouches for — a synthetic id must never acquire priority
/// routing by naming convention.
#[must_use]
pub fn openai_fast_variant_pair(model: &str) -> Option<(String, String)> {
    let lower = resolve_catalog_alias(model).trim().to_ascii_lowercase();
    if !is_openai_model(&lower) {
        return None;
    }
    if let Some(pair) = catalog_fact(|catalog| catalog.fast_pair_for(model, &lower)) {
        return Some(pair);
    }
    let declared = catalog_fact(|catalog| catalog.speed_tiers_for(model, &lower))?;
    if !declared.iter().any(|tier| tier.eq_ignore_ascii_case(FAST_SPEED_TIER)) {
        return None;
    }
    let base = lower.split('[').next().unwrap_or(&lower).to_string();
    let fast = format!("{base}[{FAST_SPEED_TIER}]");
    Some((base, fast))
}

/// Whether `model` is exactly a catalog-declared priority-tier spelling.
///
/// Merely ending in `-fast` or `[fast]` is insufficient: an unregistered or
/// synthetic id must never acquire priority routing by naming convention.
#[must_use]
pub fn openai_fast_tier_enabled(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    openai_fast_variant_pair(&normalized)
        .is_some_and(|(_, fast)| normalized.eq_ignore_ascii_case(&fast))
}

/// Whether OpenAI's first-party backend serves `model`: a registry row says
/// so, or the id follows OpenAI's own naming (`gpt*`, `o1*`, `o3*`, `o4*`,
/// `codex*`) — a naming convention, not a model list. Deterministic and
/// env-free, so a custom OpenAI-compatible provider is never re-tiered off an
/// ambient `OPENAI_API_KEY`.
#[must_use]
pub fn is_openai_model(model: &str) -> bool {
    let lower = model.trim().to_ascii_lowercase();
    catalog_entry_for_token(model_registry(), &lower)
        .is_some_and(|entry| entry.provider == ProviderKind::OpenAi)
        || is_openai_builtin_model_prefix(&lower)
}

/// Whether `token` is one of OpenAI's lineup words (`gpt`, `o1`, `o3`, `o4`,
/// `codex`): the part of an id that names the lineup rather than a release's
/// codename. `gpt-6` has a lineup word and no codename; `gpt-6-astra` has
/// both. Pure id grammar — unlike [`is_openai_model`] it never consults the
/// registry, so the answer is the same before and after an alias the grammar
/// minted has been published.
#[must_use]
pub fn is_openai_lineup_word(token: &str) -> bool {
    is_openai_builtin_model_prefix(&token.trim().to_ascii_lowercase())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub provider: ProviderKind,
    pub auth_env: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelCapability {
    canonical_model_id: String,
    provider: Option<ProviderKind>,
    context_window: Option<u64>,
    max_output_tokens: Option<u32>,
    adaptive_thinking: Option<bool>,
}

impl ModelCapability {
    fn for_model(model: &str) -> Self {
        // Explicit `provider/model` keeps capability lookup on the wire model id
        // while forcing OpenAI-compatible custom routing when the named custom
        // provider exists. Bare Claude/GPT ids still use first-party paths.
        let wire = wire_model_id(model);
        let canonical_model_id = wire.clone();
        let canonical_lower = canonical_model_id.to_ascii_lowercase();
        let model_id = canonical_lower
            .split_once('[')
            .map_or(canonical_lower.as_str(), |(id, _)| id);

        let provider = if custom_provider_for_model(model).is_some()
            && split_provider_model_ref(model).is_some()
        {
            Some(ProviderKind::OpenAi)
        } else if let Some(entry) = catalog_entry_for_token(model_registry(), &canonical_model_id)
            .filter(|entry| entry.provider == ProviderKind::OpenAi)
        {
            // OpenAI's built-in ids have historically been prefix-detectable
            // (`gpt*`, `o*`, `codex*`). Future OpenAI families may not be. Treat
            // OpenAI catalog aliases/canonicals as explicit OpenAI selections so
            // a disabled/missing OpenAI provider produces a clean unsupported-
            // provider error instead of falling through to Anthropic.
            Some(entry.provider)
        } else {
            provider_kind_for_canonical(&canonical_model_id, &canonical_lower)
        };

        // An explicit `<provider>/<model>` states its limits through the named
        // provider first: the wire id below has the provider stripped, and the
        // same bare id may be a first-party model with another window.
        let named = named_provider_ref(model);
        let context_window = named
            .as_ref()
            .and_then(|(provider, declared)| provider.context_window_for(declared))
            .unwrap_or_else(|| context_window_for_canonical(&wire, &canonical_lower));
        let max_output_tokens = named
            .as_ref()
            .and_then(|(provider, declared)| provider.max_output_tokens_for(declared))
            .map_or_else(
                || max_output_tokens_for_model_id(&wire, model_id),
                |value| u32::try_from(value).unwrap_or(u32::MAX),
            );

        Self {
            provider,
            context_window: Some(context_window),
            max_output_tokens: Some(max_output_tokens),
            adaptive_thinking: Some(adaptive_thinking_for_canonical(&canonical_lower)),
            canonical_model_id,
        }
    }
}

fn model_capability_for_model(model: &str) -> ModelCapability {
    ModelCapability::for_model(model)
}

fn provider_kind_for_canonical(
    canonical_model_id: &str,
    canonical_lower: &str,
) -> Option<ProviderKind> {
    // OpenAI is the only built-in provider whose future model-family prefixes
    // are hard to predict from the id. Trust an explicit OpenAI registry row
    // before prefix guessing so a newly-added family does not fall through to
    // Anthropic; leave other providers on their existing gated/prefix paths.
    if let Some(provider) = catalog_provider_for_canonical(model_registry(), canonical_model_id)
        .filter(|provider| *provider == ProviderKind::OpenAi)
    {
        return Some(provider);
    }
    if canonical_lower.starts_with("claude") {
        return Some(ProviderKind::Anthropic);
    }
    if custom_provider_for_model(canonical_model_id).is_some() {
        return Some(ProviderKind::OpenAi);
    }
    if is_openai_builtin_model_prefix(canonical_lower) {
        return Some(ProviderKind::OpenAi);
    }
    if non_claude_adapters_enabled() && canonical_lower.starts_with("grok") {
        return Some(ProviderKind::Xai);
    }
    if canonical_lower.starts_with("gemini") {
        return Some(ProviderKind::Google);
    }
    if canonical_lower.starts_with("ollama") || std::env::var("OLLAMA_BASE_URL").is_ok() {
        return Some(ProviderKind::Ollama);
    }
    None
}

fn is_openai_builtin_model_prefix(lower: &str) -> bool {
    lower.starts_with("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.starts_with("codex")
}

fn catalog_provider_for_canonical(
    catalog: &[ProviderCatalogEntry],
    canonical_model_id: &str,
) -> Option<ProviderKind> {
    let canonical = canonical_model_id.trim();
    catalog
        .iter()
        .find(|entry| entry.canonical_model_id.eq_ignore_ascii_case(canonical))
        .map(|entry| entry.provider)
}

fn catalog_entry_for_token<'a>(
    catalog: &'a [ProviderCatalogEntry],
    model: &str,
) -> Option<&'a ProviderCatalogEntry> {
    let lower = model.trim().to_ascii_lowercase();
    catalog.iter().find(|entry| {
        entry.alias.eq_ignore_ascii_case(&lower)
            || entry.canonical_model_id.eq_ignore_ascii_case(&lower)
    })
}

/// A single row of the provider catalog: a user-facing model `alias`, the
/// `canonical_model_id` it resolves to (the value actually sent to the API),
/// and the [`ProviderKind`] that serves it.
///
/// Connection wiring and capabilities are *derived* from the provider rather
/// than duplicated per row, so the catalog stays a flat alias→id table and a
/// new provider is one [`ProviderKind`] variant plus its registry rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCatalogEntry {
    pub alias: &'static str,
    pub canonical_model_id: &'static str,
    pub provider: ProviderKind,
    pub fit_hint: Option<ModelFitHint>,
    /// `Some(rank)` when this model is a reasoning-first orchestrator: it
    /// plans, verifies, and supervises rather than doing ordinary
    /// implementation. Lower rank is preferred for PLAN/VERIFY.
    ///
    /// The routing policy derives BOTH its reserved set and the default
    /// PLAN/VERIFY preference order from this, so "what each model is good at"
    /// is stated once, here, beside the model's identity — adding a reserved
    /// model is a catalog row, never a policy edit. The rank is explicit
    /// because registry order groups rows by provider and must not silently
    /// decide which model verifies first. Nothing else in the catalog
    /// distinguishes these models: effort ceilings do not (every Claude is
    /// `Max`; Sol and Terra are both `Ultra`).
    pub orchestration_rank: Option<u8>,
    /// One-step fallback alias when this model is starved by rate limits, or
    /// `None` when it is the bottom rung of its family ladder. Stated beside the
    /// model's identity for the same reason `orchestration_rank` is: the ladder
    /// is a fact about the lineup, and keeping it here means adding a model
    /// never means editing a policy table.
    pub demotes_to: Option<&'static str>,
    /// Where a safety-classifier refusal on this lineup goes next, in
    /// preference order, or empty when a refusal is surfaced instead. A fact
    /// about the lineup's classifier, kept beside its identity for the same
    /// reason `demotes_to` is: the runtime's refusal path reads it and names no
    /// lineup itself.
    pub refusal_fallback: &'static [&'static str],
}

impl ProviderCatalogEntry {
    /// Build one entry directly. The shipped catalog is loaded from JSON, so
    /// this exists for tests that need a hand-made catalog to exercise the
    /// resolution helpers against.
    #[must_use]
    #[cfg(test)]
    const fn new(
        alias: &'static str,
        canonical_model_id: &'static str,
        provider: ProviderKind,
    ) -> Self {
        Self {
            alias,
            canonical_model_id,
            provider,
            fit_hint: None,
            orchestration_rank: None,
            demotes_to: None,
            refusal_fallback: &[],
        }
    }

    /// Connection metadata for the entry's provider.
    #[must_use]
    pub const fn metadata(&self) -> ProviderMetadata {
        self.provider.metadata()
    }

    /// See [`ProviderKind::supports_cache_tokens`].
    #[must_use]
    pub const fn supports_cache_tokens(&self) -> bool {
        self.provider.supports_cache_tokens()
    }

    /// See [`ProviderKind::supports_thinking`].
    #[must_use]
    pub const fn supports_thinking(&self) -> bool {
        self.provider.supports_thinking()
    }

    /// See [`ProviderKind::rate_limit_key`].
    #[must_use]
    pub const fn rate_limit_key(&self) -> &'static str {
        self.provider.rate_limit_key()
    }
}

/// Read-only model fit metadata. This is advisory only: zo never selects,
/// downloads, starts, or rejects a model from these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFitHint {
    pub estimated_vram_gb: u16,
    pub quantization: &'static str,
}

impl ModelFitHint {
    #[must_use]
    pub const fn new(estimated_vram_gb: u16, quantization: &'static str) -> Self {
        Self {
            estimated_vram_gb,
            quantization,
        }
    }

    #[must_use]
    pub fn display_label(self) -> String {
        format!("VRAM ~{}GB {}", self.estimated_vram_gb, self.quantization)
    }
}

/// Semantic alias for the current default Anthropic model.
///
/// Callers that mean "latest Anthropic" must use this token rather than copy a
/// release id. Only the provider catalog below owns the alias → release mapping.
pub const ANTHROPIC_LATEST_MODEL_ALIAS: &str = "anthropic-latest";

/// Semantic alias for the current Opus family head.
///
/// This is intentionally versionless: model-authored delegation prompts are
/// stale more often than the catalog, so spawn/default/fallback paths resolve
/// this alias at execution time instead of embedding a release number.
pub const ANTHROPIC_OPUS_MODEL_ALIAS: &str = "opus";
pub const ANTHROPIC_FABLE_MODEL_ALIAS: &str = "fable";
pub const OPENAI_LATEST_MODEL_ALIAS: &str = "openai-latest";
pub const OPENAI_FAST_MODEL_ALIAS: &str = "spark";
pub const GOOGLE_LATEST_MODEL_ALIAS: &str = "google-latest";
pub const XAI_LATEST_MODEL_ALIAS: &str = "xai-latest";

/// Built-in model-discovery seeds for API-compatible provider presets.
///
/// They live with the provider catalog so CLI setup code never embeds release
/// ids. Runtime custom-provider configuration still overrides these seeds.
pub const DEEPSEEK_PRESET_MODELS: &[&str] = &["deepseek-chat", "deepseek-reasoner"];
pub const KIMI_PRESET_MODELS: &[&str] = &["kimi-k2-0905-preview", "moonshot-v1-32k"];
pub const QWEN_PRESET_MODELS: &[&str] = &["qwen-max", "qwen-plus", "qwen-turbo"];
pub const NVIDIA_PRESET_MODELS: &[&str] = &["meta/llama-3.1-8b-instruct", "z-ai/glm-5.2"];

/// The full provider catalog — the single source of truth mapping model
/// aliases to canonical ids and providers.
#[must_use]
pub fn provider_catalog() -> &'static [ProviderCatalogEntry] {
    model_registry()
}

/// Canonical id for `model`, resolved from the catalog BEFORE the live
/// provider-enable gate.
///
/// [`resolve_model_alias`] only resolves a non-Claude alias once that
/// provider's adapter is enabled, so a policy-owned token like `openai-latest`
/// stays unresolved on a machine that has not connected OpenAI — the same
/// configured pool would then name different models on different machines.
/// Ids the catalog does not carry fall through to the gated resolver.
#[must_use]
pub fn resolve_catalog_alias(model: &str) -> String {
    let trimmed = model.trim();
    model_registry()
        .iter()
        .find(|entry| entry.alias.eq_ignore_ascii_case(trimmed))
        .map_or_else(
            || resolve_model_alias(trimmed),
            |entry| entry.canonical_model_id.to_string(),
        )
}

/// The short family alias a canonical id is best known by — `claude-opus-5` →
/// `opus`, `gpt-5.6-terra` → `terra`, `gemini-3.1-pro-preview` → `gemini-pro`.
///
/// This is what settings should PIN instead of a release id: an alias follows
/// the family head when the provider ships the next release (the catalog
/// re-points it, see `runtime::model_discovery`), while a dated id is frozen
/// at whatever was newest the day it was written. Among the alias rows that
/// name `model`, the pick is the shortest one that is a real family name — not
/// a `*-latest` policy token, not a `[1m]`-style label variant, and not the
/// canonical id spelled back at itself. `None` when no such row exists, which
/// is the honest answer for a release that is not any family's head
/// (`claude-opus-4-8`).
#[must_use]
pub fn family_alias_for(model: &str) -> Option<&'static str> {
    let canonical = resolve_catalog_alias(model);
    let canonical = canonical.trim();
    if canonical.is_empty() {
        return None;
    }
    model_registry()
        .iter()
        .filter(|entry| entry.canonical_model_id.eq_ignore_ascii_case(canonical))
        .map(|entry| entry.alias)
        .filter(|alias| {
            !alias.eq_ignore_ascii_case(canonical)
                && !alias.contains('[')
                && !alias.contains('/')
                && !alias.to_ascii_lowercase().ends_with("-latest")
        })
        .min_by_key(|alias| alias.len())
}

/// The shipped catalog's own bytes — the seed every published layer sits on,
/// in the same shape the [`MODEL_CONTEXT_WINDOWS_ENV`] document uses. For
/// the audit copy that shows the effective catalog end to end.
#[must_use]
pub const fn builtin_model_catalog_json() -> &'static str {
    BUILTIN_MODEL_CONTEXT_WINDOWS_JSON
}

/// The catalog exactly as shipped in this binary — no settings overlay, no
/// discovered rows. Discovery layers its rows over THIS, so a second publish
/// does not mistake its own earlier rows for shipped ones and drop them.
#[must_use]
pub fn builtin_provider_catalog() -> &'static [ProviderCatalogEntry] {
    static BUILTIN: OnceLock<&'static [ProviderCatalogEntry]> = OnceLock::new();
    BUILTIN.get_or_init(|| leak_registry(alias_rows_of(BUILTIN_MODEL_CONTEXT_WINDOWS_JSON)))
}

/// Semantic ALIASES (not release ids) of the models the catalog reserves for
/// plan/verify/orchestrate duty, ordered by their declared
/// [`ProviderCatalogEntry::orchestration_rank`].
///
/// This is the single source for both the routing policy's reserved set and
/// the default PLAN/VERIFY pool, so declaring a new reasoning-first model is a
/// catalog row change and nothing else. Aliases rather than canonical ids
/// because this list is also what a user sees and edits via `/tier`.
///
/// Derived once: the routing policy calls this per candidate while filtering.
#[must_use]
pub fn orchestration_reserved_models() -> &'static [&'static str] {
    static RESERVED: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    RESERVED.get_or_init(|| {
        let mut ranked: Vec<&'static ProviderCatalogEntry> = model_registry()
            .iter()
            .filter(|entry| entry.orchestration_rank.is_some())
            .collect();
        ranked.sort_by_key(|entry| entry.orchestration_rank);
        let mut seen: Vec<&'static str> = Vec::new();
        let mut aliases: Vec<&'static str> = Vec::new();
        for entry in ranked {
            if !seen
                .iter()
                .any(|id| id.eq_ignore_ascii_case(entry.canonical_model_id))
            {
                seen.push(entry.canonical_model_id);
                aliases.push(entry.alias);
            }
        }
        aliases
    })
}

/// Canonical id of the current default Anthropic model.
///
/// The catalog invariant is covered by tests; this function deliberately has
/// no release-id fallback because that would recreate the distributed
/// hardcoding this API exists to prevent.
#[must_use]
pub fn latest_anthropic_model() -> &'static str {
    latest_model_for_provider(ProviderKind::Anthropic)
        .expect("provider catalog must define the latest Anthropic alias")
}

/// Canonical id for a first-party provider's semantic `latest` alias.
///
/// Providers without a discoverable first-party catalog (custom/OpenAI-compatible
/// and Ollama) deliberately return `None`; their live inventory is authoritative.
#[must_use]
pub fn latest_model_for_provider(provider: ProviderKind) -> Option<&'static str> {
    let alias = match provider {
        ProviderKind::Anthropic => ANTHROPIC_LATEST_MODEL_ALIAS,
        ProviderKind::OpenAi => OPENAI_LATEST_MODEL_ALIAS,
        ProviderKind::Google => GOOGLE_LATEST_MODEL_ALIAS,
        ProviderKind::Xai => XAI_LATEST_MODEL_ALIAS,
        ProviderKind::Ollama => return None,
    };
    model_registry()
        .iter()
        .find(|entry| entry.provider == provider && entry.alias == alias)
        .map(|entry| entry.canonical_model_id)
}

/// The alias catalog every layer resolves through, as data.
///
/// Built from [`model_context_windows.json`] and re-buildable at runtime from
/// the [`MODEL_CONTEXT_WINDOWS_ENV`] override (which is how a settings-declared
/// model or alias reaches this crate). Nothing about which models exist, what
/// a short alias means, or which release an alias points at is compiled in —
/// a provider shipping a new model is a catalog edit, not a code change.
///
/// Entries are `&'static` because that is what the rest of the codebase
/// consumes. They are produced by leaking one built slice, which is bounded:
/// the builtin build happens once, and [`refresh_model_registry_from_json`]
/// skips a republish of bytes it has already applied.
static MODEL_REGISTRY_STORE: RwLock<Option<&'static [ProviderCatalogEntry]>> = RwLock::new(None);

/// The exact override JSON last applied, so repeated identical publishes (the
/// runtime is rebuilt several times per session) neither rebuild nor re-leak.
static MODEL_REGISTRY_APPLIED: RwLock<Option<String>> = RwLock::new(None);

fn read_registry_store() -> Option<&'static [ProviderCatalogEntry]> {
    *MODEL_REGISTRY_STORE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn alias_rows_of(raw: &str) -> Vec<AliasRow> {
    serde_json::from_str::<ModelContextCatalog>(raw)
        .map(|catalog| catalog.aliases)
        .unwrap_or_default()
}

/// Turn on-disk rows into the `&'static` entries the codebase consumes.
///
/// Rows are deduplicated by `(provider, alias)` keeping the FIRST occurrence,
/// which is what makes an override authoritative: it is concatenated ahead of
/// the built-ins, so an alias it names shadows the shipped row instead of
/// colliding with it.
/// Leak an owned alias list into the `&'static [&'static str]` the catalog
/// entries hold. Empty in, empty (`&[]`) out — no allocation for the common
/// row that declares no refusal fallback.
fn leak_alias_list(aliases: &[String]) -> &'static [&'static str] {
    if aliases.is_empty() {
        return &[];
    }
    let leaked: Vec<&'static str> = aliases
        .iter()
        .map(|alias| &*String::leak(alias.clone()))
        .collect();
    Vec::leak(leaked)
}

fn leak_registry(rows: Vec<AliasRow>) -> &'static [ProviderCatalogEntry] {
    let mut seen: Vec<(ProviderKind, String)> = Vec::new();
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(provider) = provider_kind_from_key(&row.provider) else {
            continue;
        };
        let alias = row.alias.trim();
        let canonical = row.canonical.trim();
        if alias.is_empty() || canonical.is_empty() {
            continue;
        }
        let key = (provider, alias.to_ascii_lowercase());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        entries.push(ProviderCatalogEntry {
            alias: String::leak(alias.to_string()),
            canonical_model_id: String::leak(canonical.to_string()),
            provider,
            fit_hint: None,
            orchestration_rank: row.orchestration_rank,
            demotes_to: row.demotes_to.map(|to| &*String::leak(to.trim().to_string())),
            refusal_fallback: leak_alias_list(&clone_alias_list(row.refusal_fallback.as_ref())),
        });
    }
    Vec::leak(entries)
}

fn model_registry() -> &'static [ProviderCatalogEntry] {
    if let Some(registry) = read_registry_store() {
        return registry;
    }
    let mut guard = MODEL_REGISTRY_STORE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Re-check under the write lock so a race builds (and leaks) once.
    if let Some(registry) = *guard {
        return registry;
    }
    let registry = leak_registry(alias_rows_of(BUILTIN_MODEL_CONTEXT_WINDOWS_JSON));
    *guard = Some(registry);
    registry
}

/// Layer an override catalog's alias rows ahead of the built-in ones.
///
/// The live companion to the startup env bridge, mirroring
/// [`refresh_custom_providers_from_json`]: settings are written, then this is
/// called so alias resolution and the model picker reflect the change without
/// a restart. Idempotent — re-applying the same bytes is a no-op.
pub fn refresh_model_registry_from_json(raw: &str) {
    {
        let applied = MODEL_REGISTRY_APPLIED
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if applied.as_deref() == Some(raw) {
            return;
        }
    }
    let mut rows = alias_rows_of(raw);
    rows.extend(alias_rows_of(BUILTIN_MODEL_CONTEXT_WINDOWS_JSON));
    let registry = leak_registry(rows);
    *MODEL_REGISTRY_STORE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(registry);
    *MODEL_REGISTRY_APPLIED
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(raw.to_string());
    // The priors ride the same document, so they follow the same publish.
    install_router_priors(raw);
}

/// Drop every override and go back to the shipped catalog. Tests only: the
/// registry is process-global, so a case that installs an override must undo it.
/// Diagnostic dump of a provider's wire traffic. When the named environment
/// variable is set, `content` is written to `/tmp/zo-<provider>-<tag>.txt` so
/// the exact request or error body can be read beside the provider's own
/// documentation (`ZO_CHATGPT_DEBUG`, `ZO_GEMINI_DEBUG`). A no-op — and silent
/// on a write failure — otherwise; it never touches the request itself.
pub(crate) fn debug_dump(env: &str, provider: &str, tag: &str, content: &str) {
    if std::env::var_os(env).is_some() {
        let _ = std::fs::write(format!("/tmp/zo-{provider}-{tag}.txt"), content);
    }
}

#[cfg(test)]
pub(crate) fn reset_model_registry_for_tests() {
    *MODEL_REGISTRY_STORE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    *MODEL_REGISTRY_APPLIED
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    *ROUTER_PRIORS_STORE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Env var holding a JSON array of user-defined OpenAI-compatible providers,
/// letting an operator add a provider without recompiling. Consulted only
/// after `model_registry` misses, so catalog aliases always win.
pub const CUSTOM_PROVIDERS_ENV: &str = "ZO_CUSTOM_PROVIDERS";

/// A custom provider resolved for the current process: its owned model list and
/// auth requirement, plus the leaked `&'static` config the client consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomProviderUsability {
    pub name: &'static str,
    pub models: Vec<String>,
    pub requires_auth: bool,
    pub credential_env_vars: Vec<&'static str>,
    pub usable: bool,
}

#[derive(Clone)]
pub struct ResolvedCustomProvider {
    pub models: Vec<String>,
    pub requires_auth: bool,
    pub config: openai_compat::OpenAiCompatConfig,
    /// Optional operator-provided context window for every declared model
    /// that states none of its own.
    pub context_window: Option<u64>,
    /// Optional operator-provided max output cap for every declared model
    /// that states none of its own.
    pub max_output_tokens: Option<u64>,
    /// The limits a declared model states for itself, keyed by its id exactly
    /// as declared (the spelling [`Self::canonical_model`] answers with), so
    /// the lookup is one hash. Empty for a provider of plain ids.
    pub model_limits: HashMap<String, openai_compat::ModelLimits>,
    /// Only read behind the non-default `model-fit-hints` feature (see
    /// [`fit_hint_for_model`]) — dead in default builds by design.
    #[allow(dead_code)]
    pub fit_hint: Option<ModelFitHint>,
}

impl From<openai_compat::CustomProviderConfig> for ResolvedCustomProvider {
    fn from(custom: openai_compat::CustomProviderConfig) -> Self {
        let mut model_limits = HashMap::new();
        let mut models = Vec::with_capacity(custom.models.len());
        for model in &custom.models {
            if model.limits.is_stated() {
                model_limits
                    .entry(model.id.clone())
                    .or_insert(model.limits);
            }
            models.push(model.id.clone());
        }
        Self {
            context_window: custom.context_window.filter(|&value| value > 0),
            max_output_tokens: custom.max_output_tokens.filter(|&value| value > 0),
            model_limits,
            fit_hint: custom.to_fit_hint(),
            config: custom.to_static_config(),
            models,
            requires_auth: custom.requires_auth,
        }
    }
}

impl ResolvedCustomProvider {
    /// Whether this provider serves `model` (case-insensitive id match).
    fn serves(&self, model: &str) -> bool {
        self.canonical_model(model).is_some()
    }

    /// The declared id `model` names on this provider: the model half of a
    /// `<this provider>/<id>` pick, else a whole declared id.
    fn declared_id(&self, model: &str) -> Option<&str> {
        let model = model.trim();
        model
            .split_once('/')
            .filter(|(named, _)| provider_name_matches(self.config.provider_name, named))
            .and_then(|(_, half)| self.canonical_model(half))
            .or_else(|| self.canonical_model(model))
    }

    /// The limits `model` states for itself on this provider, if any.
    fn stated_limits(&self, model: &str) -> openai_compat::ModelLimits {
        if self.model_limits.is_empty() {
            return openai_compat::ModelLimits::default();
        }
        self.declared_id(model)
            .and_then(|declared| self.model_limits.get(declared))
            .copied()
            .unwrap_or_default()
    }

    /// `model`'s context window on this provider: its own, else the
    /// provider-wide one.
    fn context_window_for(&self, model: &str) -> Option<u64> {
        self.stated_limits(model)
            .context_window
            .or(self.context_window)
            .filter(|&value| value > 0)
    }

    /// `model`'s output cap on this provider: its own, else the provider-wide
    /// one.
    fn max_output_tokens_for(&self, model: &str) -> Option<u64> {
        self.stated_limits(model)
            .max_output_tokens
            .or(self.max_output_tokens)
            .filter(|&value| value > 0)
    }

    /// The registered model id matching `model`, used as the canonical id sent
    /// to the endpoint (preserves the operator's declared casing).
    fn canonical_model(&self, model: &str) -> Option<&str> {
        // Compared in place: lowering a copy of each declared id cost one
        // allocation per model on every lookup (300+ for `OpenRouter`).
        let model = model.trim();
        self.models
            .iter()
            .find(|m| m.eq_ignore_ascii_case(model))
            .map(String::as_str)
    }
}

/// Parse the `ZO_CUSTOM_PROVIDERS` JSON array. Pure, for unit testing.
pub(crate) fn parse_custom_providers(
    raw: &str,
) -> Result<Vec<openai_compat::CustomProviderConfig>, serde_json::Error> {
    serde_json::from_str(raw)
}

/// Resolve parsed custom-provider config into process-lifetime provider rows.
fn resolve_custom_providers(
    parsed: Vec<openai_compat::CustomProviderConfig>,
) -> Vec<ResolvedCustomProvider> {
    parsed.into_iter().map(ResolvedCustomProvider::from).collect()
}

fn parse_resolved_custom_providers(
    raw: &str,
) -> Result<Vec<ResolvedCustomProvider>, serde_json::Error> {
    parse_custom_providers(raw).map(resolve_custom_providers)
}

fn load_custom_providers_from_env() -> Vec<ResolvedCustomProvider> {
    let Some(raw) = std::env::var(CUSTOM_PROVIDERS_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Vec::new();
    };
    match parse_resolved_custom_providers(&raw) {
        Ok(providers) => providers,
        Err(error) => {
            eprintln!("[zo] {CUSTOM_PROVIDERS_ENV} is not a valid provider JSON array: {error}");
            Vec::new()
        }
    }
}

const DEEPSEEK_DEFAULT_BASE_URL: &str = "https://api.deepseek.com";

/// `true` when a `DeepSeek` API key is reachable — set in the environment or
/// saved in the credential store via `/connect`. Gates the built-in `DeepSeek`
/// seed so the provider only surfaces once it is actually usable, mirroring how
/// OpenAI and Google built-ins appear only when their key/OAuth is present.
#[cfg(not(test))]
fn deepseek_credential_present() -> bool {
    std::env::var("DEEPSEEK_API_KEY").is_ok_and(|value| !value.trim().is_empty())
        || crate::oauth_store::load_openai_compat_api_key("DEEPSEEK_API_KEY")
            .ok()
            .flatten()
            .is_some_and(|value| !value.trim().is_empty())
}

/// Inert under unit tests: the custom-provider catalog tests assert exact
/// contents and must not depend on an ambient `DEEPSEEK_API_KEY` or a stored
/// credential on the developer's machine.
#[cfg(test)]
fn deepseek_credential_present() -> bool {
    false
}

/// The leaked, process-lifetime OpenAI-compatible config for the built-in
/// `DeepSeek` provider, initialized once so repeated catalog refreshes never
/// leak the strings again. `from_user` keeps `DeepSeek` on the same proven
/// data-driven path as a `/connect`-declared provider; `identity_maker` returns
/// `None` for
/// the `DeepSeek` name, so its identity is corrected without claiming a wrong
/// maker.
fn deepseek_seed_config() -> openai_compat::OpenAiCompatConfig {
    static CONFIG: OnceLock<openai_compat::OpenAiCompatConfig> = OnceLock::new();
    *CONFIG.get_or_init(|| {
        openai_compat::OpenAiCompatConfig::from_user(
            "DeepSeek",
            DEEPSEEK_DEFAULT_BASE_URL,
            Some("DEEPSEEK_API_KEY"),
            false,
        )
    })
}

/// zo's built-in provider seeds — known first-party OpenAI-compatible
/// providers (currently `DeepSeek`) that ship with zo so they work the moment
/// their key is present, without a `settings.json` / `ZO_CUSTOM_PROVIDERS`
/// entry. Each seed is gated on its credential so it never clutters the picker
/// unused. Model ids and context windows live in [`model_context_windows.json`].
fn builtin_seed_providers() -> Vec<ResolvedCustomProvider> {
    let mut seeds = Vec::new();
    if deepseek_credential_present() {
        seeds.push(ResolvedCustomProvider {
            models: vec![
                "deepseek-v4-pro".to_string(),
                "deepseek-v4-flash".to_string(),
            ],
            requires_auth: true,
            config: deepseek_seed_config(),
            context_window: None,
            max_output_tokens: None,
            model_limits: HashMap::new(),
            fit_hint: None,
        });
    }
    seeds
}

/// Append built-in seeds the operator has not already declared. User/env-defined
/// providers are kept first so an explicit config wins on a model-id collision
/// (`custom_provider_for_model` returns the first match).
fn with_builtin_seed(mut providers: Vec<ResolvedCustomProvider>) -> Vec<ResolvedCustomProvider> {
    for seed in builtin_seed_providers() {
        let already_declared = providers
            .iter()
            .any(|existing| seed.models.iter().any(|model| existing.serves(model)));
        if !already_declared {
            providers.push(seed);
        }
    }
    providers
}

/// The user-defined provider catalog. It is initialized from
/// `ZO_CUSTOM_PROVIDERS` (plus zo's built-in seeds), but can be refreshed
/// after an in-session `/connect` writes settings so the current TUI does not
/// need a restart before `/model` sees newly configured providers.
fn custom_provider_store() -> &'static RwLock<CustomProviders> {
    static CACHE: OnceLock<RwLock<CustomProviders>> = OnceLock::new();
    CACHE.get_or_init(|| {
        RwLock::new(shared_providers(with_builtin_seed(
            load_custom_providers_from_env(),
        )))
    })
}

/// The catalog as it stands, shared: a lookup costs one reference count, not a
/// copy of every declared model id (a connected `OpenRouter` lists 300+), and
/// the routing, wire-id and limit lookups each take one per call.
type CustomProviders = Arc<[Arc<ResolvedCustomProvider>]>;

fn shared_providers(providers: Vec<ResolvedCustomProvider>) -> CustomProviders {
    providers.into_iter().map(Arc::new).collect()
}

fn custom_provider_snapshot() -> CustomProviders {
    Arc::clone(
        &custom_provider_store()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Replace the process-local custom-provider catalog from a JSON array. This is
/// the live companion to the startup env bridge: `/connect` writes settings,
/// then calls this so model routing and the picker reflect the new provider in
/// the already-running process.
pub fn refresh_custom_providers_from_json(raw: &str) -> Result<(), serde_json::Error> {
    let providers = shared_providers(with_builtin_seed(parse_resolved_custom_providers(raw)?));
    let mut guard = custom_provider_store()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = providers;
    Ok(())
}

/// Re-read the process-local custom-provider catalog from
/// `ZO_CUSTOM_PROVIDERS`.
pub fn refresh_custom_providers_from_env() {
    let mut guard = custom_provider_store()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = shared_providers(with_builtin_seed(load_custom_providers_from_env()));
}

/// Split an explicit `provider/model` selection into `(provider, model)`.
///
/// Used when the same model id is served by both a built-in provider (e.g.
/// Anthropic OAuth `claude-opus-4-8`) and a custom gateway (e.g. agentrouter).
/// Bare ids keep the built-in route; only a qualified selection opts into the
/// custom provider.
///
/// A gateway's own model ids carry a slash (`anthropic/claude-x` on
/// `OpenRouter`), so the split is made at the configured provider's NAME when
/// the first segment names one — `openrouter/anthropic/claude-x` is
/// (`openrouter`, `anthropic/claude-x`). Only when no configured provider is
/// named does the last slash decide, as before.
#[must_use]
pub fn split_provider_model_ref(model: &str) -> Option<(&str, &str)> {
    let trimmed = model.trim();
    if trimmed.contains("://") {
        return None;
    }
    if let Some((first, rest)) = trimmed.split_once('/') {
        let first = first.trim();
        let rest = rest.trim();
        if !first.is_empty()
            && !rest.is_empty()
            && custom_provider_snapshot()
                .iter()
                .any(|provider| provider_name_matches(provider.config.provider_name, first))
        {
            return Some((first, rest));
        }
    }
    let (provider, model_id) = trimmed.rsplit_once('/')?;
    let provider = provider.trim();
    let model_id = model_id.trim();
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some((provider, model_id))
}

fn provider_name_matches(configured: &str, requested: &str) -> bool {
    let configured = configured.trim();
    let requested = requested.trim();
    if configured.eq_ignore_ascii_case(requested) {
        return true;
    }
    // Allow `agent-router` / `agent_router` to match display name `agent router`.
    let normalize = |value: &str| {
        value
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    !normalize(configured).is_empty() && normalize(configured) == normalize(requested)
}

/// The custom provider that serves `model`, if any.
///
/// Bare model ids only match when the id is not a built-in Anthropic/OpenAI/
/// Google/xAI alias collision that should keep first-party routing. Explicit
/// `provider/model` selections always resolve to the named custom provider so
/// gateways like agentrouter can serve the same Claude id without hijacking
/// OAuth.
#[must_use]
pub fn custom_provider_for_model(model: &str) -> Option<Arc<ResolvedCustomProvider>> {
    // An explicit `<provider>/<model>` naming a configured provider that
    // serves the model half is that provider's, before any whole-id match: a
    // second gateway listing `deepseek/deepseek-chat` must not take a
    // `DeepSeek/deepseek-chat` pick, its data or its bill.
    if let Some((named, _)) = named_provider_ref(model) {
        return Some(named);
    }
    // Then a provider that declares the WHOLE id: `anthropic/claude-x`
    // declared by a gateway is that gateway's model, not a vendor prefix. A
    // bare id the built-in registry names (`claude-opus-4-8`, `opus`) is the
    // exception: a gateway listing it too must not take a person's
    // first-party model — `<provider>/<model>` is the opt-in.
    let declared = custom_provider_snapshot()
        .iter()
        .find(|provider| provider.serves(model))
        .cloned()?;
    let bare_builtin =
        !model.contains('/') && catalog_entry_for_token(model_registry(), model).is_some();
    (!bare_builtin).then_some(declared)
}

/// The configured provider an explicit `<provider>/<model>` names, when it
/// serves the model half, with that model id in the operator's declared
/// casing.
fn named_provider_ref(model: &str) -> Option<(Arc<ResolvedCustomProvider>, String)> {
    let (provider_name, model_id) = split_provider_model_ref(model)?;
    custom_provider_snapshot().iter().find_map(|provider| {
        if !provider_name_matches(provider.config.provider_name, provider_name) {
            return None;
        }
        let declared = provider.canonical_model(model_id)?.to_string();
        Some((Arc::clone(provider), declared))
    })
}

/// Format a picker / `/model` selection that must route through a specific
/// custom provider even when the model id collides with a built-in.
#[must_use]
pub fn format_provider_model_ref(provider: &str, model: &str) -> String {
    format!("{}/{}", provider.trim(), model.trim())
}

/// Wire model id to send to the remote API for `model`.
///
/// For bare ids this is [`resolve_model_alias`]. For `provider/model` this is
/// the model half only (e.g. `agent router/claude-opus-4-8` → `claude-opus-4-8`).
#[must_use]
pub fn wire_model_id(model: &str) -> String {
    // The named provider's model half, in the operator's casing — the same
    // precedence `custom_provider_for_model` routes by.
    if let Some((_, declared)) = named_provider_ref(model) {
        return declared;
    }
    // A declared id goes to its gateway whole, in the operator's casing.
    if let Some(declared) = custom_provider_snapshot()
        .iter()
        .find_map(|provider| provider.canonical_model(model).map(str::to_string))
    {
        return declared;
    }
    if let Some((_provider, model_id)) = split_provider_model_ref(model) {
        return resolve_model_alias(model_id);
    }
    resolve_model_alias(model)
}

/// One limit of the custom provider serving `canonical_model` (else
/// `raw_model`), read for the model it was found by.
fn custom_provider_limit(
    raw_model: &str,
    canonical_model: &str,
    limit: impl Fn(&ResolvedCustomProvider, &str) -> Option<u64>,
) -> Option<u64> {
    [canonical_model, raw_model]
        .into_iter()
        .find_map(|model| custom_provider_for_model(model).map(|provider| limit(&provider, model)))
        .flatten()
}

fn custom_provider_context_window(raw_model: &str, canonical_model: &str) -> Option<u64> {
    custom_provider_limit(
        raw_model,
        canonical_model,
        ResolvedCustomProvider::context_window_for,
    )
}

fn custom_provider_max_output_tokens(raw_model: &str, canonical_model: &str) -> Option<u64> {
    custom_provider_limit(
        raw_model,
        canonical_model,
        ResolvedCustomProvider::max_output_tokens_for,
    )
}

/// Configured custom providers as `(display name, model ids)` for UIs such as
/// the model picker, so models reached via `/connect` (Ollama / LM Studio /
/// `DeepSeek` / …) appear alongside the built-ins. Empty unless
/// `ZO_CUSTOM_PROVIDERS` is populated (the bootstrap mirrors settings.json
/// into it).
#[must_use]
pub fn custom_provider_catalog() -> Vec<(&'static str, Vec<String>)> {
    custom_provider_snapshot()
        .iter()
        .map(|provider| (provider.config.provider_name, provider.models.clone()))
        .collect()
}

fn custom_provider_is_usable(provider: &ResolvedCustomProvider) -> bool {
    !provider.requires_auth
        || provider.config.credential_env_vars.iter().any(|env| {
            env_non_empty(env)
                || crate::oauth_store::load_openai_compat_api_key(env)
                    .ok()
                    .flatten()
                    .is_some_and(|value| !value.trim().is_empty())
        })
}

/// Custom provider usability details for UIs that need to explain why a
/// configured provider is hidden from Smart Router's usable routing pool.
#[must_use]
pub fn custom_provider_usability_catalog() -> Vec<CustomProviderUsability> {
    custom_provider_snapshot()
        .iter()
        .map(|provider| {
            let usable = custom_provider_is_usable(provider);
            CustomProviderUsability {
                name: provider.config.provider_name,
                models: provider.models.clone(),
                requires_auth: provider.requires_auth,
                credential_env_vars: provider.config.credential_env_vars.to_vec(),
                usable,
            }
        })
        .collect()
}

/// Custom providers whose declared models are usable for Smart Router display
/// and routing. Auth-required providers are included only when one of their
/// credential env vars is present in env or saved in the credential store.
#[must_use]
pub fn custom_provider_usable_catalog() -> Vec<(&'static str, Vec<String>)> {
    custom_provider_snapshot()
        .iter()
        .filter(|provider| custom_provider_is_usable(provider))
        .map(|provider| (provider.config.provider_name, provider.models.clone()))
        .collect()
}

/// Optional read-only fit hint for `model`.
///
/// The feature is disabled by default and never drives selection or serving.
/// When enabled, callers may display the static/catalog metadata as an
/// operator hint.
#[must_use]
#[allow(unused_variables)]
pub fn fit_hint_for_model(model: &str) -> Option<ModelFitHint> {
    #[cfg(feature = "model-fit-hints")]
    {
        let canonical = resolve_model_alias(model);
        let lower = canonical.to_ascii_lowercase();
        if let Some(entry) = model_registry().iter().find(|entry| {
            entry.alias == lower || entry.canonical_model_id.eq_ignore_ascii_case(&canonical)
        }) {
            return entry.fit_hint;
        }
        custom_provider_for_model(&canonical).and_then(|provider| provider.fit_hint)
    }

    #[cfg(not(feature = "model-fit-hints"))]
    {
        None
    }
}

#[must_use]
pub fn provider_enabled(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::Anthropic)
        || explicit_non_claude_adapter_gate_enabled()
        || provider_configured(kind)
}

/// Strict provider predicate for Smart Router usable-model inventory.
///
/// Unlike [`provider_enabled`], this ignores broad experimental adapter flags.
/// Smart routing may display/select only providers that are actually configured,
/// plus Anthropic's default first-party path.
#[must_use]
pub fn provider_usable_for_smart_inventory(kind: ProviderKind) -> bool {
    provider_configured(kind)
}

/// `true` when a saved ChatGPT OAuth subscription token exists. A logged-in
/// ChatGPT subscription activates the OpenAI provider (model picker + routing)
/// without needing an API key or the experimental-adapters flag — the
/// subscription itself is the credential.
fn openai_oauth_present() -> bool {
    !crate::managed_account::external_credentials_disabled()
        && crate::oauth_store::load_openai_oauth()
            .ok()
            .flatten()
            .is_some()
}

fn explicit_non_claude_adapter_gate_enabled() -> bool {
    [EXPERIMENTAL_PROVIDERS_ENV, NON_CLAUDE_ADAPTERS_ENV]
        .into_iter()
        .any(|key| {
            std::env::var(key).ok().is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
        })
}

fn provider_configured(kind: ProviderKind) -> bool {
    match kind {
        ProviderKind::Anthropic => true,
        ProviderKind::OpenAi => {
            openai_oauth_present()
                || env_non_empty(kind.metadata().auth_env)
                || env_non_empty(kind.metadata().base_url_env)
        }
        ProviderKind::Google => {
            gemini_code_assist::oauth_present()
                || google_auth::gemini_oauth_available()
                || env_non_empty(kind.metadata().auth_env)
                || env_non_empty(kind.metadata().base_url_env)
        }
        ProviderKind::Xai | ProviderKind::Ollama => {
            env_non_empty(kind.metadata().auth_env) || env_non_empty(kind.metadata().base_url_env)
        }
    }
}

#[must_use]
pub fn non_claude_adapters_enabled() -> bool {
    // Explicit opt-in flag (either spelling) wins.
    if explicit_non_claude_adapter_gate_enabled() {
        return true;
    }
    // Implicit: any non-Claude provider the user has actually *configured* —
    // via OAuth/ADC, API key, or a custom base URL — auto-enables non-Claude
    // model-family detection. Individual provider routing still uses
    // `provider_enabled(kind)`, so configuring Google no longer enables xAI.
    [
        ProviderKind::Xai,
        ProviderKind::OpenAi,
        ProviderKind::Google,
        ProviderKind::Ollama,
    ]
    .into_iter()
    .any(provider_configured)
}

/// Whether environment variable `key` is set to a non-empty (after trim) value.
fn env_non_empty(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
        || adopted_table_key(key).or_else(|| window_keychain_key(key)).is_some()
}

/// Resolve a user-facing model alias to its canonical model id.
///
/// This is the single source of truth shared between the API client and the
/// CLI: it consults the [`provider_catalog`] first, then recovers an
/// unambiguous *near-miss* of a built-in alias (a typo like `fable5` → `fable`),
/// and finally falls back to normalising fully-qualified Claude ids
/// (dot-versioned → hyphenated). Experimental (non-Claude) aliases stay
/// untouched while their adapter gate is disabled, so the caller surfaces an
/// "unsupported provider" error rather than silently dialing out.
///
/// The near-miss recovery is what stops a mistyped alias from being passed
/// through verbatim to a provider that rejects it with an opaque `404
/// not_found` (the `fable5`/gemini-typo trap): a close typo of a real alias now
/// resolves to that alias's canonical id instead. It is deliberately
/// conservative — only a *unique*, *tight* match snaps — so distinct real
/// models (`gpt-5.6-sol` vs `gpt-5.5`), fully-qualified canonical ids, and
/// custom-provider model ids are never rerouted.
#[must_use]
pub fn resolve_model_alias(model: &str) -> String {
    let trimmed = model.trim();

    // Explicit provider/model keeps the selection token intact for routing /
    // session persistence. Wire id extraction is `wire_model_id`.
    if split_provider_model_ref(trimmed).is_some() {
        return trimmed.to_string();
    }

    let lower = trimmed.to_ascii_lowercase();
    let allow_experimental = non_claude_adapters_enabled();

    if let Some(entry) = model_registry().iter().find(|entry| entry.alias == lower) {
        if provider_enabled(entry.provider) || allow_experimental {
            return entry.canonical_model_id.to_string();
        }
        return trimmed.to_string();
    }

    // Fallback: a user-defined provider (ZO_CUSTOM_PROVIDERS). The static
    // registry above always wins, so a bare custom id can never shadow a
    // built-in alias. Explicit `provider/model` is handled above.
    if let Some(custom) = custom_provider_for_model(&lower) {
        if let Some(canonical) = custom.canonical_model(&lower) {
            return canonical.to_string();
        }
    }

    // Near-miss recovery: snap a typo to the single built-in alias it clearly
    // meant (`fable5` → `fable` → `claude-fable-5`) instead of forwarding the
    // bogus id to a provider that 404s it. Skipped for an already-canonical
    // registry id (a valid fully-qualified target must never be snapped to a
    // shorter alias) and gated on the target provider being enabled, exactly
    // like the exact-alias path above, so a Gemini/GPT typo under a Claude-only
    // setup passes through as "unsupported" rather than dialing out.
    let is_known_canonical = model_registry()
        .iter()
        .any(|entry| entry.canonical_model_id.eq_ignore_ascii_case(&lower));
    if !is_known_canonical {
        if let Some(entry) = nearest_alias_entry(&lower) {
            if provider_enabled(entry.provider) || allow_experimental {
                return entry.canonical_model_id.to_string();
            }
        }
    }

    // Displayed short form of a known model → its canonical id. Canonical Claude
    // (and DeepSeek) ids spell the version with hyphens (`claude-opus-4-8`), but
    // the model and the user naturally type the *displayed* short form
    // (`opus-4.8`, `fable-5`, `sonnet-5`), which the provider 404s verbatim
    // (`not_found_error: model: opus-4.8`). Snap it to the canonical only when
    // dot→hyphen normalization — optionally under the `claude-` family prefix —
    // lands EXACTLY on a known canonical id. This is version-preserving by
    // construction: `opus-5` → `claude-opus-5` resolves, but a genuinely
    // different version like `opus-4.6` normalizes to `claude-opus-4-6`, which is
    // no canonical, so it still passes through untouched — and providers whose
    // canonical ids keep dots (GPT `gpt-5.5-mini`, Gemini `gemini-3.7-flash`)
    // never match the dashed candidate, so their short forms are unaffected.
    // The near-miss guard above deliberately refuses cross-version snaps and so
    // cannot cover this same-version reformatting.
    let dashed = lower.replace('.', "-");
    let prefixed = format!("claude-{dashed}");
    for candidate in [dashed.as_str(), prefixed.as_str()] {
        if let Some(entry) = model_registry()
            .iter()
            .find(|entry| entry.canonical_model_id.eq_ignore_ascii_case(candidate))
        {
            if provider_enabled(entry.provider) || allow_experimental {
                return entry.canonical_model_id.to_string();
            }
        }
    }

    normalize_model_id(trimmed)
}

/// A token shorter than this is never read as a model name: two letters
/// match nothing in the catalog and would only invite accidents.
const MIN_MODEL_TOKEN_LEN: usize = 3;

/// The canonical model a single word names EXACTLY — a registry alias
/// (`fable`, `opus`, `sol`), a canonical id, or a displayed short form
/// (`opus-5`, `fable-5`) — gated on the provider being reachable like
/// [`resolve_model_alias`], but with NO near-miss snapping: this reads the
/// person's own prose, where `table` is a table and must never become Fable.
/// `None` for anything else.
#[must_use]
pub fn exact_model_reference(token: &str) -> Option<&'static str> {
    let lower = token.trim().to_ascii_lowercase();
    if lower.len() < MIN_MODEL_TOKEN_LEN {
        return None;
    }
    let allow_experimental = non_claude_adapters_enabled();
    let reachable = |entry: &ProviderCatalogEntry| provider_enabled(entry.provider) || allow_experimental;
    // The displayed short form: dots to hyphens, optionally behind the
    // provider's own id prefix (`opus-5` → `claude-opus-5`), the same
    // reformatting `resolve_model_alias` accepts — read per entry so each
    // provider's prefix comes from the one table that knows it.
    let dashed = lower.replace('.', "-");
    model_registry()
        .iter()
        .find(|entry| {
            let prefixed = provider_id_prefix(entry.provider).map(|prefix| format!("{prefix}-{dashed}"));
            reachable(entry)
                && (entry.alias == lower
                    || entry.canonical_model_id.eq_ignore_ascii_case(&lower)
                    || entry.canonical_model_id.eq_ignore_ascii_case(&dashed)
                    || prefixed.is_some_and(|prefixed| entry.canonical_model_id.eq_ignore_ascii_case(&prefixed)))
        })
        .map(|entry| entry.canonical_model_id)
}

/// The registry row whose alias names `family` under `provider`, in either
/// spelling the catalog uses: bare (`opus`, `sol`) or behind the provider's
/// id prefix (`claude-opus`, `gemini-flash`).
fn family_alias_entry(
    provider: ProviderKind,
    family: &str,
) -> Option<&'static ProviderCatalogEntry> {
    let prefixed = provider_id_prefix(provider).map(|prefix| format!("{prefix}-{family}"));
    model_registry().iter().find(|entry| {
        entry.provider == provider
            && (entry.alias.eq_ignore_ascii_case(family)
                || prefixed
                    .as_deref()
                    .is_some_and(|alias| entry.alias.eq_ignore_ascii_case(alias)))
    })
}

/// Canonical id of the current catalog head for `model`'s family — the
/// model-selection seam for semantic requests such as `opus`, `claude-latest`
/// or a stale dated id. Callers never need to know which release currently
/// heads a family; re-pointing the alias moves every default/fallback/spawn
/// path at once. `None` when the family has no alias row.
#[must_use]
pub fn latest_family_model(model: &str) -> Option<&'static str> {
    let resolved = resolve_catalog_alias(model);
    let provider = catalog_provider_of(&resolved)?;
    let family = model_family(&resolved)?;
    family_alias_entry(provider, &family).map(|entry| entry.canonical_model_id)
}

/// [`latest_family_model`] for callers that hand it an Anthropic name by
/// contract (`opus`, `claude-latest`); any other provider's id yields `None`.
#[must_use]
pub fn latest_anthropic_family_model(model: &str) -> Option<&'static str> {
    let resolved = resolve_catalog_alias(model);
    (catalog_provider_of(&resolved) == Some(ProviderKind::Anthropic))
        .then(|| latest_family_model(&resolved))
        .flatten()
}

/// Catalog-owned one-step fallback for a provider model starved by rate
/// limits: the `demotes_to` its family alias declares, else the one its own
/// row declares (a retired lineup), resolved to the current release — and to
/// that release's priority spelling when the starved model was on one.
/// `None` means the bottom of the ladder, not "unknown".
///
/// Exact release ids live only in the catalog. Callers receive the current
/// registered target and never infer a release from naming convention or
/// stale model knowledge.
#[must_use]
pub fn starvation_demotion_model(model: &str) -> Option<String> {
    let lower = resolve_catalog_alias(model).trim().to_ascii_lowercase();
    let provider = catalog_provider_of(&lower)?;
    let rung = model_family(&lower)
        .and_then(|family| family_alias_entry(provider, &family))
        .and_then(|entry| entry.demotes_to.map(str::to_string))
        .or_else(|| catalog_fact(|catalog| catalog.demotes_to_for(model, &lower)))?;
    let target = resolve_catalog_alias(&rung);
    if openai_fast_tier_enabled(&lower) {
        if let Some((_, fast)) = openai_fast_variant_pair(&target) {
            return Some(fast);
        }
    }
    Some(target)
}

/// Catalog-owned candidate list for a safety-classifier refusal on `model`, in
/// preference order and resolved to current releases: the `refusal_fallback`
/// its family alias declares, else the one its own row declares. Empty means
/// the lineup declares no fallback — a refusal there is surfaced after one
/// same-model retry, not routed — and also covers a model no catalog row
/// knows. The runtime's refusal path reads this and names no lineup: which
/// classifier declines benign requests, and which families stand in for it,
/// are facts about the catalog.
///
/// The list mixes providers on purpose (Fable's own Opus head first, then a
/// cross-provider escape): the runtime retries a same-provider candidate on
/// the bound client and hands the turn to a cross-provider one like a quota
/// fallback, skipping any whose provider is not connected.
#[must_use]
pub fn refusal_fallback_candidates(model: &str) -> Vec<String> {
    let lower = resolve_catalog_alias(model).trim().to_ascii_lowercase();
    let Some(provider) = catalog_provider_of(&lower) else {
        return Vec::new();
    };
    let declared = model_family(&lower)
        .and_then(|family| family_alias_entry(provider, &family))
        .map(|entry| entry.refusal_fallback.iter().map(|alias| (*alias).to_string()).collect::<Vec<_>>())
        .filter(|list: &Vec<String>| !list.is_empty())
        .unwrap_or_else(|| catalog_fact(|catalog| {
            let list = catalog.refusal_fallback_for(model, &lower);
            (!list.is_empty()).then_some(list)
        }).unwrap_or_default());
    declared.iter().map(|alias| resolve_catalog_alias(alias)).collect()
}

/// The head of [`refusal_fallback_candidates`] — the first, most-preferred
/// place a refusal on `model` goes. `None` when the lineup declares none.
#[must_use]
pub fn refusal_fallback_model(model: &str) -> Option<String> {
    refusal_fallback_candidates(model).into_iter().next()
}

/// Resolve model-authored spawn input to a registered Claude family target.
///
/// This composes with [`resolve_model_alias`] but adds one spawn-only rule:
/// unqualified Claude family ids — including known previous releases emitted
/// from stale model knowledge — resolve to the current catalog head. An exact
/// historical pin remains available as explicit `anthropic/model`; custom and
/// non-Claude ids retain normal passthrough semantics. Main-session model
/// selection deliberately continues to call the permissive resolver directly.
#[must_use]
pub fn resolve_registered_model_alias(model: &str) -> String {
    let trimmed = model.trim();
    let resolved = resolve_model_alias(trimmed);

    // Explicit provider routing and operator-declared model ids are intentional
    // pins. Spawn hardening must not rewrite them.
    if split_provider_model_ref(trimmed).is_some() || custom_provider_for_model(trimmed).is_some() {
        return resolved;
    }
    if let Some(head) = latest_family_model(&resolved) {
        return head.to_string();
    }
    if model_registry()
        .iter()
        .any(|entry| entry.canonical_model_id.eq_ignore_ascii_case(&resolved))
    {
        return resolved;
    }
    explicit_non_claude_provider_kind(&resolved)
        .and_then(latest_model_for_provider)
        .map_or(resolved, str::to_string)
}

/// The single built-in alias that is an unambiguous near-miss of `lower`, or
/// `None` when nothing is close enough or the closest is a tie. Guards a typo
/// (`fable5` → `fable`) while refusing to reroute a distinct model: `gpt-5.6`
/// is edit-distance 1 from *both* `gpt-5.6-sol` and `gpt-5.5`, so it is ambiguous
/// and returns `None`.
///
/// Three guards keep it conservative, so it only ever fires on a confident
/// human typo, never on an intended-but-unknown id (custom-provider models,
/// new canonical ids):
/// - unique closest — a tie means we cannot know which was meant;
/// - edit distance ≤ 2 — single/double fat-finger only;
/// - shared 3-char prefix — you typed at least the first three characters
///   correctly, which stops junk (`op`) from snapping onto a short alias
///   (`opus`) and stops far matches that merely happen to fall within 2.
fn nearest_alias_entry(lower: &str) -> Option<&'static ProviderCatalogEntry> {
    // Only alias-like inputs are candidates; a long structured id is never a
    // "typo of an alias", and bounding the length keeps the DP trivially cheap.
    if lower.is_empty() || lower.chars().count() > 32 {
        return None;
    }
    let mut best_dist = usize::MAX;
    let mut best_entry: Option<&'static ProviderCatalogEntry> = None;
    let mut unique = false;
    for entry in model_registry() {
        let dist = levenshtein(lower, entry.alias);
        if dist < best_dist {
            best_dist = dist;
            best_entry = Some(entry);
            unique = true;
        } else if dist == best_dist {
            unique = false;
        }
    }
    let entry = best_entry?;
    (unique
        && best_dist <= 2
        && best_dist < entry.alias.chars().count()
        && shared_prefix_len(lower, entry.alias) >= 3
        // Never turn one model into a DIFFERENT version of another: a typo may
        // only differ from its alias in the *word*, never in an explicit
        // version/qualifier number. This is what keeps `opus-5`, `sonnet-4`,
        // `grok-4`, `gpt-5.5-mini`, and `gemini-3.7-flash` from silently
        // snapping onto an older/cheaper sibling — a semantic reroute that is
        // worse than a clean "unsupported model" error — while `fable5`,
        // `fabel`, and `opuss` still recover (they carry no version token).
        && version_tokens(lower) == version_tokens(entry.alias))
    .then_some(entry)
}

/// The explicit version/qualifier tokens of a model id, in order: each maximal
/// run that *starts with a digit at a field boundary* (string start, or right
/// after `-`/`.`) and extends over the following alphanumerics and dots.
///
/// This captures `5.6` in `gpt-5.6-sol`, `5` in `opus-5`, and `5.6x` in `gpt-5.6x`
/// (trailing letters glued to a version are part of the version spec, so a
/// suffix change is treated as a distinct model). It deliberately does NOT
/// capture the `5` in `fable5`: that digit follows a letter, so it is a
/// fat-finger on the `fable` alias rather than an explicit version field.
fn version_tokens(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let prev_is_boundary = i == 0 || matches!(chars[i - 1], '-' | '.');
        if chars[i].is_ascii_digit() && prev_is_boundary {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else {
            i += 1;
        }
    }
    out
}

/// Levenshtein edit distance over `char`s (so multibyte input never panics on a
/// byte index). Bounded by the ≤32-char guard in [`nearest_alias_entry`], so
/// the two-row DP is cheap.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Count leading `char`s shared by `a` and `b`.
fn shared_prefix_len(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .count()
}

/// Normalize dot-separated version numbers to hyphens in fully-qualified
/// Claude model ids (e.g. `claude-opus-4.6` → `claude-opus-4-6`). Other
/// providers' ids are passed through verbatim.
fn normalize_model_id(model: &str) -> String {
    if model.starts_with("claude-") && model.contains('.') {
        model.replace('.', "-")
    } else {
        model.to_string()
    }
}

#[must_use]
pub fn metadata_for_model(model: &str) -> Option<ProviderMetadata> {
    model_capability_for_model(model)
        .provider
        .map(ProviderKind::metadata)
}

#[must_use]
pub fn explicit_non_claude_provider_kind(model: &str) -> Option<ProviderKind> {
    let lower = model.trim().to_ascii_lowercase();
    if lower.starts_with("grok") {
        return Some(ProviderKind::Xai);
    }
    if is_openai_model(&lower) {
        return Some(ProviderKind::OpenAi);
    }
    if lower.starts_with("gemini") {
        return Some(ProviderKind::Google);
    }
    if std::env::var("OLLAMA_BASE_URL").is_ok() {
        return Some(ProviderKind::Ollama);
    }
    None
}

/// The maker attribution used in the non-Anthropic identity override, or
/// `None` when zo must not assert a specific maker.
///
/// zo's base system prompt hardcodes a Claude Code identity ("You are Claude
/// Code…"). Served by any non-Anthropic provider, that text makes the model
/// introduce itself as Claude. (The prompt's `Model family:` line is NOT part
/// of the problem — it already follows the bound model, so a GPT session's
/// prompt names GPT.) Each
/// first-party non-Anthropic provider returns its real maker so the override
/// names it; `Ollama` returns `None` (zo cannot know which model/lab serves
/// a local endpoint, so it corrects the Claude claim without inventing a false
/// maker). `Anthropic` returns `None` because its path is never rewritten — the
/// verbatim identity line is a Claude Max OAuth fingerprint requirement.
#[must_use]
pub fn maker_for_provider(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::OpenAi => Some("OpenAI"),
        ProviderKind::Xai => Some("xAI"),
        ProviderKind::Google => Some("Google"),
        ProviderKind::Anthropic | ProviderKind::Ollama => None,
    }
}

/// Prepend the non-Anthropic identity override to a joined system prompt so a
/// non-Claude model does not introduce itself as Claude. Single source of truth
/// shared by every non-Anthropic backend (ChatGPT/Responses, Gemini Code
/// Assist, and the OpenAI-compatible chat path).
///
/// Returns `system` unchanged when it is empty (nothing to override). Otherwise
/// it prepends a one-paragraph override naming the model and — when `maker` is
/// `Some` — its maker, instructing the model to follow zo's Claude-authored
/// operating manual and provider-neutral response style contract while keeping
/// its own identity. With `maker == None` the override still corrects the
/// Claude claim but asserts no specific maker, so a custom/self-hosted endpoint
/// is never mislabeled.
///
/// The Anthropic backend must never call this: its first system block has to be
/// the verbatim Claude Code identity line (OAuth fingerprint requirement).
#[must_use]
pub fn apply_non_anthropic_identity(system: &str, model: &str, maker: Option<&str>) -> String {
    if system.is_empty() {
        return system.to_string();
    }
    let maker_clause = match maker {
        Some(maker) => format!("a large language model made by {maker}"),
        None => "a large language model".to_string(),
    };
    format!(
        "You are {model}, {maker_clause}, operating through the zo coding CLI. The operating \
         manual below was written for Claude Code; follow its tools, workflow, and provider-neutral \
         response style contract, but your identity is {model} — do not claim to be Claude or to \
         be made by Anthropic.\n\n{system}"
    )
}

#[must_use]
pub fn detect_provider_kind(model: &str) -> ProviderKind {
    if let Some(metadata) = metadata_for_model(model) {
        return metadata.provider;
    }
    if anthropic::has_auth_from_env_or_saved().unwrap_or(false) {
        return ProviderKind::Anthropic;
    }
    if non_claude_adapters_enabled() {
        if gemini_code_assist::oauth_present()
            || openai_compat::has_api_key("GOOGLE_API_KEY")
            || google_auth::gemini_oauth_available()
        {
            return ProviderKind::Google;
        }
        if openai_compat::has_api_key("OPENAI_API_KEY") {
            return ProviderKind::OpenAi;
        }
        if openai_compat::has_api_key("XAI_API_KEY") {
            return ProviderKind::Xai;
        }
    }
    if std::env::var("OLLAMA_BASE_URL").is_ok() {
        return ProviderKind::Ollama;
    }
    ProviderKind::Anthropic
}

/// Flatten a tool result's content blocks into a single plain-text string for
/// providers whose `tool`/`function_call_output` messages accept only text.
///
/// Blocks are joined by `\n`; JSON blocks are serialized; image blocks degrade
/// to a `[image <media-type>]` placeholder rather than being dropped silently.
/// Shared by the OpenAI-compatible, ChatGPT/Responses, and Gemini Code Assist
/// backends so the flattening stays byte-identical across them.
pub(crate) fn flatten_tool_result_content(
    content: &[crate::types::ToolResultContentBlock],
) -> String {
    use crate::types::ToolResultContentBlock;

    let mut flattened = String::new();
    for (index, block) in content.iter().enumerate() {
        if index > 0 {
            flattened.push('\n');
        }
        match block {
            ToolResultContentBlock::Text { text } => flattened.push_str(text),
            ToolResultContentBlock::Json { value } => flattened.push_str(&value.to_string()),
            ToolResultContentBlock::Image { source } => {
                flattened.push_str("[image ");
                flattened.push_str(&source.media_type);
                flattened.push(']');
            }
        }
    }
    flattened
}

/// The `data:<media-type>;base64,<data>` URL for an inline image, shared by the
/// OpenAI-compatible and ChatGPT/Responses backends (both send images as data
/// URLs inside an `image_url`). Gemini uses a distinct `inline_data` structure,
/// not a data URL, so it does not use this.
pub(crate) fn image_data_url(source: &crate::types::ImageSource) -> String {
    format!("data:{};base64,{}", source.media_type, source.data)
}

/// Exponential backoff schedule shared by the retrying provider clients
/// (Anthropic, OpenAI-compatible, ChatGPT/Responses): `initial_backoff`
/// doubled per attempt (`attempt` is 1-based), capped at `max_backoff`.
///
/// Returns [`crate::error::ApiError::BackoffOverflow`] when the doubling
/// multiplier (`1 << (attempt - 1)`) would overflow `u32`; the caller adds
/// jitter via `retry_backoff::spread_backoff`.
pub(crate) fn backoff_for_attempt(
    attempt: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
) -> Result<Duration, crate::error::ApiError> {
    let Some(multiplier) = 1_u32.checked_shl(attempt.saturating_sub(1)) else {
        return Err(crate::error::ApiError::BackoffOverflow {
            attempt,
            base_delay: initial_backoff,
        });
    };
    Ok(initial_backoff
        .checked_mul(multiplier)
        .map_or(max_backoff, |delay| delay.min(max_backoff)))
}

/// Default per-chunk idle budget for provider streams (ms). A streaming read that
/// receives no bytes for this long is aborted with a retryable
/// [`crate::error::ApiError::stream_idle_timeout`] instead of hanging the turn
/// forever — a half-open TCP connection or a backend that holds the socket open
/// while it reasons silently would otherwise freeze the spinner indefinitely
/// (and let a spawned sub-agent overrun its wall-clock budget, since the
/// iteration-boundary deadline check never runs while a read is parked).
/// Sized well above any normal inter-chunk gap.
pub(crate) const STREAM_IDLE_TIMEOUT_MS: u64 = 90_000;

/// Env override for [`STREAM_IDLE_TIMEOUT_MS`]; `0` disables the idle timeout
/// (restores the unbounded-wait behaviour). Shared by the Anthropic and
/// OpenAI-compatible streams; the ChatGPT/Responses backend keeps its own
/// `ZO_CHATGPT_STREAM_IDLE_TIMEOUT_MS` because it also drives an in-place
/// restart on idle.
pub(crate) const STREAM_IDLE_TIMEOUT_ENV: &str = "ZO_STREAM_IDLE_TIMEOUT_MS";

/// Resolve the per-chunk idle budget, honouring the env override. `None` means
/// "no timeout" (override set to `0`).
#[must_use]
pub(crate) fn stream_idle_timeout() -> Option<std::time::Duration> {
    let millis = std::env::var(STREAM_IDLE_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(STREAM_IDLE_TIMEOUT_MS);
    (millis > 0).then(|| std::time::Duration::from_millis(millis))
}

// ---------------------------------------------------------------------------
// Mid-stream restart predicate (shared by every streaming backend)
// ---------------------------------------------------------------------------
//
// A streaming turn can be transparently re-opened only while nothing has been
// surfaced yet — once a non-empty text or tool-argument delta has crossed into
// the caller's render path, restarting would duplicate output, so the fault
// must propagate instead. The Anthropic Messages API and the ChatGPT/Responses
// backend share this rule (neither has a resumable-offset token; recovery means
// re-sending the request), so the predicate lives here as the single source of
// truth rather than being duplicated per backend.

/// Pure restart predicate (unit-testable without a live socket): restart only
/// while the turn is uncommitted, the fault is retryable, and the restart
/// budget is not yet spent.
#[must_use]
pub(crate) fn should_restart(
    committed: bool,
    retryable: bool,
    attempts: u32,
    max_retries: u32,
) -> bool {
    !committed && retryable && attempts < max_retries
}

/// Default total wall-clock ceiling over a pre-commit restart sequence,
/// shared by the OpenAI-compatible and Gemini Code Assist streams (the
/// ChatGPT/Responses backend keeps its own equal `MAX_RESTART_WALLCLOCK`).
/// Attempt counting alone lets a *silent* backend hold the turn for
/// idle-timeout × restarts; this bounds the whole sequence by elapsed time.
pub(crate) const DEFAULT_MAX_RESTART_WALLCLOCK: Duration = Duration::from_secs(120);

/// Restart predicate with an added total wall-clock budget over the restart
/// sequence. A pre-commit stream can stall (idle-timeout) and re-open up to
/// `max_retries` times; each idle wait can be tens of seconds, so the attempt
/// count alone lets a *silent* backend hold the turn for minutes (the observed
/// ~275 s freeze: idle-timeout × restarts with no overall ceiling). This bounds
/// the whole sequence by elapsed time too, so a backend that never produces a
/// byte fails out promptly instead of looping the full budget. `elapsed` is the
/// time since the first restart in the sequence (`None` before any restart, so
/// the first restart is always allowed); the wall-clock gate only tightens
/// [`should_restart`], never loosens it.
#[must_use]
pub(crate) fn should_restart_within_budget(
    committed: bool,
    retryable: bool,
    attempts: u32,
    max_retries: u32,
    elapsed: Option<Duration>,
    max_wallclock: Duration,
) -> bool {
    should_restart(committed, retryable, attempts, max_retries)
        && elapsed.is_none_or(|e| e < max_wallclock)
}

/// Whether an event surfaces output that a restart would duplicate. Only
/// non-empty text and tool-call argument deltas commit; message/block framing,
/// reasoning deltas, and empty placeholders are bookkeeping — replaying them
/// after a stalled stream is preferable to wedging the turn forever.
#[must_use]
pub(crate) fn crosses_restart_commit_boundary(event: &StreamEvent) -> bool {
    match event {
        StreamEvent::ContentBlockDelta(delta) => match &delta.delta {
            ContentBlockDelta::TextDelta { text } => !text.is_empty(),
            ContentBlockDelta::InputJsonDelta { partial_json } => !partial_json.is_empty(),
            ContentBlockDelta::ThinkingDelta { .. } | ContentBlockDelta::SignatureDelta { .. } => {
                false
            }
        },
        StreamEvent::MessageStart(_)
        | StreamEvent::MessageDelta(_)
        | StreamEvent::ContentBlockStart(_)
        | StreamEvent::ContentBlockStop(_)
        | StreamEvent::MessageStop(_) => false,
    }
}

/// Shared sink for mid-stream restart notices, held by every streaming
/// backend that performs transparent pre-commit restarts (ChatGPT, the
/// OpenAI-compatible adapter, Gemini Code Assist).
///
/// A transparent restart never returns an error the establish-time retry
/// layer could render, so without a notice the turn just freezes for the
/// backoff. Wrapping the optional callback here keeps the "fire if installed"
/// pattern in one place and lets stream structs keep `#[derive(Debug)]`
/// (a bare `dyn Fn` is not `Debug`).
pub(crate) struct StreamRetryNotifier(
    Option<std::sync::Arc<dyn Fn(core_types::StreamRetryNotice) + Send + Sync>>,
);

impl StreamRetryNotifier {
    /// No sink installed (the default): notices are dropped, preserving the
    /// log-only behaviour for non-interactive callers.
    pub(crate) const fn none() -> Self {
        Self(None)
    }

    pub(crate) fn install(
        &mut self,
        callback: impl Fn(core_types::StreamRetryNotice) + Send + Sync + 'static,
    ) {
        self.0 = Some(std::sync::Arc::new(callback));
    }

    pub(crate) fn notify(&self, notice: core_types::StreamRetryNotice) {
        if let Some(callback) = &self.0 {
            callback(notice);
        }
    }
}

impl std::fmt::Debug for StreamRetryNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamRetryNotifier")
            .field("installed", &self.0.is_some())
            .finish()
    }
}

fn model_id_matches_prefix_segment(model_id: &str, prefix: &str) -> bool {
    model_id == prefix
        || model_id
            .strip_prefix(prefix)
            .is_some_and(|suffix| matches!(suffix.as_bytes().first(), Some(b'-' | b'@' | b'[')))
}

const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 64_000;
const EXTENDED_MAX_OUTPUT_TOKENS: u32 = 128_000;
const LEGACY_OPUS_MAX_OUTPUT_TOKENS: u32 = 32_000;

#[must_use]
pub fn max_tokens_for_model(model: &str) -> u32 {
    model_capability_for_model(model)
        .max_output_tokens
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
}

fn max_output_tokens_for_model_id(raw_model: &str, model_id: &str) -> u32 {
    // Anthropic's current model table documents 128k synchronous max output for
    // Fable/Mythos 5, Sonnet 5, and Opus 4.6+; the streaming guide explicitly
    // uses `claude-opus-4-8` with `max_tokens: 128000` as the large-output path
    // that avoids HTTP timeouts. Let those models use the full documented cap.
    // Older Opus 4.1/4.0 variants remain at their documented 32k cap; everything
    // else keeps the prior conservative 64k default, which still prevents the
    // old 32k mid-tool-call truncation without over-claiming unknown providers.
    if model_id.contains("fable")
        || model_id.contains("mythos")
        || model_id_matches_prefix_segment(model_id, "claude-sonnet-5")
        || model_id_matches_prefix_segment(model_id, "claude-opus-5")
        || model_id_matches_prefix_segment(model_id, "claude-opus-4-8")
        || model_id_matches_prefix_segment(model_id, "claude-opus-4-7")
        || model_id_matches_prefix_segment(model_id, "claude-opus-4-6")
    {
        return EXTENDED_MAX_OUTPUT_TOKENS;
    }
    if model_id_matches_prefix_segment(model_id, "claude-opus-4-1")
        || model_id_matches_prefix_segment(model_id, "claude-opus-4-0")
        || model_id == "claude-opus-4"
        || model_id_matches_prefix_segment(model_id, "claude-opus-4-20250514")
    {
        return LEGACY_OPUS_MAX_OUTPUT_TOKENS;
    }
    // Non-Anthropic models: honour the docs-verified catalog cap (GPT-5.5 128k,
    // DeepSeek V4 384k) before the conservative default, so large refactors and
    // full-file writes are not truncated mid-generation — the same failure the
    // Anthropic path was widened to 128k to avoid, previously still present for
    // every other provider because the catalog field was never read.
    if let Some(max_out) = custom_provider_max_output_tokens(raw_model, model_id) {
        return u32::try_from(max_out).unwrap_or(u32::MAX);
    }
    if let Some(max_out) = catalog_max_output_tokens(raw_model, model_id) {
        return u32::try_from(max_out).unwrap_or(u32::MAX);
    }
    DEFAULT_MAX_OUTPUT_TOKENS
}

/// Whether `model` uses Anthropic *adaptive* thinking — the server sizes the
/// thinking budget from `output_config.effort` rather than an explicit
/// `thinking.budget_tokens`. True for the Opus 4.6+/Fable generation (and the
/// matching Sonnet), false for Opus 4.5 and earlier, which still take a budget.
///
/// This is the SSOT that decides which Anthropic wire shape a request gets, so
/// the request builder never sends a deprecated `budget_tokens` to a model that
/// expects effort, nor an unsupported `output_config` to a legacy model.
#[must_use]
pub fn uses_adaptive_thinking(model: &str) -> bool {
    model_capability_for_model(model)
        .adaptive_thinking
        .unwrap_or(false)
}

/// Whether `model` thinks on every request and cannot be told not to.
///
/// Fable and Mythos run adaptive thinking whether or not the request carries a
/// `thinking` field; `{type: "disabled"}` and a `budget_tokens` budget both
/// return 400. So a request to them with no `thinking` field is still a
/// thinking request, and the reasoning blocks in its history are valid.
#[must_use]
pub fn thinking_always_on(model: &str) -> bool {
    let canonical = resolve_catalog_alias(model).to_ascii_lowercase();
    canonical.contains("fable") || canonical.contains("mythos")
}

/// Whether `model` returns 400 on a forced tool choice (`tool_choice:
/// {type: "any"}` or `{type: "tool", name}`) — Claude Fable 5.1 and Mythos
/// 5.1, as Mythos Preview did. `auto` and `none` are unchanged there.
#[must_use]
pub fn rejects_forced_tool_choice(model: &str) -> bool {
    preserved_thinking_generation(model)
}

/// Whether `model` is the Fable/Mythos 5.1 generation, which binds each
/// thinking block to the model and the conversation prefix that produced it
/// (`thinking.block_binding`) and refuses forced tool use.
#[must_use]
pub fn preserved_thinking_generation(model: &str) -> bool {
    let canonical = resolve_catalog_alias(model).to_ascii_lowercase();
    let bare = canonical
        .split('[')
        .next()
        .unwrap_or_default()
        .replace('.', "-");
    [
        "claude-fable-5-1",
        "claude-mythos-5-1",
        "claude-mythos-preview",
    ]
    .iter()
    .any(|generation| model_id_matches_prefix_segment(&bare, generation))
}

fn adaptive_thinking_for_canonical(canonical_lower: &str) -> bool {
    // Fable is adaptive-only.
    if canonical_lower.contains("fable") {
        return true;
    }
    // Opus/Sonnet 4.6+ are adaptive; 4.5 and earlier are not. Match the
    // generation digits in the canonical id (e.g. `claude-opus-4-6`).
    if canonical_lower.contains("opus") || canonical_lower.contains("sonnet") {
        return !is_legacy_anthropic_generation(canonical_lower);
    }
    false
}

/// True when an Anthropic model id is the 4.5-or-earlier generation, which uses
/// legacy budget-based thinking. Recognizes the `4-5`/`4.5` (and older `-3`,
/// `4-0`…`4-5`) generation markers; anything newer (4-6+) is adaptive.
fn is_legacy_anthropic_generation(canonical_lower: &str) -> bool {
    const LEGACY_MARKERS: &[&str] = &[
        "4-5", "4.5", "4-4", "4.4", "4-3", "4.3", "4-2", "4.2", "4-1", "4.1", "4-0", "4.0", "-3-",
        "claude-3",
    ];
    LEGACY_MARKERS
        .iter()
        .any(|marker| canonical_lower.contains(marker))
}

/// Map a thinking budget (in tokens) to a provider-neutral [`EffortLevel`], so a
/// request that only carries a budget can still drive an adaptive model's
/// `output_config.effort` (and a GPT backend's `reasoning_effort`).
///
/// The thresholds are the exact inverse of the CLI `Effort` preset budgets
/// (`Effort::budget()`): each preset round-trips to the level it came from
/// (`Low` from 1024, `Medium` from 4096, `High` from 10000, `Xhigh` from 16000,
/// `Max` from 24000). This function has no `Ultra` bucket — it predates the
/// named Ultra tier and only derives a level from a raw budget number — so
/// `Effort::Ultra`'s 26000 budget and `Effort::Smart`'s 28000 budget both land
/// in the open-ended `Max` bucket rather than round-tripping to `Ultra`;
/// callers that carry the named `Effort::Ultra` preset (or `Effort::Smart`'s
/// dynamic band) must use `Effort::level()`/`Effort::band_ceiling()` directly
/// to get `Ultra`, not this budget-only fallback. The CLI-side
/// `Effort::level()` is the single source of truth for the non-Ultra
/// pairings, and a round-trip test in `effort_picker` fails loudly if the two
/// ever drift.
/// (P9 note: `Ultracode` was renamed to `Smart`, and the `ultra` token — a
/// mere alias of `Ultracode` before P9 — is now this separate static
/// `Effort::Ultra` preset in its own right; see `effort_picker.rs`.)
///
/// A previous calibration was tuned for large legacy budgets (`Max` needed a
/// budget above 48k), so every preset landed one to two tiers low on the wire:
/// a headless `ZO_EFFORT=max` reached Anthropic as `high`, and `xhigh`
/// reached GPT as `medium`. Boundaries now sit at the midpoint between
/// consecutive presets, so an off-preset custom budget snaps to the nearest
/// tier, and any budget above the top preset clamps to `Max`.
/// Combine a request's configured thinking budget with an optional escalation
/// **floor** (e.g. the deep-gate's `ApiRequest::effort_override`). The floor can
/// only raise effort, never lower it: the result is `max(configured, floor)`
/// when a floor is present, otherwise the configured budget unchanged. A zero or
/// absent floor is inert. Both inputs are `Option` so "no thinking configured"
/// (`None`) still escalates to the floor when one is set.
#[must_use]
pub fn effort_budget_with_floor(configured: Option<u32>, floor: Option<u32>) -> Option<u32> {
    match floor.filter(|&f| f > 0) {
        Some(f) => Some(configured.map_or(f, |c| c.max(f))),
        None => configured,
    }
}

#[must_use]
pub fn effort_level_for_budget(budget_tokens: u32) -> crate::types::EffortLevel {
    use crate::types::EffortLevel;
    match budget_tokens {
        0..=2_560 => EffortLevel::Low,         // Low preset = 1_024
        2_561..=7_048 => EffortLevel::Medium,  // Medium preset = 4_096
        7_049..=13_000 => EffortLevel::High,   // High preset = 10_000
        13_001..=20_000 => EffortLevel::Xhigh, // Xhigh preset = 16_000
        _ => EffortLevel::Max,                 // Max preset = 24_000
    }
}

/// The model-specific Zo tier used for selection, routing, and display for a
/// requested `level`. UI surfaces can ask this without branching on provider.
///
/// This is a **read-only capability projection**, not the final wire string.
/// A backend still applies its provider serializer after this selection; in
/// particular, GPT `Max`/`Ultra` are internal Zo tiers that both serialize
/// as the provider-supported `xhigh` value.
///
/// - Anthropic, OpenAI and Google ids: [`highest_accepted_effort`] — the
///   catalog row's declared scale, else the provider's documented one. A Sonnet
///   without `xhigh` clamps `Xhigh -> High`; a GPT without `max` clamps
///   `Max -> Xhigh`; Gemini 3 tops out at `high`. GPT fast keeps its ceiling
///   because `/fast` is service priority, not an effort ceiling.
/// - Everything else (xAI, Ollama, unknown/custom non-OpenAI ids): **pass
///   through unchanged** — never silently downgrade a model whose ceiling we do
///   not know (the BUG-R14 trap: env-fallback misclassifying a custom provider).
///
/// Detection is **name/family-based**, deliberately *not* env-sensitive
/// [`detect_provider_kind`], so the result is deterministic and a custom
/// OpenAI-compatible model id is never re-tiered off ambient `OPENAI_API_KEY`.
#[must_use]
pub fn effective_effort_for_model(
    level: crate::types::EffortLevel,
    model: &str,
) -> crate::types::EffortLevel {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    // A first-party lineup clamps to what its catalog row (or its provider's
    // documented scale) accepts. `/fast` is service priority and never lowers
    // a ceiling. xAI / Ollama / unknown / custom non-OpenAI ids pass through
    // unchanged: we do not know their ceiling, and guessing risks a silent
    // downgrade; the wire path applies whatever the provider enforces.
    match effort_scale_provider(&lower) {
        Some(_) => highest_accepted_effort(&lower, level),
        None => level,
    }
}

/// `level` when `model` accepts it, else the nearest lower level it does. Every
/// declared and documented scale includes `low`, so this never walks off the
/// ladder; a model whose scale zo does not know accepts everything.
#[must_use]
pub fn highest_accepted_effort(model: &str, level: crate::types::EffortLevel) -> crate::types::EffortLevel {
    walk_down_to_accepted(level, |candidate| model_accepts_effort(model, candidate))
}

/// [`highest_accepted_effort`] for a wire projection that knows which
/// provider it serializes for: a model with no declared scale is clamped to
/// `provider`'s documented one rather than passed through, because that is
/// the enum the wire will actually validate against.
#[must_use]
pub fn highest_accepted_effort_on(
    provider: ProviderKind,
    model: &str,
    level: crate::types::EffortLevel,
) -> crate::types::EffortLevel {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    let accepted = accepted_effort_levels(model, &lower, Some(provider));
    walk_down_to_accepted(level, |candidate| {
        accepted.as_ref().is_none_or(|levels| levels.contains(&candidate))
    })
}

fn walk_down_to_accepted(
    level: crate::types::EffortLevel,
    accepts: impl Fn(crate::types::EffortLevel) -> bool,
) -> crate::types::EffortLevel {
    let mut rank = effort_rank(level);
    loop {
        let candidate = effort_level_from_rank(rank);
        if rank == 0 || accepts(candidate) {
            return candidate;
        }
        rank -= 1;
    }
}

/// The effort levels `model` accepts: its catalog row's declaration, else
/// the scale `scale` documents for every model it serves, else `None`
/// (unknown).
fn accepted_effort_levels(
    raw_model: &str,
    canonical_lower: &str,
    scale: Option<ProviderKind>,
) -> Option<Vec<crate::types::EffortLevel>> {
    catalog_fact(|catalog| catalog.effort_levels_for(raw_model, canonical_lower))
        .or_else(|| scale.and_then(provider_effort_scale).map(<[crate::types::EffortLevel]>::to_vec))
}

/// The provider whose documented scale applies to `canonical_lower` when its
/// row declares none: Anthropic, OpenAI and Gemini ids. `None` for xAI /
/// Ollama / unknown / custom non-OpenAI ids — a scale zo cannot vouch for.
fn effort_scale_provider(canonical_lower: &str) -> Option<ProviderKind> {
    if canonical_lower.starts_with("claude") {
        Some(ProviderKind::Anthropic)
    } else if is_openai_model(canonical_lower) {
        Some(ProviderKind::OpenAi)
    } else if canonical_lower.starts_with("gemini") {
        Some(ProviderKind::Google)
    } else {
        None
    }
}

/// What a provider documents for every model it serves, when a row says
/// nothing more: Anthropic's `output_config.effort` base scale (`xhigh` is
/// per model), OpenAI's `reasoning_effort` enum before the 5.6 line added
/// `max`, Gemini 3's `thinkingLevel`.
fn provider_effort_scale(provider: ProviderKind) -> Option<&'static [crate::types::EffortLevel]> {
    use crate::types::EffortLevel::{High, Low, Max, Medium, Xhigh};
    match provider {
        ProviderKind::Anthropic => Some(&[Low, Medium, High, Max]),
        ProviderKind::OpenAi => Some(&[Low, Medium, High, Xhigh]),
        ProviderKind::Google => Some(&[Low, Medium, High]),
        ProviderKind::Xai | ProviderKind::Ollama => None,
    }
}

/// Whether `model` accepts `level`: a provider-declared capability fact from
/// its catalog row, else its provider's documented scale, else `true` for a
/// model whose scale zo does not know (never silently downgrade it).
#[must_use]
pub fn model_accepts_effort(model: &str, level: crate::types::EffortLevel) -> bool {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    accepted_effort_levels(model, &lower, effort_scale_provider(&lower))
        .is_none_or(|levels| levels.contains(&level))
}

/// Whether `model` actually accepts the `xhigh` reasoning tier — the
/// provider-neutral predicate UI surfaces use to decide whether to flag a
/// `xhigh`/`smart` selection as clamped. Defined in terms of
/// [`effective_effort_for_model`] so it can never disagree with the projection.
#[must_use]
pub fn model_supports_xhigh(model: &str) -> bool {
    use crate::types::EffortLevel;
    effective_effort_for_model(EffortLevel::Xhigh, model) == EffortLevel::Xhigh
}

/// Env var holding a JSON object of `{"model-or-family-prefix": "ultra|max|xhigh|high"}`
/// ceiling overrides, consulted before every built-in rule in
/// [`max_supported_effort`]. This is the zero-rebuild escape hatch for a newly
/// announced model's effort ceiling — mirrors how [`MODEL_CONTEXT_WINDOWS_ENV`]
/// lets an operator inject capability facts without recompiling. Longest
/// matching prefix wins (segment-boundary aware, see
/// `crate::types::model_id_matches_family`), so a specific override
/// (`gpt-5.7-nova`) beats a broader one (`gpt-5.7`) in the same JSON blob.
pub const MODEL_EFFORT_CEILINGS_ENV: &str = "ZO_MODEL_EFFORT_CEILINGS";

/// `"xhigh"` → [`EffortLevel::Xhigh`](crate::types::EffortLevel): the spelling
/// the catalog, Codex's `supported_reasoning_levels` and the ceiling override
/// share. Codex's `minimal`/`none` rungs fold into `low`.
#[must_use]
pub fn parse_effort_level(value: &str) -> Option<crate::types::EffortLevel> {
    use crate::types::EffortLevel;
    match value.trim().to_ascii_lowercase().as_str() {
        "low" | "minimal" | "none" => Some(EffortLevel::Low),
        "medium" => Some(EffortLevel::Medium),
        "high" => Some(EffortLevel::High),
        "xhigh" => Some(EffortLevel::Xhigh),
        "max" => Some(EffortLevel::Max),
        "ultra" => Some(EffortLevel::Ultra),
        _ => None,
    }
}

/// Read fresh (not cached) so tests can set/unset the env var per-case — the
/// same non-caching choice [`catalog_fact`] makes for the sibling catalog
/// override.
fn env_effort_ceiling_override(canonical_lower: &str) -> Option<crate::types::EffortLevel> {
    let raw = std::env::var(MODEL_EFFORT_CEILINGS_ENV).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let overrides: std::collections::HashMap<String, String> = serde_json::from_str(&raw).ok()?;
    let mut best: Option<(usize, crate::types::EffortLevel)> = None;
    for (prefix, value) in &overrides {
        let prefix_lower = prefix.trim().to_ascii_lowercase();
        if prefix_lower.is_empty() {
            continue;
        }
        if !crate::types::model_id_matches_family(canonical_lower, &prefix_lower) {
            continue;
        }
        let Some(level) = parse_effort_level(value) else {
            continue;
        };
        if best.is_none_or(|(best_len, _)| prefix_lower.len() > best_len) {
            best = Some((prefix_lower.len(), level));
        }
    }
    best.map(|(_, level)| level)
}

/// Single source of truth for the highest internal [`crate::types::EffortLevel`]
/// tier Zo exposes for a `model` — consolidates what was previously
/// scattered across `crate::types::gpt_model_accepts_ultra`/
/// `crate::types::gpt_model_accepts_max`, the Anthropic xhigh/max split baked
/// into [`crate::types::EffortLevel::anthropic_for_model`], and the Gemini
/// `high` cap applied ad hoc inside [`effective_effort_for_model`].
///
/// This is read as a **provider-declared capability fact** (design principle:
/// only capability-derived signals feed the router), never a routing
/// preference — it does not depend on connectivity, pins, or outcome data.
///
/// Resolution order: [`MODEL_EFFORT_CEILINGS_ENV`] override (longest matching
/// prefix wins) → alias resolution ([`resolve_model_alias`]) → per-provider
/// family rule. Dated/`@`/`[`-suffixed ids resolve through the same
/// alias/family-matching machinery every other capability lookup in this file
/// uses, so `gpt-5.6-sol-2026-07-09` and `gpt-5.6-terra@openai` get the same
/// ceiling as their bare family.
///
/// The ceiling is the top of the model's accepted scale: its catalog row's
/// `effort_levels` (Sol/Terra declare `ultra`, Luna `max`, Haiku stops at
/// `max` without `xhigh`), else the scale its provider documents for every
/// model (Anthropic `low|medium|high|max`, OpenAI `…|xhigh`, Gemini 3
/// `…|high`). Everything else (xAI, Ollama, unknown/custom non-OpenAI) gets a
/// conservative `High`. That is a capability *unknown*, not a capability
/// grant — it deliberately does NOT return `Ultra`/`Max` on the strength of
/// `effective_effort_for_model`'s pass-through wire tolerance, since that
/// would let an unrecognized model over-claim a Deep-tier promotion (see
/// `model_inventory::tiers_for_model` in the runtime crate).
#[must_use]
pub fn max_supported_effort(model: &str) -> crate::types::EffortLevel {
    known_effort_ceiling(model).unwrap_or(crate::types::EffortLevel::High)
}

/// The effort ceiling Zo can actually *vouch for*, or `None` when the model's
/// ceiling is *unknown* (xAI, Ollama, custom OpenAI-compatible providers, any
/// unrecognized id).
///
/// [`max_supported_effort`] collapses that `None` to a conservative `High`
/// because its consumer — the router's Deep-tier promotion
/// (`model_inventory::effort_ceiling_for_model`) — must not hand an
/// unrecognized model a capability *grant* it never declared. But "we do not
/// know this model's ceiling" and "this model tops out at high" are different
/// facts, and clamping a dynamic effort BAND against the conservative stand-in
/// is what silently pinned `/effort smart` to a single rung on custom
/// providers: the band's floor and ceiling both clamped to rank 2, so every
/// request resolved to `high` no matter how many difficulty signals fired.
///
/// So band clamping ([`resolve_effort_band`]) reads THIS function and leaves an
/// unknown ceiling unclamped, matching [`effective_effort_for_model`]'s
/// established pass-through rule for the same models (never silently downgrade
/// a model whose ceiling we do not know). A real ceiling can still be declared
/// per-model through [`MODEL_EFFORT_CEILINGS_ENV`], which is honored here first
/// and therefore counts as *known*.
#[must_use]
pub fn known_effort_ceiling(model: &str) -> Option<crate::types::EffortLevel> {
    let lower = resolve_catalog_alias(model).to_ascii_lowercase();
    if let Some(level) = env_effort_ceiling_override(&lower) {
        return Some(level);
    }
    accepted_effort_levels(model, &lower, effort_scale_provider(&lower))
        .and_then(|levels| levels.into_iter().max_by_key(|level| effort_rank(*level)))
}

// ---------------------------------------------------------------------------
// Dynamic effort-band resolver
//
// `Effort::Smart` (formerly `Ultracode`) no longer pins a single static
// wire level: it carries a FLOOR (`MessageRequest::effort`, always `Xhigh`
// in practice) plus a requested CEILING (`MessageRequest::effort_band_ceiling`,
// `Max`). Each wire backend resolves the band to one concrete `EffortLevel`
// per request via `resolve_effort_band`. This module owns the per-request
// difficulty classification outright — the GPT backend's own auto-effort
// ladder was folded into it, so no backend keeps a second copy.
// ---------------------------------------------------------------------------

/// Heavy-reasoning intent keywords (EN + KO), matched as substrings/stems of
/// the last user message. Single source of truth for per-request difficulty
/// classification — [`band_difficulty_for_request`] is its only consumer; do
/// not duplicate it elsewhere.
pub(crate) const HEAVY_INTENT: &[&str] = &[
    "analy",
    "debug",
    "refactor",
    "architect",
    "design",
    "review",
    "audit",
    "investigat",
    "optimiz",
    "compare",
    "benchmark",
    "분석",
    "디버",
    "리팩",
    "설계",
    "아키텍",
    "리뷰",
    "감사",
    "조사",
    "최적",
    "비교",
    "벤치",
];

/// Concatenate the plain-text blocks of one message (ignores tool calls,
/// results, and binary parts) — the per-request difficulty signal shared by
/// the GPT auto-effort ladder and the ultra-band resolver.
pub(crate) fn message_plain_text(message: &crate::types::InputMessage) -> String {
    use crate::types::InputContentBlock;
    let mut text = String::new();
    for block in &message.content {
        if let InputContentBlock::Text { text: value, .. } = block {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(value);
        }
    }
    text
}

/// The most recent user-authored message's plain text, or empty if none.
pub(crate) fn last_user_message_text(request: &crate::types::MessageRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(message_plain_text)
        .unwrap_or_default()
}

/// Total plain-text character count across every message — the large-context
/// difficulty proxy (~6k tokens at the 24_000-char threshold).
pub(crate) fn total_message_text_chars(request: &crate::types::MessageRequest) -> usize {
    use crate::types::InputContentBlock;
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            InputContentBlock::Text { text, .. } => Some(text.len()),
            _ => None,
        })
        .sum()
}

/// Per-request difficulty signals for the dynamic ultra band: a heavy-intent
/// keyword hit on the last user message, a large accumulated context
/// (>`24_000` chars, ~6k tokens), and a long single ask (>600 chars). Mirrors
/// (and is the sole source for) the GPT auto-effort ladder's existing triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BandDifficulty {
    pub heavy_intent: bool,
    pub large_context: bool,
    pub long_ask: bool,
}

impl BandDifficulty {
    /// Number of difficulty signals that fired (0..=3) — the escalation rung
    /// count [`resolve_effort_band`] steps by.
    #[must_use]
    pub fn signal_count(self) -> u8 {
        u8::from(self.heavy_intent) + u8::from(self.large_context) + u8::from(self.long_ask)
    }
}

/// Classify a request's difficulty signals from its message content — the
/// single source [`resolve_effort_band`] and the GPT auto-effort ladder both
/// read, so a heavy-intent Korean prompt escalates identically whether it
/// rides the dynamic ultra band or the legacy Auto ladder.
#[must_use]
pub fn band_difficulty_for_request(request: &crate::types::MessageRequest) -> BandDifficulty {
    let last_user = last_user_message_text(request);
    let total_chars = total_message_text_chars(request);
    let lower = last_user.to_lowercase();
    BandDifficulty {
        heavy_intent: HEAVY_INTENT.iter().any(|kw| lower.contains(kw)),
        large_context: total_chars > 24_000,
        long_ask: last_user.chars().count() > 600,
    }
}

/// Env var kill switch for the dynamic ultra band: `ZO_ULTRA_BAND=off`
/// restores the pre-band static-top behavior — [`resolve_effort_band`] always
/// returns the clamped ceiling regardless of per-request difficulty signals.
pub const ULTRA_BAND_ENV: &str = "ZO_ULTRA_BAND";

fn ultra_band_disabled() -> bool {
    std::env::var(ULTRA_BAND_ENV).is_ok_and(|value| value.trim().eq_ignore_ascii_case("off"))
}

/// Rank an [`crate::types::EffortLevel`] on the shared low→ultra scale.
/// Callers may compare named and budget-derived levels without changing their
/// independent budget, routing, or ceiling policies.
#[must_use]
pub const fn effort_rank(level: crate::types::EffortLevel) -> u8 {
    use crate::types::EffortLevel;
    match level {
        EffortLevel::Low => 0,
        EffortLevel::Medium => 1,
        EffortLevel::High => 2,
        EffortLevel::Xhigh => 3,
        EffortLevel::Max => 4,
        EffortLevel::Ultra => 5,
    }
}

fn effort_level_from_rank(rank: u8) -> crate::types::EffortLevel {
    use crate::types::EffortLevel;
    match rank {
        0 => EffortLevel::Low,
        1 => EffortLevel::Medium,
        2 => EffortLevel::High,
        3 => EffortLevel::Xhigh,
        4 => EffortLevel::Max,
        _ => EffortLevel::Ultra,
    }
}

/// Resolve a dynamic effort BAND — `[floor ..= min(ceiling, known_effort_ceiling(model))]`
/// — to the single concrete internal [`crate::types::EffortLevel`] this request
/// selects. Called once per request, BEFORE the backend's per-model
/// wire projection (`gpt_for_model`/`anthropic_for_model`/`gemini`) — the
/// resolved level then flows through that projection exactly as if it had
/// been the named `effort` all along.
///
/// Escalation rule: zero difficulty signals stay at `floor`; exactly one
/// signal steps up one rung (clamped to the ceiling); two or more signals
/// jump straight to the ceiling. In the one real caller (`Effort::Smart`,
/// floor=Xhigh, ceiling=Max) this is 0→xhigh and 1+→ceiling (`max` on
/// sol/terra/fable/luna, `high` on gemini). The one- and two-signal rungs
/// coincide there because the band is two rungs wide by design — automatic
/// escalation tops out at `max`, and `ultra` is reachable only through the
/// explicit `Effort::Ultra` pin.
///
/// `ZO_ULTRA_BAND=off` disables the dynamic behavior entirely: every call
/// returns the clamped ceiling, matching the pre-band static-top pin.
///
/// Properties (see the `resolve_effort_band_*` tests): the result never
/// exceeds `min(ceiling, known_effort_ceiling(model))` (just `ceiling` when the
/// model's ceiling is unknown), never falls below
/// `floor` (a misconfigured `ceiling < floor` degenerates to always
/// returning the clamped ceiling, never below it), and is a deterministic,
/// pure function of its inputs (for a fixed `ZO_ULTRA_BAND`).
#[must_use]
pub fn resolve_effort_band(
    floor: crate::types::EffortLevel,
    ceiling: crate::types::EffortLevel,
    model: &str,
    difficulty: BandDifficulty,
) -> crate::types::EffortLevel {
    // Clamp only against a ceiling we actually know ([`known_effort_ceiling`]),
    // NOT against `max_supported_effort`'s conservative `High` stand-in for
    // unknown models: that stand-in exists to withhold a Deep-tier *grant*, and
    // reusing it here collapsed the whole band onto one rung for every custom
    // provider (floor and ceiling both clamped to `High`, so the difficulty
    // signals below could never move the pick). An unknown ceiling stays
    // unclamped — the same pass-through `effective_effort_for_model` applies to
    // the resolved level right after this.
    let ceiling_rank = match known_effort_ceiling(model) {
        Some(model_ceiling) => effort_rank(ceiling).min(effort_rank(model_ceiling)),
        None => effort_rank(ceiling),
    };
    let floor_rank = effort_rank(floor).min(ceiling_rank);

    if ultra_band_disabled() {
        return effort_level_from_rank(ceiling_rank);
    }

    let picked_rank = match difficulty.signal_count() {
        0 => floor_rank,
        1 => floor_rank.saturating_add(1),
        _ => ceiling_rank,
    };

    effort_level_from_rank(picked_rank.clamp(floor_rank, ceiling_rank))
}

/// Total input context window (in tokens) for `model`.
///
/// Anthropic windows are known from the Claude model ids/beta suffixes.
/// Providers that do not expose context-window fields through `/models` (notably
/// OpenAI's public model object) are resolved from the capability catalog loaded
/// via [`MODEL_CONTEXT_WINDOWS_ENV`] or the bundled docs-verified catalog. It is
/// **not** the per-response output cap — that is [`max_tokens_for_model`].
///
/// The lookup works off the canonical model id so both short aliases
/// (`opus`, `sonnet`, `gpt-5.5`) and fully-qualified ids
/// (`claude-opus-4-6`, `claude-opus-4-6[1m]`, `gpt-5.5-2026-04-23`) resolve
/// correctly.
#[must_use]
pub fn context_window_for_model(model: &str) -> u64 {
    model_capability_for_model(model)
        .context_window
        .unwrap_or(200_000)
}

fn context_window_for_canonical(raw_model: &str, canonical_lower: &str) -> u64 {
    // DECLARED FIRST. The operator's own custom-provider declaration, then the
    // catalog (which already honours the `ZO_MODEL_CONTEXT_WINDOWS` override).
    //
    // These used to sit BELOW the claude-name guesses, which made them
    // unreachable for any id containing `claude`: an operator running a gateway
    // that serves a Claude-compatible model could not state its real window,
    // because `contains("claude")` answered first. It also made the four
    // Anthropic rows in `model_context_windows.json` dead data — their own
    // `source` strings admitted as much.
    //
    // Reordering is a no-op for the built-in catalog: every Anthropic row
    // declares exactly what the guess below would have returned (fable/opus/
    // sonnet-5 → 1M, haiku → 258k), so this only changes the answer for a model
    // somebody actually declared. `context_window_declaration_beats_the_name_guess`
    // pins both halves of that.
    if let Some(window) = custom_provider_context_window(raw_model, canonical_lower) {
        return window;
    }
    if let Some(window) = catalog_model_context_window(raw_model, canonical_lower) {
        return window;
    }

    // Undeclared, from here down: name-shape guesses, last resort.
    //
    // Current flagship Claude models with documented native 1M windows.
    // Sonnet 5 is the first Sonnet in Zo's built-in catalog with a documented
    // 1M default/max window; older Sonnet and Haiku variants stay capped below.
    if canonical_lower.contains("fable")
        || canonical_lower.contains("opus")
        || model_id_matches_prefix_segment(canonical_lower, "claude-sonnet-5")
    {
        return 1_000_000;
    }
    // Older Sonnet and Haiku top out at 258k in the environments Zo targets.
    // Apply that cap before the conservative Claude fallback so HUD/compaction
    // never advertises more context than these models can actually accept.
    if canonical_lower.contains("sonnet") || canonical_lower.contains("haiku") {
        return 258_000;
    }

    if canonical_lower.contains("claude") {
        return 200_000;
    }

    // OpenAI-compatible fallback: conservative until a capability catalog entry
    // exists for the exact model. This avoids displaying fabricated precision
    // for new GPT/o/codex models.
    if canonical_lower.contains("gpt")
        || canonical_lower.contains("o3")
        || canonical_lower.contains("o4")
        || canonical_lower.contains("codex")
    {
        return 200_000;
    }

    // xAI Grok 3: 131k. Do not project that limit onto newer custom Grok ids;
    // use a custom-provider override or the conservative unknown-model fallback
    // until a docs-verified catalog entry exists.
    if model_id_matches_prefix_segment(canonical_lower, "grok-3") {
        return 131_072;
    }

    // Google Gemini: 1M
    if canonical_lower.contains("gemini") {
        return 1_000_000;
    }

    // Ollama: conservative default
    200_000
}

/// Read an environment variable, mapping an unset *or empty* value to `None`.
///
/// Shared by the Anthropic and OpenAI-compatible providers so the "empty
/// string means absent" credential-lookup rule has a single definition.
///
/// # Errors
/// Propagates a non-`NotPresent` [`crate::error::ApiError`] (e.g. a value that
/// is not valid Unicode).
pub(crate) fn read_env_non_empty(key: &str) -> Result<Option<String>, crate::error::ApiError> {
    Ok(read_env_key(key)?.map(|(value, _)| value))
}

/// The ladder itself: `key`'s value and the rung that answered for it.
///
/// One climb, in one order — the environment, then this process's own table,
/// then the window's keychain — so a caller that needs the rung and one that
/// needs only the value cannot walk two different ladders.
///
/// # Errors
/// A variable set to bytes that are not UTF-8.
pub(crate) fn read_env_key(
    key: &str,
) -> Result<Option<(String, KeySource)>, crate::error::ApiError> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Ok(Some((value, KeySource::Environment))),
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            Ok(adopted_table_key(key).or_else(|| window_keychain_key(key)))
        }
        Err(error) => Err(crate::error::ApiError::from(error)),
    }
}

/// Which rung of the ladder above answered for a key this process sends.
///
/// A person needs this named, not inferred: "no key" and "the window has one
/// you cannot see" are different problems, and before the third rung existed
/// every judgment a zo outside the window made was `no_key` (t-5805). It is
/// the same shape the OpenAI login carries for the same reason (t-5777's
/// `CodexHomeSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// ① The process environment — a shell, a harness or a CI leg set it.
    Environment,
    /// ② This process's own table ([`adopt_launch_keys`]): the window handed
    ///    it over at launch, under a prefix of ours.
    Adopted,
    /// ③ The window's keychain item, read once when the key was first needed
    ///    ([`find_service_keys_in_keychain`], [`find_router_keys_in_keychain`])
    ///    — the rung a zo the window did not launch stands on.
    WindowKeychain,
}

impl KeySource {
    /// The word a report, a pane or a JSON answer names this rung by.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Adopted => "adopted",
            Self::WindowKeychain => "window_keychain",
        }
    }
}

/// Keys this process holds outside its environment, by variable name, each
/// with the rung it came from ([`adopt_launch_keys`], [`window_keychain_key`]).
///
/// The rung is kept beside the value rather than re-derived on the next read:
/// the keychain is asked once per name and its answer is remembered here, so a
/// second reader that only saw this table would call a keychain key "adopted"
/// and a person looking for why their key works would be told the wrong story.
fn adopted_launch_keys() -> &'static RwLock<std::collections::HashMap<String, (String, KeySource)>>
{
    static ADOPTED: OnceLock<RwLock<std::collections::HashMap<String, (String, KeySource)>>> =
        OnceLock::new();
    ADOPTED.get_or_init(Default::default)
}

fn adopted_table_key(key: &str) -> Option<(String, KeySource)> {
    adopted_launch_keys()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(key)
        .cloned()
}

/// A key the window keeps that this process was not handed, read from the
/// window's keychain item the first time it is needed
/// ([`find_router_keys_in_keychain`], [`find_service_keys_in_keychain`]) and
/// kept in the adopted table. One reader at a time, so a caller that waited
/// finds what the first one read.
fn window_keychain_key(key: &str) -> Option<(String, KeySource)> {
    let keychains = window_keychains()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let keychain = keychains.iter().find(|keychain| keychain.names.answers(key))?;
    let mut asked = asked_window_keychain()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !asked.insert(key.to_string()) {
        return adopted_table_key(key);
    }
    match (keychain.read)(&format!("{}{key}", keychain.service_prefix)) {
        KeychainAnswer::Found(value) => {
            adopted_launch_keys()
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.to_string(), (value.clone(), KeySource::WindowKeychain));
            Some((value, KeySource::WindowKeychain))
        }
        KeychainAnswer::Absent => None,
        // Not an answer, so it is not one to remember: the next caller that
        // needs this key asks the keychain again rather than going without one
        // the machine has.
        KeychainAnswer::Unanswered => {
            asked.remove(key);
            None
        }
    }
}

/// Move every environment variable named with `prefix` out of the process
/// environment into this process's own key table, and answer how many moved.
///
/// The window hands zo the keys of the routers connected in its settings as
/// variables under one prefix, for zo's own requests. Left in the environment,
/// every child zo starts — an MCP server fetched with `npx …@latest`, a tool's
/// shell, a hook — inherits them. Adopted, they are still read by name here
/// (the credential lookup falls back to this table) and by nothing zo spawns.
///
/// Call once, first thing in `main`, before any thread exists: removing an
/// environment variable races every concurrent reader of the environment.
pub fn adopt_launch_keys(prefix: &str) -> usize {
    if prefix.is_empty() {
        return 0;
    }
    let found: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, value)| name.starts_with(prefix) && !value.trim().is_empty())
        .collect();
    let mut table = adopted_launch_keys()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (name, value) in &found {
        std::env::remove_var(name);
        table.insert(name.clone(), (value.clone(), KeySource::Adopted));
    }
    found.len()
}

/// Which variables one of the window's keychain roads answers for.
enum WindowKeyNames {
    /// Every variable under a prefix — the router keys, whose names the window
    /// spells itself.
    Prefix(String),
    /// Variables named exactly — a service key whose name the vendor's SDKs
    /// already fixed (`TYPESAFE_API_KEY`), so no prefix of ours marks it.
    Exactly(Vec<String>),
}

impl WindowKeyNames {
    fn answers(&self, key: &str) -> bool {
        match self {
            Self::Prefix(prefix) => key.starts_with(prefix.as_str()),
            Self::Exactly(names) => names.iter().any(|name| name == key),
        }
    }
}

/// Where the window keeps keys it did not hand this process: one keychain item
/// per variable, `service_prefix` and the variable's name.
struct WindowKeychain {
    names: WindowKeyNames,
    service_prefix: String,
    read: fn(&str) -> KeychainAnswer,
}

fn window_keychains() -> &'static RwLock<Vec<WindowKeychain>> {
    static KEYCHAINS: OnceLock<RwLock<Vec<WindowKeychain>>> = OnceLock::new();
    KEYCHAINS.get_or_init(Default::default)
}

/// The variables the window's keychain has answered about — found or absent,
/// each is asked once per process. A read it could not answer is not recorded
/// here, so it is asked again ([`window_keychain_key`]).
fn asked_window_keychain() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static ASKED: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    ASKED.get_or_init(Default::default)
}

/// Let this process find a window router key it was not handed — a zo typed
/// into a shell pane, which no window launch reached, or one run outside the
/// window. A variable named with `env_prefix` that is in neither the
/// environment nor the adopted table is read once, when zo first needs it,
/// from the keychain item `service_prefix` + its name — the item the window's
/// router pane writes (macOS) — and kept in the adopted table like a handed
/// key: never in the environment, so nothing zo spawns sees it. Nothing is
/// read here.
pub fn find_router_keys_in_keychain(env_prefix: &str, service_prefix: &str) {
    if env_prefix.is_empty() {
        return;
    }
    install_window_keychain(
        WindowKeyNames::Prefix(env_prefix.to_string()),
        service_prefix,
        anthropic::keychain::read_router_key,
    );
}

/// Let this process find a service key the window's settings keep under the
/// key's own variable name — TypeSafe's `TYPESAFE_API_KEY`, which every
/// TypeSafe SDK and this crate's [`crate::SystemOneConfig`] read by that name.
/// The same road as [`find_router_keys_in_keychain`], for names listed rather
/// than prefixed: read once, from `service_prefix` + the name, when first
/// needed, into the adopted table and never into the environment. Nothing is
/// read here.
pub fn find_service_keys_in_keychain(names: &[&str], service_prefix: &str) {
    if names.is_empty() {
        return;
    }
    install_window_keychain(
        WindowKeyNames::Exactly(names.iter().map(|name| (*name).to_string()).collect()),
        service_prefix,
        anthropic::keychain::read_router_key,
    );
}

fn install_window_keychain(names: WindowKeyNames, service_prefix: &str, read: fn(&str) -> KeychainAnswer) {
    window_keychains()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(WindowKeychain {
            names,
            service_prefix: service_prefix.to_string(),
            read,
        });
}

#[cfg(test)]
pub(crate) fn forget_adopted_launch_keys_for_test() {
    adopted_launch_keys()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    asked_window_keychain()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    window_keychains()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Append the *stable* prefix of a request's system blocks to `prefix`.
///
/// OpenAI prompt caching rewards stable content at the very beginning of the
/// prompt. Zo's system prompt is ordered identity/static/dynamic; the last
/// block is the per-session/project tail when a dynamic block exists. Keep the
/// routing key on the reusable prefix instead of fragmenting it by user/project
/// tail content. The request body still contains the full system text.
pub(crate) fn push_stable_system_prefix(prefix: &mut String, request: &crate::types::MessageRequest) {
    let Some(system) = &request.system else {
        return;
    };
    let stable_len = if system.len() > 1 {
        system.len() - 1
    } else {
        system.len()
    };
    for block in system.iter().take(stable_len) {
        let crate::types::SystemBlock::Text { text, .. } = block;
        prefix.push_str(text);
        prefix.push('\n');
    }
}

/// Append the conversation-stream discriminator — the text of the first user
/// message — to `prefix`. Distinct agents (main conversation, each fanout
/// spawn) open with distinct first messages, so this splits them into
/// distinct cache keys even when they share a session id; appending later
/// turns never changes it. Non-text opening blocks (images, tool results)
/// are skipped: text is present in every real opening prompt and is stable
/// across serialization details.
pub(crate) fn push_conversation_stream_discriminator(
    prefix: &mut String,
    request: &crate::types::MessageRequest,
) {
    let Some(first_user) = request
        .messages
        .iter()
        .find(|message| message.role == "user")
    else {
        return;
    };
    for block in &first_user.content {
        if let crate::types::InputContentBlock::Text { text, .. } = block {
            prefix.push_str(text);
        }
    }
    prefix.push('\n');
}

/// Derive the deterministic OpenAI `prompt_cache_key` for a request.
///
/// The routing key hashes the (wire) `model`, a non-empty session id, the
/// conversation-stream discriminator (the first user message's text), the
/// stable system prefix, and the tool schemas. `model` is passed explicitly
/// because the ChatGPT backend keys on the resolved wire model id rather than
/// `request.model`; the OpenAI-compatible path passes `&request.model`.
///
/// The stream discriminator is what keeps concurrent agents from sharing one
/// key: a fanout spawn re-stamps its requests with the parent's session id, so
/// under a session-only key every spawn's divergent transcript competed for
/// the same provider cache shard and evicted each other's prefixes (observed
/// live 07-20: sol cache reads pinned at the ~12k shared system prefix across
/// 400+ requests while each interleaved agent re-billed its full 100k+
/// history). The first user message is unique per conversation stream (the
/// user's opening prompt / the spawn's task prompt), never changes as turns
/// append, and when compaction rewrites it the key rolls over exactly when
/// the cached prefix is invalidated anyway.
pub(crate) fn prompt_cache_key(
    model: &str,
    request: &crate::types::MessageRequest,
    session_id: &str,
) -> String {
    use sha2::Digest;
    let mut prefix = String::new();
    prefix.push_str(model);
    prefix.push('\n');
    if !session_id.is_empty() {
        prefix.push_str(session_id);
        prefix.push('\n');
    }
    push_conversation_stream_discriminator(&mut prefix, request);
    push_stable_system_prefix(&mut prefix, request);
    if let Some(tools) = &request.tools {
        for tool in tools {
            prefix.push_str(&tool.name);
            prefix.push('\n');
            if let Some(description) = &tool.description {
                prefix.push_str(description);
                prefix.push('\n');
            }
            prefix.push_str(&tool.input_schema.to_string());
            prefix.push('\n');
        }
    }
    let digest = sha2::Sha256::digest(prefix.as_bytes());
    format!("zo-{digest:x}")[..64].to_string()
}

#[cfg(test)]
mod tests {
    use super::{preserved_thinking_generation, rejects_forced_tool_choice, thinking_always_on};
    use super::{
        BandDifficulty, EXPERIMENTAL_PROVIDERS_ENV, MODEL_CLASSES_ENV, MODEL_CONTEXT_WINDOWS_ENV,
        MODEL_EFFORT_CEILINGS_ENV,
        ModelClass, NON_CLAUDE_ADAPTERS_ENV,
        PromptCacheStrategy, ProviderKind, STREAM_IDLE_TIMEOUT_ENV, STREAM_IDLE_TIMEOUT_MS,
        ULTRA_BAND_ENV,
        apply_non_anthropic_identity, band_difficulty_for_request, context_window_for_model,
        declared_model_class, detect_provider_kind,
        effective_effort_for_model, effort_budget_with_floor, effort_level_for_budget, effort_rank,
        ProviderCatalogEntry, catalog_entry_for_token, catalog_provider_for_canonical,
        explicit_non_claude_provider_kind, fit_hint_for_model, is_openai_builtin_model_prefix,
        known_effort_ceiling, display_name_from_id, family_from_id, is_openai_lineup_word, is_openai_model,
        model_accepts_effort, model_display_name, model_family, model_has_capability,
        model_supports_vision,
        latest_model_for_provider, levenshtein, maker_for_provider, max_supported_effort,
        max_tokens_for_model, model_capability_for_model, model_supports_xhigh,
        non_claude_adapters_enabled, openai_fast_tier_enabled,
        openai_prompt_cache_retention, provider_catalog, resolve_effort_band, resolve_model_alias,
        resolve_registered_model_alias, shared_prefix_len, starvation_demotion_model,
        stream_idle_timeout, uses_adaptive_thinking, version_tokens,
    };

    #[test]
    fn effort_rank_exhaustively_orders_the_six_levels() {
        use crate::types::EffortLevel::{High, Low, Max, Medium, Ultra, Xhigh};

        const ULTRA_RANK: u8 = effort_rank(Ultra);
        let levels = [Low, Medium, High, Xhigh, Max, Ultra];
        let ranks = levels.map(effort_rank);
        assert_eq!(ranks, [0, 1, 2, 3, 4, 5]);
        assert!(ranks.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(ULTRA_RANK, 5);
    }

    #[test]
    fn maker_for_provider_names_first_party_non_anthropic_only() {
        // First-party non-Anthropic providers name their real maker …
        assert_eq!(maker_for_provider(ProviderKind::OpenAi), Some("OpenAI"));
        assert_eq!(maker_for_provider(ProviderKind::Xai), Some("xAI"));
        assert_eq!(maker_for_provider(ProviderKind::Google), Some("Google"));
        // … while Anthropic (never rewritten) and Ollama (unknown local model)
        // assert no maker.
        assert_eq!(maker_for_provider(ProviderKind::Anthropic), None);
        assert_eq!(maker_for_provider(ProviderKind::Ollama), None);
    }

    #[test]
    fn apply_non_anthropic_identity_overrides_claude_and_names_maker() {
        let system = "You are Claude Code, Anthropic's official CLI for Claude.\nDo work.";
        let out = apply_non_anthropic_identity(system, "gpt-5.5", Some("OpenAI"));
        // The original Claude-authored body is preserved …
        assert!(out.contains("Do work."), "body preserved: {out}");
        // … under an explicit identity override naming the model + maker.
        assert!(out.contains("You are gpt-5.5, a large language model made by OpenAI"));
        assert!(out.contains("provider-neutral response style contract"));
        assert!(out.contains("do not claim to be Claude or to be made by Anthropic"));
        assert!(out.ends_with(system), "override is a prefix of the body");
    }

    #[test]
    fn apply_non_anthropic_identity_uses_neutral_wording_without_maker() {
        // A custom/self-hosted model (no known maker) is still identity-corrected
        // but must not be mislabeled as made by a specific lab.
        let out = apply_non_anthropic_identity("manual", "deepseek-chat", None);
        assert!(out.contains("You are deepseek-chat, a large language model, operating"));
        assert!(
            !out.contains("made by OpenAI")
                && !out.contains("made by xAI")
                && !out.contains("made by Google"),
            "no false maker attribution: {out}"
        );
        assert!(out.contains("do not claim to be Claude"));
    }

    #[test]
    fn apply_non_anthropic_identity_passes_empty_system_through() {
        // Empty system → unchanged empty output (no stray identity block); this
        // is the short-circuit that also protects every backend's empty path.
        assert_eq!(
            apply_non_anthropic_identity("", "gpt-5.5", Some("OpenAI")),
            ""
        );
    }

    #[test]
    fn stream_idle_timeout_defaults_and_env_override() {
        // Env access is process-global, and the Anthropic mid-stream-restart
        // tests also drive STREAM_IDLE_TIMEOUT_ENV, so serialise on the shared
        // lock to avoid a cross-test read of a half-written value.
        let _guard = crate::test_env_lock();
        let key = STREAM_IDLE_TIMEOUT_ENV;
        let restore = std::env::var(key).ok();

        std::env::remove_var(key);
        assert_eq!(
            stream_idle_timeout(),
            Some(std::time::Duration::from_millis(STREAM_IDLE_TIMEOUT_MS)),
            "default budget applies when unset"
        );

        std::env::set_var(key, "1500");
        assert_eq!(
            stream_idle_timeout(),
            Some(std::time::Duration::from_millis(1_500)),
            "valid override is honoured"
        );

        std::env::set_var(key, "0");
        assert_eq!(
            stream_idle_timeout(),
            None,
            "zero disables the idle timeout"
        );

        std::env::set_var(key, "not-a-number");
        assert_eq!(
            stream_idle_timeout(),
            Some(std::time::Duration::from_millis(STREAM_IDLE_TIMEOUT_MS)),
            "garbage falls back to the default"
        );

        match restore {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// The alias catalog is data: an override can introduce a model the binary
    /// predates AND repoint a shipped alias at it, with no code change.
    ///
    /// Serialized against the other registry tests through the env lock — the
    /// registry is process-global, so the override is undone before returning.
    #[test]
    fn an_override_can_add_and_repoint_aliases() {
        let _lock = crate::test_env_lock();

        // Baseline: the shipped catalog decides what `google-latest` means.
        super::reset_model_registry_for_tests();
        assert_eq!(super::resolve_catalog_alias("google-latest"), "gemini-3.6-flash");
        assert_eq!(super::resolve_catalog_alias("gemini-3.7-flash"), "gemini-3.7-flash");

        super::refresh_model_registry_from_json(
            r#"{"aliases":[
                {"alias":"google-latest","canonical":"gemini-3.7-flash","provider":"google"},
                {"alias":"flash37","canonical":"gemini-3.7-flash","provider":"google"}
            ]}"#,
        );
        assert_eq!(
            super::resolve_catalog_alias("google-latest"),
            "gemini-3.7-flash",
            "an override row shadows the shipped one"
        );
        assert_eq!(
            super::resolve_catalog_alias("flash37"),
            "gemini-3.7-flash",
            "a brand-new alias resolves"
        );
        assert_eq!(
            super::resolve_catalog_alias("opus"),
            "claude-opus-5",
            "rows the override does not name are untouched"
        );

        super::reset_model_registry_for_tests();
        assert_eq!(super::resolve_catalog_alias("google-latest"), "gemini-3.6-flash");
    }

    /// The refusal fallback is catalog data, now an ordered candidate list per
    /// Anthropic lineup: every family names where a declined request goes next,
    /// in preference order, and the runtime reads this instead of matching
    /// family substrings. `refusal_fallback_model` is the head of that list.
    #[test]
    fn the_refusal_fallback_comes_from_the_catalog() {
        use super::{
            refusal_fallback_candidates, refusal_fallback_model, resolve_catalog_alias,
            resolve_model_alias, ANTHROPIC_OPUS_MODEL_ALIAS,
        };
        super::reset_model_registry_for_tests();
        let opus_head = resolve_model_alias(ANTHROPIC_OPUS_MODEL_ALIAS);
        let openai = resolve_catalog_alias("openai-latest");
        let google = resolve_catalog_alias("google-latest");

        // Fable: its sibling Opus head first, then a cross-provider escape.
        assert_eq!(
            refusal_fallback_candidates("fable"),
            vec![opus_head.clone(), openai.clone()]
        );
        assert_eq!(refusal_fallback_model("claude-fable-5-1").as_deref(), Some(opus_head.as_str()));
        assert_eq!(refusal_fallback_model("Claude-Fable-5-1[1m]").as_deref(), Some(opus_head.as_str()));
        assert_eq!(refusal_fallback_candidates("claude-fable-5"), vec![opus_head.clone(), openai.clone()]);

        // Opus itself now escapes across providers (its own head cannot stand
        // in for it): OpenAI first, then Google.
        assert_eq!(refusal_fallback_candidates("opus"), vec![openai.clone(), google.clone()]);
        assert_eq!(refusal_fallback_model("claude-opus-5").as_deref(), Some(openai.as_str()));

        // Sonnet and Haiku fall back to Opus first, then across providers.
        assert_eq!(refusal_fallback_candidates("claude-sonnet-5"), vec![opus_head.clone(), openai.clone()]);
        assert_eq!(refusal_fallback_candidates("haiku"), vec![opus_head.clone(), openai.clone()]);

        // Other providers name none — a refusal there is surfaced.
        assert!(refusal_fallback_candidates("gpt-6-astra").is_empty());
        assert!(refusal_fallback_candidates("gemini-3.8-flash").is_empty());
        assert!(refusal_fallback_candidates("no-such-model").is_empty());
        assert_eq!(refusal_fallback_model("gpt-6-astra"), None);
    }

    /// The router's per-role specialty seed is catalog data too, keyed by
    /// the route role key; a role the table does not name seeds nothing.
    #[test]
    fn the_specialty_seed_comes_from_the_catalog() {
        use super::{router_priors, RouterPriors};
        let priors = router_priors();
        assert_eq!(priors.specialty_families("coding"), ["gpt", "claude"]);
        assert_eq!(priors.specialty_families("analysis"), ["claude", "gemini", "deepseek"]);
        assert_eq!(priors.specialty_families("writing"), ["claude", "gemini"]);
        assert!(priors.specialty_families("verifier").is_empty());
        assert!(priors.specialty_families("no-such-role").is_empty());
        // Layering: a document that declares no seed takes the shipped one.
        let mut layered = RouterPriors::default();
        assert!(!layered.declares_anything());
        layered.fill_from(priors);
        assert_eq!(layered.specialty_seed, priors.specialty_seed);
    }

    /// The starvation ladder is catalog data too, so a family's next rung moves
    /// with the catalog instead of with a policy edit.
    #[test]
    fn the_starvation_ladder_comes_from_the_catalog() {
        super::reset_model_registry_for_tests();
        assert_eq!(starvation_demotion_model("claude-opus-5").as_deref(), Some("claude-sonnet-5"));
        assert_eq!(
            starvation_demotion_model("sonnet").as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        // No declared rung means bottom, not unknown.
        assert_eq!(starvation_demotion_model("haiku"), None);
        assert_eq!(starvation_demotion_model("fable"), None);
        // Google resolves its declared rung to the pinned release.
        assert_eq!(
            starvation_demotion_model("gemini-3.1-pro-preview").as_deref(),
            Some("gemini-3.6-flash")
        );
    }

    /// The wire mapping is a catalog fact, not a Gemini special case: any
    /// provider's model can declare the id it is served under, and a model that
    /// declares none is sent under its own id.
    #[test]
    fn a_declared_wire_id_applies_to_any_provider() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(
            super::MODEL_CONTEXT_WINDOWS_ENV,
            Some(
                r#"{"models":[
                    {"provider":"openai","ids":["gpt-5.7"],"wire":"gpt-5.7-2026-08-01"},
                    {"provider":"anthropic","ids":["opus-next"],"wire":{"low":"claude-opus-6-low","high":"claude-opus-6-high"}}
                ]}"#,
            ),
        );

        assert_eq!(
            super::wire_model_for_request("gpt-5.7", crate::types::ReasoningRequest::Auto),
            "gpt-5.7-2026-08-01"
        );
        assert_eq!(
            super::wire_model_for_request(
                "opus-next",
                crate::types::ReasoningRequest::Effort(crate::types::EffortLevel::Max)
            ),
            "claude-opus-6-high"
        );
        // Undeclared stays untouched — the catalog never invents a variant.
        assert_eq!(
            super::wire_model_for_request("gpt-5.6-sol", crate::types::ReasoningRequest::Auto),
            "gpt-5.6-sol"
        );
    }

    /// The three rungs a budget folds into, so a legacy `thinkingBudget` picks
    /// the same served id an equivalent effort would.
    #[test]
    fn a_thinking_budget_folds_into_the_effort_rungs() {
        use super::effort_rung;
        use crate::types::{EffortLevel, ReasoningRequest};
        assert_eq!(effort_rung(ReasoningRequest::BudgetTokens(1_000)), EffortLevel::Low);
        assert_eq!(effort_rung(ReasoningRequest::BudgetTokens(6_000)), EffortLevel::Medium);
        assert_eq!(effort_rung(ReasoningRequest::BudgetTokens(16_000)), EffortLevel::High);
        assert_eq!(effort_rung(ReasoningRequest::Auto), EffortLevel::Low);
    }

    /// A declared context window must beat the name-shape guess. The declared
    /// sources used to sit BELOW the `contains("claude")` ladder, so an operator
    /// running a gateway that serves a Claude-compatible id could not state its
    /// real window, and the catalog's own Anthropic rows were dead data for this
    /// field. Reordering must not move any built-in window, because every
    /// Anthropic row declares exactly what the guess returned.
    #[test]
    fn context_window_declaration_beats_the_name_guess() {
        let _lock = crate::test_env_lock();
        let _clean = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);

        for (model, expected) in [
            ("claude-fable-5", 1_000_000_u64),
            ("claude-opus-5", 1_000_000),
            ("claude-opus-4-8", 1_000_000),
            ("claude-sonnet-5", 1_000_000),
            ("claude-haiku-4-5-20251001", 258_000),
        ] {
            assert_eq!(
                context_window_for_model(model),
                expected,
                "built-in window for {model} must not move"
            );
        }

        // Declared through the same env catalog an operator would use, so this
        // exercises the real path rather than a test-only seam.
        let _declared = EnvVarGuard::set(
            MODEL_CONTEXT_WINDOWS_ENV,
            Some(
                r#"{"models":[{"provider":"custom","ids":["claude-3-opus-proxy"],"context_window":200000}]}"#,
            ),
        );
        assert_eq!(
            context_window_for_model("claude-3-opus-proxy"),
            200_000,
            "a declared window must beat the contains(\"opus\") guess (1M)"
        );
    }

    #[test]
    fn resolves_grok_aliases_only_when_adapter_gate_is_enabled() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        assert_eq!(resolve_model_alias("grok"), "grok");

        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        assert_eq!(resolve_model_alias("grok"), "grok-3");
    }

    /// The `fable5` trap: a near-miss of a real Anthropic alias must snap to the
    /// alias's canonical id, NOT pass through verbatim to Anthropic as a bogus
    /// `404 not_found` model. Anthropic is always provider-enabled, so this
    /// holds regardless of the non-Claude adapter gate.
    #[test]
    fn recovers_near_miss_of_a_builtin_alias() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        // Single fat-finger extra char and a transposition both recover.
        assert_eq!(resolve_model_alias("fable5"), "claude-fable-5-1");
        assert_eq!(resolve_model_alias("fabel"), "claude-fable-5-1");
        assert_eq!(resolve_model_alias("opuss"), "claude-opus-5");
    }

    /// A GPT/Gemini near-miss recovers only when the target provider is enabled,
    /// exactly like an exact-alias hit — so a typo under a Claude-only setup
    /// passes through as "unsupported" rather than silently dialing OpenAI.
    #[test]
    fn near_miss_recovery_honors_the_provider_gate() {
        let _lock = crate::test_env_lock();

        let _off = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        // Gate off: no snap to a disabled provider's alias — passthrough.
        assert_eq!(resolve_model_alias("gemini-flsh"), "gemini-flsh");

        let _on = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        // Gate on: `gemini-flsh` is a unique word-typo of `gemini-flash`.
        assert_eq!(resolve_model_alias("gemini-flsh"), "gemini-3.6-flash");
    }

    #[test]
    fn priority_tier_requires_a_catalog_declared_fast_pair() {
        assert!(openai_fast_tier_enabled("gpt-5.5-fast"));
        assert!(openai_fast_tier_enabled("gpt-5.6-terra[fast]"));
        assert!(!openai_fast_tier_enabled("gpt-5.6-terra-preview-fast"));
        assert!(!openai_fast_tier_enabled("gpt-5.7-nova-fast"));
        assert!(!openai_fast_tier_enabled("claude-opus-fast"));
    }

    #[test]
    fn starvation_demotion_uses_current_catalog_targets() {
        assert_eq!(starvation_demotion_model("opus").as_deref(), Some("claude-sonnet-5"));
        assert_eq!(
            starvation_demotion_model("gpt-5.5-fast").as_deref(),
            Some("gpt-5.6-terra[fast]")
        );
        assert_eq!(
            starvation_demotion_model("gpt-5.6-sol").as_deref(),
            Some("gpt-5.6-luna")
        );
        assert_eq!(starvation_demotion_model("gpt-5.6-luna"), None);
        assert_eq!(
            starvation_demotion_model("gemini-pro").as_deref(),
            Some("gemini-3.6-flash")
        );
        assert_eq!(starvation_demotion_model("gemini-3.6-flash"), None);
        assert_eq!(starvation_demotion_model("custom-flash-proxy"), None);
    }

    #[test]
    fn semantic_latest_aliases_have_catalog_heads() {
        assert_eq!(
            latest_model_for_provider(ProviderKind::Anthropic),
            Some("claude-opus-5")
        );
        assert_eq!(
            latest_model_for_provider(ProviderKind::OpenAi),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            latest_model_for_provider(ProviderKind::Google),
            Some("gemini-3.6-flash")
        );
        assert_eq!(
            latest_model_for_provider(ProviderKind::Xai),
            Some("grok-3")
        );
        assert_eq!(latest_model_for_provider(ProviderKind::Ollama), None);
    }

    /// The critical conservatism boundary: a typo may differ from its alias only
    /// in the WORD, never in an explicit version/qualifier number. Silently
    /// running an older/cheaper sibling is worse than a clean "unsupported"
    /// error, so a version bump or suffix change must pass through untouched
    /// even when the provider is enabled and the edit distance is tiny.
    #[test]
    fn near_miss_recovery_never_reroutes_across_versions() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));

        // Anthropic is always enabled — these are the worst always-on reroutes.
        // `opus-4.6` is a real older Opus that is NOT in the registry: it must
        // pass through, never snap onto the registered `opus` (Opus 5).
        assert_eq!(resolve_model_alias("opus-4.6"), "opus-4.6");
        assert_eq!(resolve_model_alias("sonnet-4"), "sonnet-4");
        // Non-Claude version bumps / suffix changes with the gate ON.
        assert_eq!(resolve_model_alias("grok-4"), "grok-4");
        assert_eq!(resolve_model_alias("gpt-5.5-mini"), "gpt-5.5-mini");
        assert_eq!(resolve_model_alias("gpt-5.6o"), "gpt-5.6o");
        assert_eq!(resolve_model_alias("gemini-3.7-flash"), "gemini-3.7-flash");
    }

    /// The displayed short form of a model resolves to its canonical id. A live
    /// session spawned agents with `model: opus-4.8` (the version spelled with a
    /// dot, exactly as the HUD prints it) and the provider 404'd it verbatim
    /// (`not_found_error: model: opus-4.8`), even though `claude-opus-4-8` is the
    /// same model. This is version-*preserving* reformatting, distinct from the
    /// cross-version snaps `near_miss_recovery_never_reroutes_across_versions`
    /// forbids: only the separator (dot vs hyphen) and the `claude-` prefix
    /// differ, and the result must land on a real canonical id or pass through.
    #[test]
    fn resolves_dotted_and_bare_short_form_to_canonical() {
        let _lock = crate::test_env_lock();
        // Anthropic is always enabled, so these hold regardless of the gate.
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);

        // The reported 404: dotted version short form → canonical.
        assert_eq!(resolve_model_alias("opus-4.8"), "claude-opus-4-8");
        assert_eq!(resolve_model_alias("claude-opus-4.8"), "claude-opus-4-8");
        // Bare (no-minor / hyphen) short forms of other Claude families.
        assert_eq!(resolve_model_alias("fable-5"), "claude-fable-5");
        assert_eq!(resolve_model_alias("sonnet-5"), "claude-sonnet-5");
        assert_eq!(resolve_model_alias("opus-5"), "claude-opus-5");

        // Conservatism preserved: a genuinely different version whose
        // normalization matches no canonical still passes through untouched, and
        // providers whose canonical ids keep dots are unaffected.
        assert_eq!(resolve_model_alias("opus-4.6"), "opus-4.6");
        assert_eq!(resolve_model_alias("gpt-5.5-mini"), "gpt-5.5-mini");
        assert_eq!(resolve_model_alias("gemini-3.7-flash"), "gemini-3.7-flash");
    }

    #[test]
    fn registered_spawn_alias_promotes_unqualified_stale_ids_but_preserves_explicit_pins() {
        let _lock = crate::test_env_lock();
        super::refresh_custom_providers_from_json("[]").expect("clear custom providers");
        for input in [
            "opus-4.6",
            "claude-opus-4.6",
            "claude-opus-4-6",
            "opus-6",
        ] {
            assert_eq!(
                resolve_registered_model_alias(input),
                "claude-opus-5",
                "input {input}"
            );
        }
        assert_eq!(
            resolve_registered_model_alias("opus"),
            "claude-opus-5"
        );
        assert_eq!(
            resolve_registered_model_alias("opus-5"),
            "claude-opus-5"
        );
        // A model-authored unqualified release id may be stale even when it was
        // once registered. Spawn routing promotes it to the family head; an
        // operator who truly wants the historical release uses an explicit
        // provider-qualified pin, which remains byte-for-byte intact.
        assert_eq!(
            resolve_registered_model_alias("opus-4.8"),
            "claude-opus-5"
        );
        assert_eq!(
            resolve_registered_model_alias("claude-opus-4-8"),
            "claude-opus-5"
        );
        assert_eq!(
            resolve_registered_model_alias("anthropic/claude-opus-4-8"),
            "anthropic/claude-opus-4-8"
        );
        assert_eq!(
            resolve_registered_model_alias("opus[1m]"),
            "claude-opus-5"
        );
        assert_eq!(
            resolve_registered_model_alias("opus-4.6[1m]"),
            "claude-opus-5"
        );
        assert_eq!(
            resolve_registered_model_alias("sonnet-4.5"),
            "claude-sonnet-5"
        );
        assert_eq!(
            resolve_registered_model_alias("claude-3-5-haiku"),
            "claude-haiku-4-5-20251001"
        );
        assert_eq!(
            resolve_registered_model_alias("fable-4"),
            "claude-fable-5-1"
        );

        // Current registered canonicals remain exact targets. Unregistered
        // first-party ids emitted from stale model knowledge promote to that
        // provider's semantic latest head; provider-qualified historical pins
        // remain exact.
        assert_eq!(
            resolve_registered_model_alias("gpt-5.6-sol"),
            "gpt-5.6-sol"
        );
        assert_eq!(
            resolve_registered_model_alias("gpt-5.5"),
            "gpt-5.6-sol"
        );
        assert_eq!(
            resolve_registered_model_alias("openai/gpt-5.5"),
            "openai/gpt-5.5"
        );
        assert_eq!(
            resolve_registered_model_alias("gemini-3-flash"),
            "gemini-3.6-flash"
        );
        assert_eq!(
            resolve_registered_model_alias("google/gemini-3-flash"),
            "google/gemini-3-flash"
        );
        assert_eq!(resolve_registered_model_alias("grok-3"), "grok-3");
        assert_eq!(
            resolve_registered_model_alias("openai/gpt-x"),
            "openai/gpt-x"
        );

        super::refresh_custom_providers_from_json(
            r#"[{"name":"custom claude","base_url":"https://example.invalid/v1",
                "models":["claude-opus-4.6"],"requires_auth":false}]"#,
        )
        .expect("load custom provider");
        assert_eq!(
            resolve_registered_model_alias("claude-opus-4.6"),
            "claude-opus-4.6",
            "operator-declared custom ids must not be snapped to Anthropic"
        );
        super::refresh_custom_providers_from_json("[]").expect("clear custom providers");
    }

    /// Conservatism guards: an ambiguous or too-far input must NEVER be
    /// rerouted to a distinct real model — passthrough (then the caller can
    /// surface a clean error) is safer than snapping to the wrong model.
    #[test]
    fn near_miss_recovery_refuses_ambiguous_or_distant_inputs() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));

        // Bare `gpt-5.6`은 한때 "형제로 스냅 금지" passthrough였으나, 사용자
        // 확정(2026-07-11)으로 terra를 가리키는 **명시 레지스트리 별칭**이 됐다
        // — near-miss 퍼지 스냅이 아니라 exact-alias 경로라 보수성 원칙과
        // 충돌하지 않는다. 서빙 티어는 별칭에 박지 않는다(세션 /fast 상태가
        // 결정) — bare id로 해석된다.
        assert_eq!(resolve_model_alias("gpt-5.6"), "gpt-5.6-terra");
        // No shared 3-char prefix with any alias → no snap.
        assert_eq!(resolve_model_alias("op"), "op");
        assert_eq!(resolve_model_alias("xyzzy"), "xyzzy");
        // A distinct real model is NOT snapped onto its sibling.
        assert_eq!(resolve_model_alias("gpt-5.6-sol"), "gpt-5.6-sol");
    }

    /// A fully-qualified canonical id already in the registry must resolve to
    /// itself, never get snapped to a shorter alias by the near-miss pass.
    #[test]
    fn near_miss_recovery_never_snaps_a_known_canonical_id() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        assert_eq!(resolve_model_alias("claude-fable-5"), "claude-fable-5");
        assert_eq!(resolve_model_alias("claude-opus-4-8"), "claude-opus-4-8");
    }

    #[test]
    fn levenshtein_and_prefix_helpers_are_correct() {
        assert_eq!(levenshtein("fable", "fable"), 0);
        assert_eq!(levenshtein("fable5", "fable"), 1);
        assert_eq!(levenshtein("fabel", "fable"), 2);
        assert_eq!(levenshtein("", "opus"), 4);
        // Multibyte input must not panic and counts by char.
        assert_eq!(levenshtein("한글", "opus"), 4);
        assert_eq!(shared_prefix_len("gpt-5.6x", "gpt-5.6-sol"), 7);
        assert_eq!(shared_prefix_len("op", "opus"), 2);
    }

    #[test]
    fn version_tokens_splits_only_field_boundary_numbers() {
        // A digit glued to a letter is a fat-finger, not a version field.
        assert!(version_tokens("fable5").is_empty());
        assert!(version_tokens("fable").is_empty());
        assert!(version_tokens("opus").is_empty());
        // Numbers at a `-`/`.`/start boundary ARE version fields.
        assert_eq!(version_tokens("opus-5"), vec!["5".to_string()]);
        assert_eq!(version_tokens("gpt-5.6-sol"), vec!["5.6".to_string()]);
        assert_eq!(version_tokens("gpt-5.6o"), vec!["5.6o".to_string()]);
        assert_eq!(version_tokens("gpt-5.5-mini"), vec!["5.5".to_string()]);
        assert_eq!(version_tokens("gemini-3.7-flash"), vec!["3.7".to_string()]);
    }

    #[test]
    fn detects_provider_from_model_name_only_when_adapter_gate_is_enabled() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        assert_eq!(detect_provider_kind("grok"), ProviderKind::Anthropic);

        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        assert_eq!(detect_provider_kind("grok"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn catalog_provider_lookup_supports_non_prefix_openai_canonicals() {
        const FUTURE_CATALOG: &[ProviderCatalogEntry] = &[
            ProviderCatalogEntry::new(
                "future-openai",
                "future-openai-2026-01-01",
                ProviderKind::OpenAi,
            ),
            ProviderCatalogEntry::new("future-google", "future-google-1", ProviderKind::Google),
        ];

        assert_eq!(
            catalog_provider_for_canonical(FUTURE_CATALOG, "future-openai-2026-01-01"),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(
            catalog_provider_for_canonical(FUTURE_CATALOG, "FUTURE-OPENAI-2026-01-01"),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(
            catalog_entry_for_token(FUTURE_CATALOG, "future-openai")
                .map(|entry| entry.provider),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(
            catalog_entry_for_token(FUTURE_CATALOG, "future-openai-2026-01-01")
                .map(|entry| entry.provider),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(catalog_provider_for_canonical(FUTURE_CATALOG, "unknown"), None);
    }

    #[test]
    fn openai_catalog_tokens_are_explicit_even_without_a_gpt_prefix() {
        const FUTURE_CATALOG: &[ProviderCatalogEntry] = &[ProviderCatalogEntry::new(
            "future-openai",
            "future-openai-2026-01-01",
            ProviderKind::OpenAi,
        )];
        let entry = catalog_entry_for_token(FUTURE_CATALOG, "future-openai")
            .expect("future OpenAI alias found");

        assert_eq!(entry.provider, ProviderKind::OpenAi);
        assert!(is_openai_builtin_model_prefix("o1-preview"));
        assert_eq!(explicit_non_claude_provider_kind("o1-preview"), Some(ProviderKind::OpenAi));
    }

    #[test]
    fn detects_openai_provider_after_alias_resolution() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        let _key = EnvVarGuard::set("OPENAI_API_KEY", None);
        let _base = EnvVarGuard::set("OPENAI_BASE_URL", Some("http://localhost:8080/v1"));

        // gpt-5.5 계열은 카탈로그 퇴역(2026-07-11) — 별칭 없이 passthrough.
        // 레거시 정확 id의 provider 감지는 아래에서 계속 보장한다.
        assert_eq!(resolve_model_alias("gpt-5.5"), "gpt-5.5");
        assert_eq!(resolve_model_alias("gpt-5.6"), "gpt-5.6-terra");
        assert_eq!(resolve_model_alias("gpt-5.6-luna"), "gpt-5.6-luna");
        assert_eq!(
            resolve_model_alias("gpt-5.3-codex-spark"),
            "gpt-5.3-codex-spark"
        );
        assert_eq!(
            detect_provider_kind("gpt-5.5-2026-04-23"),
            ProviderKind::OpenAi
        );
    }

    #[test]
    fn resolves_current_gemini_code_assist_aliases_when_google_is_enabled() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));

        assert_eq!(resolve_model_alias("gemini-flash"), "gemini-3.6-flash");
        assert_eq!(resolve_model_alias("gemini-pro"), "gemini-3.1-pro-preview");
        assert_eq!(
            resolve_model_alias("gemini-flash-lite"),
            "gemini-3.1-flash-lite"
        );
        assert_eq!(
            resolve_model_alias("gemini-3.5-pro"),
            "gemini-3.1-pro-preview"
        );
        assert_eq!(
            detect_provider_kind("gemini-3.1-pro-preview"),
            ProviderKind::Google
        );
    }

    #[test]
    fn adapter_gate_parses_common_falsey_values() {
        let _lock = crate::test_env_lock();
        let _legacy_unset = EnvVarGuard::set(EXPERIMENTAL_PROVIDERS_ENV, None);
        let _unset = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        assert!(!non_claude_adapters_enabled());

        let _legacy_false = EnvVarGuard::set(EXPERIMENTAL_PROVIDERS_ENV, Some("false"));
        let _false = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("false"));
        assert!(!non_claude_adapters_enabled());

        let _legacy_true = EnvVarGuard::set(EXPERIMENTAL_PROVIDERS_ENV, Some("1"));
        let _true = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        assert!(non_claude_adapters_enabled());
    }

    #[test]
    fn custom_base_url_enables_adapters_like_an_api_key() {
        let _lock = crate::test_env_lock();
        let _f1 = EnvVarGuard::set(EXPERIMENTAL_PROVIDERS_ENV, None);
        let _f2 = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        // Clear every provider credential/endpoint so the default is clean.
        let _k1 = EnvVarGuard::set("OPENAI_API_KEY", None);
        let _k2 = EnvVarGuard::set("GOOGLE_API_KEY", None);
        let _k3 = EnvVarGuard::set("XAI_API_KEY", None);
        let _k4 = EnvVarGuard::set("OLLAMA_API_KEY", None);
        let _b2 = EnvVarGuard::set("GOOGLE_BASE_URL", None);
        let _b3 = EnvVarGuard::set("XAI_BASE_URL", None);
        let _b4 = EnvVarGuard::set("OLLAMA_BASE_URL", None);

        // Nothing configured → Anthropic-only.
        let base_off = EnvVarGuard::set("OPENAI_BASE_URL", None);
        assert!(!non_claude_adapters_enabled());
        drop(base_off);

        // A self-hosted OpenAI-compatible endpoint (base URL, no key) now
        // auto-enables the adapters — matching how OLLAMA_BASE_URL already did.
        let _base_on = EnvVarGuard::set("OPENAI_BASE_URL", Some("http://localhost:8080/v1"));
        assert!(non_claude_adapters_enabled());
    }

    #[test]
    fn explicit_non_claude_provider_detection_works_without_gate() {
        assert_eq!(
            explicit_non_claude_provider_kind("grok-3"),
            Some(ProviderKind::Xai)
        );
        assert_eq!(
            explicit_non_claude_provider_kind("gpt-5.6-sol"),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(explicit_non_claude_provider_kind("claude-sonnet-4-6"), None);
    }

    #[test]
    fn max_tokens_follow_docs_for_opus_fable_and_safe_fallbacks() {
        // Current Anthropic docs list 128K max output for Fable/Mythos and
        // Opus 4.6+ (including the `opus` alias -> Opus 4.8). Deprecated Opus
        // 4.1/4.0 variants stay at their documented 32K. Non-Anthropic models
        // now honour the docs-verified catalog cap; a model with no catalog entry
        // (or no max_output_tokens) keeps the safe 64K default.
        assert_eq!(max_tokens_for_model("opus"), 128_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-8"), 128_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-8-20260611"), 128_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-7"), 128_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-6[1m]"), 128_000);
        assert_eq!(max_tokens_for_model("fable"), 128_000);
        assert_eq!(max_tokens_for_model("sonnet"), 128_000);
        assert_eq!(max_tokens_for_model("claude-sonnet-5"), 128_000);
        assert_eq!(max_tokens_for_model("claude-sonnet-5-20260625"), 128_000);
        assert_eq!(max_tokens_for_model("claude-mythos-5"), 128_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-1"), 32_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-20250514"), 32_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-5"), 64_000);
        assert_eq!(max_tokens_for_model("claude-opus-4-10"), 64_000);
        assert_eq!(max_tokens_for_model("claude-sonnet-4-6"), 64_000);
        assert_eq!(max_tokens_for_model("grok-3"), 64_000);
        // Non-Anthropic models now honour the docs-verified catalog
        // max_output_tokens (previously all silently 64k because the field was
        // never deserialized).
        assert_eq!(max_tokens_for_model("gpt-5.6-sol"), 128_000);
        assert_eq!(max_tokens_for_model("gpt-5.6-sol-2026-07-09"), 128_000);
        assert_eq!(max_tokens_for_model("gpt-5.6-terra"), 128_000);
        assert_eq!(max_tokens_for_model("gpt-5.6-terra@openai"), 128_000);
        assert_eq!(max_tokens_for_model("gpt-5.6-luna"), 128_000);
        assert_eq!(max_tokens_for_model("gpt-5.6-luna[fast]"), 128_000);
        assert_eq!(max_tokens_for_model("gpt-5.5"), 128_000);
        assert_eq!(max_tokens_for_model("gpt-5.5-fast"), 128_000);
        assert_eq!(max_tokens_for_model("deepseek-v4-pro"), 384_000);
        // A catalog entry without max_output_tokens keeps the safe default.
        assert_eq!(max_tokens_for_model("gpt-5.3-codex-spark"), 64_000);
        assert_eq!(max_tokens_for_model("gpt-6-preview"), 64_000);
    }

    #[test]
    fn effort_floor_only_raises_never_lowers() {
        // No floor → configured budget passes through unchanged.
        assert_eq!(effort_budget_with_floor(Some(10_000), None), Some(10_000));
        assert_eq!(effort_budget_with_floor(None, None), None);
        // Floor below configured → keep the higher configured (never lower).
        assert_eq!(
            effort_budget_with_floor(Some(24_000), Some(16_000)),
            Some(24_000)
        );
        // Floor above configured → raise to the floor.
        assert_eq!(
            effort_budget_with_floor(Some(0), Some(16_000)),
            Some(16_000)
        );
        assert_eq!(
            effort_budget_with_floor(Some(10_000), Some(16_000)),
            Some(16_000)
        );
        // No configured thinking, floor set → escalate from nothing to the floor.
        assert_eq!(effort_budget_with_floor(None, Some(16_000)), Some(16_000));
        // A zero floor is inert.
        assert_eq!(
            effort_budget_with_floor(Some(10_000), Some(0)),
            Some(10_000)
        );
        assert_eq!(effort_budget_with_floor(None, Some(0)), None);
    }

    #[test]
    fn adaptive_thinking_gates_on_model_generation() {
        // Opus 4.6+/Fable use adaptive (output_config.effort); 4.5 and earlier
        // keep legacy budget thinking.
        assert!(uses_adaptive_thinking("claude-opus-4-8"));
        assert!(uses_adaptive_thinking("claude-opus-4-6"));
        assert!(uses_adaptive_thinking("fable"));
        assert!(uses_adaptive_thinking("claude-fable-5"));
        assert!(uses_adaptive_thinking("sonnet"));
        assert!(uses_adaptive_thinking("claude-sonnet-5"));
        assert!(uses_adaptive_thinking("claude-sonnet-4-6"));
        assert!(!uses_adaptive_thinking("claude-opus-4-5"));
        assert!(!uses_adaptive_thinking("claude-sonnet-4-5"));
        assert!(!uses_adaptive_thinking("claude-3-5-sonnet"));
        // Non-Anthropic models never use Anthropic adaptive thinking.
        assert!(!uses_adaptive_thinking("gpt-5.5"));
        assert!(!uses_adaptive_thinking("grok-3"));
    }

    #[test]
    fn budget_maps_to_effort_levels_monotonically() {
        use crate::types::EffortLevel;
        // Thresholds invert the CLI Effort preset budgets (1024/4096/10000/
        // 16000/24000/32000); each preset maps back to its own level, and the
        // boundaries are the midpoints between consecutive presets.
        assert_eq!(effort_level_for_budget(0), EffortLevel::Low);
        assert_eq!(effort_level_for_budget(1_024), EffortLevel::Low); // Low preset
        assert_eq!(effort_level_for_budget(2_560), EffortLevel::Low);
        assert_eq!(effort_level_for_budget(2_561), EffortLevel::Medium);
        assert_eq!(effort_level_for_budget(4_096), EffortLevel::Medium); // Medium preset
        assert_eq!(effort_level_for_budget(7_048), EffortLevel::Medium);
        assert_eq!(effort_level_for_budget(7_049), EffortLevel::High);
        assert_eq!(effort_level_for_budget(10_000), EffortLevel::High); // High preset
        assert_eq!(effort_level_for_budget(13_000), EffortLevel::High);
        assert_eq!(effort_level_for_budget(13_001), EffortLevel::Xhigh);
        assert_eq!(effort_level_for_budget(16_000), EffortLevel::Xhigh); // Xhigh preset
        assert_eq!(effort_level_for_budget(20_000), EffortLevel::Xhigh);
        assert_eq!(effort_level_for_budget(20_001), EffortLevel::Max);
        assert_eq!(effort_level_for_budget(24_000), EffortLevel::Max); // Max preset
        assert_eq!(effort_level_for_budget(28_000), EffortLevel::Max); // Smart preset (formerly Ultracode)
        assert_eq!(effort_level_for_budget(32_000), EffortLevel::Max);
        assert_eq!(effort_level_for_budget(200_000), EffortLevel::Max);
    }

    #[test]
    fn effective_effort_for_model_projects_every_provider_ceiling() {
        use crate::types::EffortLevel::{High, Low, Max, Medium, Ultra, Xhigh};
        // (model, requested, effective). Covers the full low..max scale across
        // every provider family so a UI can read the *actual* tier without
        // branching on provider.
        let cases = [
            // Anthropic: Opus/Fable/Sonnet 5 keep the full scale; Sonnet 4.6
            // and Haiku clamp xhigh -> high but KEEP max (max is in their
            // supported set).
            ("opus", Xhigh, Xhigh),
            ("opus", Max, Max),
            ("claude-opus-4-8", Xhigh, Xhigh),
            ("claude-fable-5", Xhigh, Xhigh),
            ("sonnet", Xhigh, Xhigh),
            ("sonnet", Max, Max),
            ("claude-sonnet-5", Xhigh, Xhigh),
            ("claude-sonnet-4-6", Xhigh, High),
            ("haiku", Xhigh, High),
            ("claude-haiku-4-5-20251001", Xhigh, High),
            // OpenAI: GPT-5.6 keeps Max; older GPT families keep the historical
            // Max -> Xhigh clamp. `/fast` is service priority, not an effort
            // ceiling.
            ("gpt-5.6-sol", Xhigh, Xhigh),
            ("gpt-5.6-sol", Max, Max),
            ("gpt-5.6-terra", Max, Max),
            ("gpt-5.6-luna", Max, Max),
            ("gpt-5.5", Xhigh, Xhigh),
            ("gpt-5.5", Max, Xhigh),
            ("gpt-5.5-fast", Xhigh, Xhigh),
            ("gpt-5.5-fast", Max, Xhigh),
            ("gpt-5.5-2026-04-23-fast", Max, Xhigh),
            ("gpt-5.3-codex-spark", Xhigh, Xhigh),
            ("gpt-5.3-codex-spark", Max, Xhigh),
            ("gpt-5.6-sol", Ultra, Ultra),
            ("gpt-5.6-sol-2026-07-09", Ultra, Ultra),
            ("gpt-5.6-terra@openai", Ultra, Ultra),
            ("gpt-5.6-terra[fast]", Ultra, Ultra),
            ("gpt-5.6-luna", Max, Max),
            // Luna has no internal Ultra rung, so Ultra falls to Luna's own
            // ceiling (Max) — the same value `gpt_for_model` puts on the wire.
            ("gpt-5.6-luna", Ultra, Max),
            ("gpt-5.5", Ultra, Xhigh),
            // Google Gemini 3: tops out at high.
            ("gemini-3.5-flash", Xhigh, High),
            ("gemini-3.5-flash", Max, High),
            ("gemini-3.5-flash", Ultra, High),
            ("gemini-pro", Max, High),
            ("gemini-pro", High, High),
            // Custom / unknown / xAI: conservative pass-through (never silently
            // downgraded off an ambient OPENAI_API_KEY — the BUG-R14 trap).
            ("deepseek-chat", Xhigh, Xhigh),
            ("deepseek-chat", Max, Max),
            ("my-self-hosted-model", Xhigh, Xhigh),
            // Lower tiers are identity on every provider.
            ("sonnet", Low, Low),
            ("gpt-5.5-fast", Medium, Medium),
            ("gemini-3.5-flash", High, High),
        ];
        for (model, requested, effective) in cases {
            assert_eq!(
                effective_effort_for_model(requested, model),
                effective,
                "model={model} requested={requested:?}"
            );
        }
    }

    #[test]
    fn model_supports_xhigh_truth_table() {
        // Mirrors the per-provider ceilings: Opus/Fable/Sonnet 5 and GPT
        // (including fast) accept xhigh; Sonnet 4.6, Haiku, and all Gemini do
        // not; unknown/custom pass through as "supported" (we don't claim to
        // know their ceiling).
        assert!(model_supports_xhigh("opus"));
        assert!(model_supports_xhigh("claude-fable-5"));
        assert!(model_supports_xhigh("gpt-5.5"));
        assert!(model_supports_xhigh("claude-sonnet-5"));
        assert!(!model_supports_xhigh("claude-sonnet-4-6"));
        assert!(!model_supports_xhigh("claude-haiku-4-5-20251001"));
        assert!(model_supports_xhigh("gpt-5.5-fast"));
        assert!(model_supports_xhigh("gpt-5.3-codex-spark"));
        assert!(!model_supports_xhigh("gemini-3.5-flash"));
        assert!(!model_supports_xhigh("gemini-pro"));
        assert!(model_supports_xhigh("deepseek-chat"));
    }

    #[test]
    fn max_supported_effort_matches_every_provider_ceiling() {
        use crate::types::EffortLevel::{High, Max, Ultra, Xhigh};
        let cases = [
            // Anthropic: every model's true ceiling is Max regardless of the
            // xhigh split (Sonnet/Haiku accept max, just not xhigh).
            ("opus", Max),
            ("claude-opus-4-8", Max),
            ("claude-fable-5", Max),
            ("sonnet", Max),
            ("claude-sonnet-5", Max),
            ("haiku", Max),
            ("claude-haiku-4-5-20251001", Max),
            // OpenAI internal selection: Sol/Terra reach Ultra; Luna tops out
            // at Max; every other GPT family tops out at Xhigh.
            ("gpt-5.6-sol", Ultra),
            ("gpt-5.6-terra", Ultra),
            ("gpt-5.6-luna", Max),
            ("gpt-5.5", Xhigh),
            ("gpt-5.5-fast", Xhigh),
            ("gpt-5.3-codex-spark", Xhigh),
            // Dated / explicit-provider / service-tier suffixed ids inherit
            // their bare family's ceiling.
            ("gpt-5.6-sol-2026-07-09", Ultra),
            ("gpt-5.6-terra@openai", Ultra),
            ("gpt-5.6-terra[fast]", Ultra),
            ("gpt-5.6-luna-2026-07-09", Max),
            // Google Gemini 3 tops out at High.
            ("gemini-3.5-flash", High),
            ("gemini-pro", High),
            // Custom / unknown / xAI: conservative default — never Ultra/Max on
            // the strength of ambient pass-through tolerance.
            ("deepseek-chat", High),
            ("my-self-hosted-model", High),
        ];
        for (model, expected) in cases {
            assert_eq!(max_supported_effort(model), expected, "model={model}");
        }
    }

    #[test]
    fn max_supported_effort_env_override_wins_and_prefers_longest_prefix() {
        let _lock = crate::test_env_lock();
        let _override = EnvVarGuard::set(
            MODEL_EFFORT_CEILINGS_ENV,
            Some(r#"{"gpt-5.7": "max", "gpt-5.7-nova": "ultra"}"#),
        );
        // Longest matching prefix wins: the specific `gpt-5.7-nova` override
        // beats the broader `gpt-5.7` one for a nova id.
        assert_eq!(max_supported_effort("gpt-5.7-nova"), crate::types::EffortLevel::Ultra);
        // A sibling family under the broader prefix gets the broader override.
        assert_eq!(max_supported_effort("gpt-5.7-other"), crate::types::EffortLevel::Max);
        // A model the override does not name is untouched by it.
        assert_eq!(max_supported_effort("gpt-5.5"), crate::types::EffortLevel::Xhigh);
    }

    #[test]
    fn max_supported_effort_env_override_ignores_garbage() {
        let _lock = crate::test_env_lock();
        let _override = EnvVarGuard::set(MODEL_EFFORT_CEILINGS_ENV, Some("not-json"));
        assert_eq!(max_supported_effort("gpt-5.6-sol"), crate::types::EffortLevel::Ultra);

        let _override = EnvVarGuard::set(MODEL_EFFORT_CEILINGS_ENV, Some(""));
        assert_eq!(max_supported_effort("gpt-5.6-sol"), crate::types::EffortLevel::Ultra);
    }

    fn request_with_text(model: &str, text: &str) -> crate::types::MessageRequest {
        crate::types::MessageRequest {
            model: model.to_string(),
            max_tokens: 4_096,
            messages: vec![crate::types::InputMessage::user_text(text)],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        }
    }

    #[test]
    fn band_difficulty_for_request_fires_one_signal_per_trigger() {
        // heavy-intent keyword (EN + KO), plain trivial, and a long ask each
        // fire exactly the one signal they should — the shared classifier the
        // effort-band resolver keys off.
        let trivial = band_difficulty_for_request(&request_with_text("gpt-5.5", "hi"));
        assert_eq!(trivial, BandDifficulty::default());
        assert_eq!(trivial.signal_count(), 0);

        let heavy_en = band_difficulty_for_request(&request_with_text(
            "gpt-5.5",
            "please refactor this module",
        ));
        assert!(heavy_en.heavy_intent);
        assert!(!heavy_en.large_context);
        assert!(!heavy_en.long_ask);
        assert_eq!(heavy_en.signal_count(), 1);

        let heavy_ko =
            band_difficulty_for_request(&request_with_text("gpt-5.5", "이 코드베이스를 분석해줘"));
        assert!(heavy_ko.heavy_intent);
        assert_eq!(heavy_ko.signal_count(), 1);

        let long_ask = "word ".repeat(150); // > 600 chars, no heavy keyword
        let long_only = band_difficulty_for_request(&request_with_text("gpt-5.5", &long_ask));
        assert!(!long_only.heavy_intent);
        assert!(long_only.long_ask);
        assert_eq!(long_only.signal_count(), 1);

        // Heavy keyword AND a long ask stack to two signals.
        let heavy_and_long = format!("please refactor this module. {long_ask}");
        let two_signal =
            band_difficulty_for_request(&request_with_text("gpt-5.5", &heavy_and_long));
        assert!(two_signal.heavy_intent && two_signal.long_ask);
        assert_eq!(two_signal.signal_count(), 2);
    }

    #[test]
    fn resolve_effort_band_examples_sol_fable_luna() {
        use crate::types::EffortLevel::{Max, Ultra, Xhigh};

        let none = BandDifficulty::default();
        let one = BandDifficulty {
            heavy_intent: true,
            ..BandDifficulty::default()
        };
        let two = BandDifficulty {
            heavy_intent: true,
            long_ask: true,
            ..BandDifficulty::default()
        };

        // Sol: internal ceiling is Ultra — the full 3-rung selection band is
        // reachable before the OpenAI wire projection.
        assert_eq!(resolve_effort_band(Xhigh, Ultra, "gpt-5.6-sol", none), Xhigh);
        assert_eq!(resolve_effort_band(Xhigh, Ultra, "gpt-5.6-sol", one), Max);
        assert_eq!(resolve_effort_band(Xhigh, Ultra, "gpt-5.6-sol", two), Ultra);

        // Fable: Anthropic's ceiling is Max (Ultra has no Anthropic wire value),
        // so the one-signal and two-signal rungs both land on Max — there is no
        // higher named level to escalate to.
        assert_eq!(
            resolve_effort_band(Xhigh, Ultra, "claude-fable-5", none),
            Xhigh
        );
        assert_eq!(resolve_effort_band(Xhigh, Ultra, "claude-fable-5", one), Max);
        assert_eq!(resolve_effort_band(Xhigh, Ultra, "claude-fable-5", two), Max);

        // Luna's internal selection ceiling is Max — the same [xhigh..max]
        // shape as Fable before provider-specific wire projection.
        assert_eq!(
            resolve_effort_band(Xhigh, Ultra, "gpt-5.6-luna", none),
            Xhigh
        );
        assert_eq!(resolve_effort_band(Xhigh, Ultra, "gpt-5.6-luna", one), Max);
        assert_eq!(resolve_effort_band(Xhigh, Ultra, "gpt-5.6-luna", two), Max);
    }

    #[test]
    fn resolve_effort_band_gemini_collapses_to_its_own_ceiling() {
        use crate::types::EffortLevel::{High, Ultra, Xhigh};
        // Gemini's declared ceiling is High regardless of signal count, so the
        // band degenerates to a single rung — matching the pre-band static
        // projection (`EffortLevel::gemini()` already collapses everything
        // >= high).
        let two = BandDifficulty {
            heavy_intent: true,
            long_ask: true,
            ..BandDifficulty::default()
        };
        assert_eq!(
            resolve_effort_band(Xhigh, Ultra, "gemini-3.5-flash", BandDifficulty::default()),
            High
        );
        assert_eq!(
            resolve_effort_band(Xhigh, Ultra, "gemini-3.5-flash", two),
            High
        );
    }

    #[test]
    fn resolve_effort_band_keeps_the_band_for_an_unknown_model_ceiling() {
        use crate::types::EffortLevel::{Max, Ultra, Xhigh};
        let _lock = crate::test_env_lock();

        // A custom-provider / unrecognized model has an UNKNOWN ceiling, not a
        // `high` one. Clamping it against `max_supported_effort`'s conservative
        // `High` stand-in used to squash floor and ceiling onto the same rung,
        // so `/effort smart` resolved to `high` on every request no matter how
        // many difficulty signals fired — the band was dead on arrival for
        // every gateway-served model.
        for model in ["kimi-k3", "minimax-m2", "deepseek-chat", "default"] {
            assert_eq!(known_effort_ceiling(model), None, "model={model}");
            assert_eq!(
                resolve_effort_band(Xhigh, Ultra, model, BandDifficulty::default()),
                Xhigh,
                "no signals must sit at the floor: model={model}"
            );
            assert_eq!(
                resolve_effort_band(
                    Xhigh,
                    Ultra,
                    model,
                    BandDifficulty {
                        heavy_intent: true,
                        ..BandDifficulty::default()
                    }
                ),
                Max,
                "one signal must step one rung: model={model}"
            );
            assert_eq!(
                resolve_effort_band(
                    Xhigh,
                    Ultra,
                    model,
                    BandDifficulty {
                        heavy_intent: true,
                        large_context: true,
                        long_ask: true,
                    }
                ),
                Ultra,
                "saturated signals must reach the ceiling: model={model}"
            );
        }

        // A model whose ceiling IS known still clamps exactly as before.
        assert_eq!(
            resolve_effort_band(Xhigh, Ultra, "gemini-3.5-flash", BandDifficulty::default()),
            crate::types::EffortLevel::High
        );
    }

    #[test]
    fn max_supported_effort_still_reports_the_conservative_unknown_floor() {
        use crate::types::EffortLevel;

        // The Deep-tier promotion path (`model_inventory::effort_ceiling_for_model`)
        // reads this and must keep withholding a capability GRANT from a model
        // that never declared one — splitting `known_effort_ceiling` out must not
        // change what this answers.
        let _lock = crate::test_env_lock();
        assert_eq!(max_supported_effort("kimi-k3"), EffortLevel::High);
        assert_eq!(max_supported_effort("deepseek-chat"), EffortLevel::High);
        assert_eq!(max_supported_effort("gpt-5.6-sol"), EffortLevel::Ultra);
        assert_eq!(max_supported_effort("claude-opus-4-8"), EffortLevel::Max);
        assert_eq!(max_supported_effort("gemini-3.5-flash"), EffortLevel::High);
    }

    #[test]
    fn resolve_effort_band_kill_switch_returns_static_ceiling() {
        use crate::types::EffortLevel::{Max, Ultra, Xhigh};
        let _lock = crate::test_env_lock();

        {
            let _guard = EnvVarGuard::set(ULTRA_BAND_ENV, Some("off"));
            // With the band disabled, every call collapses to the clamped
            // ceiling regardless of difficulty — the legacy static-top pin.
            assert_eq!(
                resolve_effort_band(Xhigh, Ultra, "gpt-5.6-sol", BandDifficulty::default()),
                Ultra
            );
            assert_eq!(
                resolve_effort_band(
                    Xhigh,
                    Ultra,
                    "claude-fable-5",
                    BandDifficulty {
                        heavy_intent: true,
                        ..BandDifficulty::default()
                    }
                ),
                Max
            );
        }
        {
            // Case-insensitive matching still gates on the exact "off" value.
            let _guard = EnvVarGuard::set(ULTRA_BAND_ENV, Some("OFF"));
            assert_eq!(
                resolve_effort_band(Xhigh, Ultra, "gpt-5.6-sol", BandDifficulty::default()),
                Ultra
            );
        }

        // Guard dropped (env restored/unset) — the band is active again.
        assert_eq!(
            resolve_effort_band(Xhigh, Ultra, "gpt-5.6-sol", BandDifficulty::default()),
            Xhigh
        );
    }

    #[test]
    fn resolve_effort_band_never_exceeds_ceiling_or_falls_below_floor() {
        // Exhaustive sweep: every (floor, ceiling, model, signal-count) combo
        // must land within [floor ..= min(ceiling, model_ceiling)] and must be
        // deterministic (same inputs -> same output across repeated calls).
        use crate::types::EffortLevel::{High, Low, Max, Medium, Ultra, Xhigh};

        // resolve_effort_band reads ULTRA_BAND_ENV, so this read side must
        // hold the crate env lock too — a sibling test's EnvVarGuard flipping
        // the kill switch between the paired calls is exactly the observed
        // workspace-parallel flake.
        let _lock = crate::test_env_lock();

        let levels = [Low, Medium, High, Xhigh, Max, Ultra];
        let models = [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "claude-fable-5",
            "sonnet",
            "gemini-3.5-flash",
            "deepseek-chat",
        ];
        let difficulties = [
            BandDifficulty::default(),
            BandDifficulty {
                heavy_intent: true,
                ..BandDifficulty::default()
            },
            BandDifficulty {
                large_context: true,
                ..BandDifficulty::default()
            },
            BandDifficulty {
                heavy_intent: true,
                large_context: true,
                long_ask: true,
            },
        ];

        for &floor in &levels {
            for &ceiling in &levels {
                for &model in &models {
                    // Only a KNOWN model ceiling clamps the band; an unknown one
                    // (`deepseek-chat` here) leaves the requested ceiling as-is.
                    let effective_ceiling_rank = match known_effort_ceiling(model) {
                        Some(model_ceiling) => {
                            effort_rank(ceiling).min(effort_rank(model_ceiling))
                        }
                        None => effort_rank(ceiling),
                    };
                    let floor_rank = effort_rank(floor).min(effective_ceiling_rank);
                    for &difficulty in &difficulties {
                        let first = resolve_effort_band(floor, ceiling, model, difficulty);
                        let second = resolve_effort_band(floor, ceiling, model, difficulty);
                        assert_eq!(
                            first, second,
                            "non-deterministic for floor={floor:?} ceiling={ceiling:?} model={model} difficulty={difficulty:?}"
                        );
                        assert!(
                            effort_rank(first) <= effective_ceiling_rank,
                            "exceeded ceiling: floor={floor:?} ceiling={ceiling:?} model={model} difficulty={difficulty:?} got={first:?}"
                        );
                        assert!(
                            effort_rank(first) >= floor_rank,
                            "fell below floor: floor={floor:?} ceiling={ceiling:?} model={model} difficulty={difficulty:?} got={first:?}"
                        );
                    }
                }
            }
        }
    }

    /// The router's name-token tables are catalog data: the shipped section
    /// carries them, and a published override that fills a field wins that
    /// field while the rest keep the shipped words.
    #[test]
    fn router_priors_are_catalog_data_and_layer_per_field() {
        let _lock = crate::test_env_lock();
        super::reset_model_registry_for_tests();
        let shipped = super::router_priors().clone();
        assert!(shipped.small_tokens.iter().any(|token| token == "haiku"));
        assert!(shipped.deep_flagship_tokens.iter().any(|token| token == "opus"));
        assert!(shipped.frontier_family_tokens.iter().any(|token| token == "claude"));
        assert_eq!(shipped.small_parameter_billion_below, 12);

        super::refresh_model_registry_from_json(
            r#"{"models":[],"aliases":[],"priors":{"small_tokens":["tiny"]}}"#,
        );
        let layered = super::router_priors();
        assert_eq!(layered.small_tokens, ["tiny"], "the override fills the field it names");
        assert_eq!(
            layered.deep_flagship_tokens, shipped.deep_flagship_tokens,
            "every other field keeps the shipped words"
        );
        assert_eq!(layered.small_parameter_billion_below, 12);

        // A document that declares no priors leaves the shipped section in force.
        super::refresh_model_registry_from_json(r#"{"models":[],"aliases":[]}"#);
        assert_eq!(super::router_priors(), &shipped);
        super::reset_model_registry_for_tests();
    }

    #[test]
    fn declared_model_class_matches_catalog_declarations() {
        // `declared_model_class` consults ZO_MODEL_CLASSES. Serialize with
        // the override tests below so their process-global env cannot leak
        // into this catalog-only assertion under the parallel test runner.
        let _lock = crate::test_env_lock();
        let cases = [
            // Codex model cache 2026-07-09: sol=frontier(1), terra=balanced(2), luna=fast(3).
            ("gpt-5.6-sol", Some(ModelClass::Frontier)),
            ("gpt-5.6-terra", Some(ModelClass::Balanced)),
            ("gpt-5.6-luna", Some(ModelClass::Fast)),
            // Dated/service-tier-suffixed ids resolve through the same matcher.
            ("gpt-5.6-sol-2026-07-09", Some(ModelClass::Frontier)),
            ("gpt-5.6-terra@openai", Some(ModelClass::Balanced)),
            ("gpt-5.6-luna[fast]", Some(ModelClass::Fast)),
            // gpt-5.5 superseded as frontier by sol; declared balanced.
            ("gpt-5.5", Some(ModelClass::Balanced)),
            ("gpt-5.5-fast", Some(ModelClass::Balanced)),
            ("gpt-5.5-2026-04-23", Some(ModelClass::Balanced)),
            // Anthropic public positioning: fable-5 above opus; opus/sonnet/haiku
            // are executors per the user-declared hierarchy.
            ("claude-fable-5", Some(ModelClass::Frontier)),
            ("fable", Some(ModelClass::Frontier)),
            ("claude-opus-4-8", Some(ModelClass::Balanced)),
            ("opus", Some(ModelClass::Balanced)),
            ("claude-sonnet-5", Some(ModelClass::Balanced)),
            ("sonnet", Some(ModelClass::Balanced)),
            ("claude-haiku-4-5-20251001", Some(ModelClass::Fast)),
            ("haiku", Some(ModelClass::Fast)),
            // Never declared for these providers — undeclared falls back to the
            // runtime's own capability-derived derivation.
            ("gpt-5.3-codex-spark", None),
            ("deepseek-chat", None),
            ("gemini-3.5-flash", None),
            ("grok-4", None),
            ("my-self-hosted-model", None),
        ];
        for (model, expected) in cases {
            assert_eq!(declared_model_class(model), expected, "model={model}");
        }
    }

    #[test]
    fn declared_model_class_env_override_wins_and_prefers_longest_prefix() {
        let _lock = crate::test_env_lock();
        let _override = EnvVarGuard::set(
            MODEL_CLASSES_ENV,
            Some(r#"{"gpt-5.7": "balanced", "gpt-5.7-nova": "frontier"}"#),
        );
        // Longest matching prefix wins: the specific `gpt-5.7-nova` override
        // beats the broader `gpt-5.7` one for a nova id.
        assert_eq!(declared_model_class("gpt-5.7-nova"), Some(ModelClass::Frontier));
        // A sibling family under the broader prefix gets the broader override.
        assert_eq!(declared_model_class("gpt-5.7-other"), Some(ModelClass::Balanced));
        // The override wins over an existing catalog declaration too.
        let _override2 = EnvVarGuard::set(MODEL_CLASSES_ENV, Some(r#"{"gpt-5.6-sol": "fast"}"#));
        assert_eq!(declared_model_class("gpt-5.6-sol"), Some(ModelClass::Fast));
        // A model the override does not name falls through to the catalog.
        assert_eq!(declared_model_class("gpt-5.6-terra"), Some(ModelClass::Balanced));
    }

    #[test]
    fn declared_model_class_env_override_ignores_garbage() {
        let _lock = crate::test_env_lock();
        let _override = EnvVarGuard::set(MODEL_CLASSES_ENV, Some("not-json"));
        assert_eq!(declared_model_class("gpt-5.6-sol"), Some(ModelClass::Frontier));

        let _override = EnvVarGuard::set(MODEL_CLASSES_ENV, Some(""));
        assert_eq!(declared_model_class("gpt-5.6-sol"), Some(ModelClass::Frontier));
    }

    #[test]
    fn effective_effort_matches_the_wire_clamp_for_each_provider() {
        use crate::types::EffortLevel;
        // The tripwire: applying the provider's final wire serializer to the
        // effective internal tier must equal serializing the requested tier
        // directly. If capability selection and wire projection drift apart,
        // this fails loudly.
        let reps = [
            "claude-opus-4-8",
            "claude-sonnet-5",
            "claude-haiku-4-5-20251001",
            "claude-fable-5",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.5-fast",
            "gpt-5.3-codex-spark",
            "gemini-3.5-flash",
        ];
        for model in reps {
            let lower = resolve_model_alias(model).to_ascii_lowercase();
            for level in [
                EffortLevel::Low,
                EffortLevel::Medium,
                EffortLevel::High,
                EffortLevel::Xhigh,
                EffortLevel::Max,
                EffortLevel::Ultra,
            ] {
                let effective = effective_effort_for_model(level, model);
                let wire = if lower.starts_with("claude") {
                    level.anthropic_for_model(&lower).anthropic()
                } else if lower.starts_with("gpt") {
                    level.gpt_for_model(&lower)
                } else {
                    level.gemini()
                };
                let projected_wire = if lower.starts_with("claude") {
                    effective.anthropic()
                } else if lower.starts_with("gpt") {
                    effective.gpt_for_model(&lower)
                } else {
                    effective.gemini()
                };
                assert_eq!(
                    projected_wire,
                    wire,
                    "projection drifted from wire: model={model} level={level:?} \
                     projection={effective:?} wire={wire}"
                );
            }
        }
    }

    #[test]
    fn effort_level_wire_strings_clamp_for_gpt() {
        use crate::types::EffortLevel;
        // Anthropic accepts the full scale.
        assert_eq!(EffortLevel::Max.anthropic(), "max");
        assert_eq!(EffortLevel::Xhigh.anthropic(), "xhigh");
        assert_eq!(EffortLevel::Low.anthropic(), "low");
        // Model-blind `gpt()` stays conservative: it cannot know whether the id
        // predates the `max` rung, and guessing wrong is a 400.
        assert_eq!(EffortLevel::Max.gpt(), "xhigh");
        assert_eq!(EffortLevel::Xhigh.gpt(), "xhigh");
        assert_eq!(EffortLevel::High.gpt(), "high");
        // Told the model, the projection sends what that model actually takes:
        // GPT-5.6 accepts `max`; GPT-5.5 predates it and still caps at `xhigh`.
        assert_eq!(EffortLevel::Max.gpt_for_model("gpt-5.6-sol"), "max");
        assert_eq!(EffortLevel::Ultra.gpt_for_model("gpt-5.6-sol"), "max");
        assert_eq!(EffortLevel::Max.gpt_for_model("gpt-5.5"), "xhigh");
    }

    #[test]
    fn opus_and_fable_are_the_only_1m_claude_windows() {
        assert_eq!(context_window_for_model("fable"), 1_000_000);
        assert_eq!(context_window_for_model("claude-fable-5"), 1_000_000);
        assert_eq!(context_window_for_model("opus"), 1_000_000);
        assert_eq!(context_window_for_model("claude-opus-5"), 1_000_000);
        assert_eq!(context_window_for_model("claude-opus-4-8"), 1_000_000);
        assert_eq!(context_window_for_model("claude-opus-4-6[1m]"), 1_000_000);
    }

    #[test]
    fn opus_1m_label_alias_resolves_to_bare_opus_with_1m_window() {
        // The `opus[1m]` picker label is the same model as `opus`: it resolves
        // to the bare `claude-opus-5` (so the `[1m]` suffix never reaches the
        // wire and cannot 404) and still reports the native 1M window, which on
        // Opus 5 is both the default and the maximum.
        assert_eq!(resolve_model_alias("opus[1m]"), "claude-opus-5");
        assert_eq!(resolve_model_alias("claude-opus[1m]"), "claude-opus-5");
        assert!(
            !resolve_model_alias("opus[1m]").contains('['),
            "the [1m] label must never reach the wire model id"
        );
        assert_eq!(context_window_for_model("opus[1m]"), 1_000_000);
        assert_eq!(context_window_for_model("claude-opus[1m]"), 1_000_000);
    }

    #[test]
    fn sonnet_5_uses_1m_context_window() {
        assert_eq!(context_window_for_model("sonnet"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet-5"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet-5[1m]"), 1_000_000);
    }

    #[test]
    fn legacy_sonnet_and_haiku_are_capped_at_258k_even_with_1m_suffix() {
        assert_eq!(context_window_for_model("claude-sonnet-4-6"), 258_000);
        assert_eq!(context_window_for_model("claude-sonnet-4-6[1m]"), 258_000);
        assert_eq!(context_window_for_model("haiku"), 258_000);
        assert_eq!(
            context_window_for_model("claude-haiku-4-5-20251001"),
            258_000
        );
        assert_eq!(
            context_window_for_model("claude-haiku-4-5-20251001[1m]"),
            258_000
        );
    }

    #[test]
    fn codex_spark_context_window_comes_from_capability_catalog() {
        let _lock = crate::test_env_lock();
        let _override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);
        assert_eq!(context_window_for_model("gpt-5.3-codex-spark"), 122_000);
    }

    #[test]
    fn gpt_context_window_comes_from_capability_catalog() {
        let _lock = crate::test_env_lock();
        let _override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);
        assert_eq!(context_window_for_model("gpt"), 258_000);
        // User-directed 2026-07-14: the whole GPT family rides the 258k
        // effective window — the 5.6 trio's declared 353k proved unusable in
        // live sessions past ~256k.
        assert_eq!(context_window_for_model("gpt-5.6-sol"), 258_000);
        assert_eq!(context_window_for_model("gpt-5.6-sol-2026-07-09"), 258_000);
        assert_eq!(context_window_for_model("gpt-5.6-terra"), 258_000);
        assert_eq!(context_window_for_model("gpt-5.6-terra@openai"), 258_000);
        assert_eq!(context_window_for_model("gpt-5.6-luna"), 258_000);
        assert_eq!(context_window_for_model("gpt-5.6-luna[fast]"), 258_000);
        assert_eq!(context_window_for_model("gpt-5.5"), 258_000);
        assert_eq!(context_window_for_model("gpt-5.5-fast"), 258_000);
        assert_eq!(context_window_for_model("gpt-6-preview"), 200_000);
    }

    #[test]
    fn model_metadata_helpers_stay_consistent_across_alias_and_canonical_ids() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let _override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);

        for (alias, canonical, provider, context_window, adaptive) in [
            (
                "opus",
                "claude-opus-5",
                ProviderKind::Anthropic,
                1_000_000,
                true,
            ),
            (
                "claude-opus-4-8",
                "claude-opus-4-8",
                ProviderKind::Anthropic,
                1_000_000,
                true,
            ),
            (
                "sonnet",
                "claude-sonnet-5",
                ProviderKind::Anthropic,
                1_000_000,
                true,
            ),
            (
                "gpt-5.6-sol",
                "gpt-5.6-sol",
                ProviderKind::OpenAi,
                258_000,
                false,
            ),
            (
                "gpt-5.6-terra",
                "gpt-5.6-terra",
                ProviderKind::OpenAi,
                258_000,
                false,
            ),
            (
                "gpt-5.6-luna",
                "gpt-5.6-luna",
                ProviderKind::OpenAi,
                258_000,
                false,
            ),
            // gpt-5.5는 카탈로그 퇴역(2026-07-11) — bare 세대 별칭은 이제
            // gpt-5.6→terra다. 컨텍스트 윈도는 terra의 것.
            (
                "gpt-5.6",
                "gpt-5.6-terra",
                ProviderKind::OpenAi,
                258_000,
                false,
            ),
            (
                "gemini-flash",
                "gemini-3.6-flash",
                ProviderKind::Google,
                1_000_000,
                false,
            ),
        ] {
            assert_eq!(resolve_model_alias(alias), canonical, "alias {alias}");
            assert_eq!(
                detect_provider_kind(canonical),
                provider,
                "provider {alias}"
            );
            assert_eq!(
                context_window_for_model(alias),
                context_window,
                "alias context {alias}"
            );
            assert_eq!(
                context_window_for_model(canonical),
                context_window,
                "canonical context {canonical}"
            );
            assert_eq!(
                uses_adaptive_thinking(alias),
                adaptive,
                "alias adaptive thinking {alias}"
            );
            assert_eq!(
                uses_adaptive_thinking(canonical),
                adaptive,
                "canonical adaptive thinking {canonical}"
            );
        }
    }

    #[test]
    fn model_capability_is_a_narrow_projection_of_existing_helpers() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let _override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);

        for model in ["opus", "gpt-5.5", "gemini-flash", "grok", "unknown-model"] {
            let capability = model_capability_for_model(model);
            assert_eq!(capability.canonical_model_id, resolve_model_alias(model));
            assert_eq!(
                capability.provider.map(ProviderKind::metadata),
                super::metadata_for_model(model)
            );
            assert_eq!(
                capability.context_window,
                Some(context_window_for_model(model))
            );
            assert_eq!(
                capability.max_output_tokens,
                Some(max_tokens_for_model(model))
            );
            assert_eq!(
                capability.adaptive_thinking,
                Some(uses_adaptive_thinking(model))
            );
        }
    }

    #[test]
    fn custom_provider_context_and_output_overrides_are_used() {
        let _lock = crate::test_env_lock();
        let _catalog_override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);
        super::refresh_custom_providers_from_json(
            r#"[{"name":"xai-custom","base_url":"https://api.x.ai/v1","models":["grok-4.5"],"requires_auth":false,"context_window":256000,"max_output_tokens":32000}]"#,
        )
        .expect("refresh custom provider");

        assert_eq!(context_window_for_model("grok-4.5"), 256_000);
        assert_eq!(max_tokens_for_model("grok-4.5"), 32_000);

        super::refresh_custom_providers_from_json("[]").expect("restore empty custom providers");
    }

    #[test]
    fn unknown_grok_versions_do_not_inherit_grok3_context_window() {
        let _lock = crate::test_env_lock();
        let _catalog_override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);
        super::refresh_custom_providers_from_json("[]").expect("clear custom providers");
        assert_eq!(context_window_for_model("grok-3"), 131_072);
        assert_eq!(context_window_for_model("grok-4.5"), 200_000);
    }

    #[test]
    fn context_window_can_be_supplied_without_code_changes() {
        let _lock = crate::test_env_lock();
        let _override = EnvVarGuard::set(
            MODEL_CONTEXT_WINDOWS_ENV,
            Some(r#"{"models":[{"ids":["gpt-future"],"context_window":777000}]}"#),
        );
        assert_eq!(context_window_for_model("gpt-future"), 777_000);
    }

    #[test]
    fn resolves_claude_aliases_and_normalizes_dotted_versions() {
        // Anthropic is always enabled, so these hold regardless of the gate.
        assert_eq!(resolve_model_alias("fable"), "claude-fable-5-1");
        assert_eq!(resolve_model_alias("claude-fable"), "claude-fable-5-1");
        // A release-bearing id is a pin: Fable 5 stays Fable 5 beside its head.
        assert_eq!(resolve_model_alias("claude-fable-5"), "claude-fable-5");
        assert_eq!(resolve_model_alias("fable-5"), "claude-fable-5");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-5");
        assert_eq!(resolve_model_alias("claude-opus"), "claude-opus-5");
        // The previous-generation canonical stays registered and resolvable.
        assert_eq!(resolve_model_alias("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-5");
        assert_eq!(resolve_model_alias("claude-sonnet"), "claude-sonnet-5");
        assert_eq!(
            resolve_model_alias("claude-haiku"),
            "claude-haiku-4-5-20251001"
        );
        // Dot-separated versions of fully-qualified ids normalise to hyphens.
        assert_eq!(resolve_model_alias("claude-opus-4.6"), "claude-opus-4-6");
        assert_eq!(
            resolve_model_alias("claude-sonnet-4.6"),
            "claude-sonnet-4-6"
        );
        // Non-Claude ids are passed through verbatim.
        assert_eq!(resolve_model_alias("grok-3"), "grok-3");
    }

    #[test]
    fn provider_capabilities_distinguish_cache_from_thinking_support() {
        assert!(ProviderKind::Anthropic.supports_cache_tokens());
        assert!(ProviderKind::OpenAi.supports_cache_tokens());
        assert!(ProviderKind::Anthropic.supports_thinking());
        assert_eq!(
            ProviderKind::Anthropic.prompt_cache_strategy(),
            PromptCacheStrategy::AnthropicCacheControl
        );
        assert_eq!(
            ProviderKind::OpenAi.prompt_cache_strategy(),
            PromptCacheStrategy::OpenAiPromptCacheKey
        );
        for kind in [
            ProviderKind::Google,
            ProviderKind::Xai,
            ProviderKind::Ollama,
        ] {
            assert!(!kind.supports_cache_tokens(), "{kind} should not cache");
            assert!(!kind.supports_thinking(), "{kind} should not think");
            assert_eq!(
                kind.prompt_cache_strategy(),
                PromptCacheStrategy::NoRequestControls
            );
        }
        assert_eq!(ProviderKind::Anthropic.rate_limit_key(), "anthropic");
        assert_eq!(ProviderKind::OpenAi.rate_limit_key(), "openai");
    }

    #[test]
    fn every_provider_profile_is_fully_populated() {
        // The single profile table is the source of truth: a new provider that
        // forgets a field would surface here rather than silently shipping an
        // empty display name or rate-limit key. Display must echo the profile,
        // and metadata must mirror the profile's connection wiring.
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::Xai,
            ProviderKind::OpenAi,
            ProviderKind::Google,
            ProviderKind::Ollama,
        ] {
            let profile = kind.profile();
            assert!(!profile.display_name.is_empty(), "{kind} display name");
            assert!(!profile.rate_limit_key.is_empty(), "{kind} rate-limit key");
            assert!(!profile.auth_env.is_empty(), "{kind} auth env");
            assert!(!profile.base_url_env.is_empty(), "{kind} base-url env");
            assert!(
                !profile.default_base_url.is_empty(),
                "{kind} default base url"
            );
            assert_eq!(kind.to_string(), profile.display_name, "{kind} Display");
            let metadata = kind.metadata();
            assert_eq!(metadata.auth_env, profile.auth_env);
            assert_eq!(metadata.base_url_env, profile.base_url_env);
            assert_eq!(metadata.default_base_url, profile.default_base_url);
        }
    }

    #[test]
    fn families_and_names_come_from_the_id_grammar() {
        use ProviderKind::{Anthropic, Google, OpenAi};
        assert_eq!(family_from_id(OpenAi, "gpt-5.6-sol").as_deref(), Some("sol"));
        assert_eq!(family_from_id(OpenAi, "gpt-5.6-terra[fast]").as_deref(), Some("terra"));
        assert_eq!(family_from_id(OpenAi, "gpt-6-astra").as_deref(), Some("astra"));
        assert_eq!(family_from_id(OpenAi, "gpt-5.3-codex-spark").as_deref(), Some("spark"));
        assert_eq!(family_from_id(OpenAi, "gpt-5.5-fast").as_deref(), Some("gpt"));
        assert_eq!(family_from_id(OpenAi, "gpt-5.4-mini").as_deref(), Some("mini"));
        assert_eq!(family_from_id(Anthropic, "claude-fable-5-1").as_deref(), Some("fable"));
        assert_eq!(family_from_id(Anthropic, "claude-haiku-4-5-20251001").as_deref(), Some("haiku"));
        assert_eq!(family_from_id(Anthropic, "opus[1m]").as_deref(), Some("opus"));
        assert_eq!(family_from_id(Google, "gemini-3.1-flash-lite").as_deref(), Some("flash-lite"));
        assert_eq!(family_from_id(Google, "gemini-3.6-flash-low").as_deref(), Some("flash"));
        assert_eq!(family_from_id(Google, "gemini-3.1-pro-preview-customtools").as_deref(), Some("pro"));
        assert_eq!(family_from_id(ProviderKind::Xai, "grok-3"), None);

        assert_eq!(display_name_from_id(OpenAi, "gpt-5.6-sol"), "GPT-5.6-Sol");
        assert_eq!(display_name_from_id(OpenAi, "gpt-6-astra"), "GPT-6-Astra");
        assert_eq!(display_name_from_id(OpenAi, "gpt-5.3-codex-spark"), "GPT-5.3-Codex-Spark");
        assert_eq!(display_name_from_id(Anthropic, "claude-fable-5-1"), "Fable 5.1");
        assert_eq!(display_name_from_id(Anthropic, "claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(display_name_from_id(Anthropic, "claude-opus-5"), "Opus 5");
        assert_eq!(display_name_from_id(Google, "gemini-3.1-flash-lite"), "Gemini 3.1 Flash Lite");

        // Through the catalog: aliases resolve first, and a registry row
        // decides the provider before the id's naming does.
        assert_eq!(model_family("sol").as_deref(), Some("sol"));
        assert_eq!(model_family("opus").as_deref(), Some("opus"));
        assert_eq!(model_display_name("fable"), "Fable 5.1");
        assert_eq!(model_display_name("gemini-flash"), "Gemini 3.6 Flash");
        assert!(is_openai_model("sol"));
        assert!(is_openai_model("gpt-7-nova"));
        assert!(!is_openai_model("claude-opus-5"));
        assert!(!is_openai_model("deepseek-chat"));
        // The lineup word is grammar, not registration: `gpt` names a lineup,
        // `sol` a release's codename even though both are OpenAI names.
        assert!(is_openai_lineup_word("gpt") && is_openai_lineup_word("Codex"));
        assert!(!is_openai_lineup_word("sol") && !is_openai_lineup_word("astra"));
    }

    /// The capability facts a lineup used to be named for are catalog rows
    /// now: a row the binary predates carries its own effort scale, priority
    /// tier and capabilities, and the rules read them.
    #[test]
    fn a_discovered_row_brings_its_own_effort_scale_and_priority_tier() {
        use crate::types::EffortLevel::{Max, Ultra, Xhigh};
        let _lock = crate::test_env_lock();
        let _ceilings = EnvVarGuard::set(MODEL_EFFORT_CEILINGS_ENV, None);
        let _catalog = EnvVarGuard::set(
            MODEL_CONTEXT_WINDOWS_ENV,
            Some(r#"{"models":[{"provider":"openai","ids":["gpt-6-astra"],"context_window":258000,"effort_levels":["low","medium","high","xhigh","max","ultra"],"speed_tiers":["fast"],"capabilities":["imagegen"]},{"provider":"openai","ids":["gpt-6-quiet"],"context_window":258000}]}"#),
        );
        assert_eq!(known_effort_ceiling("gpt-6-astra"), Some(Ultra));
        assert!(model_accepts_effort("gpt-6-astra", Ultra));
        assert_eq!(Ultra.gpt_for_model("gpt-6-astra"), "max");
        assert_eq!(effective_effort_for_model(Ultra, "gpt-6-astra"), Ultra);
        assert_eq!(
            super::openai_fast_variant_pair("gpt-6-astra"),
            Some(("gpt-6-astra".to_string(), "gpt-6-astra[fast]".to_string()))
        );
        assert!(openai_fast_tier_enabled("gpt-6-astra[fast]"));
        assert!(model_has_capability("gpt-6-astra", "imagegen"));
        // A row that declares nothing falls back to what OpenAI documents for
        // every model: the `xhigh`-topped enum, no priority tier, no tools.
        assert_eq!(known_effort_ceiling("gpt-6-quiet"), Some(Xhigh));
        assert!(!model_accepts_effort("gpt-6-quiet", Max));
        assert_eq!(super::openai_fast_variant_pair("gpt-6-quiet"), None);
        assert!(!model_has_capability("gpt-6-quiet", "imagegen"));
    }

    #[test]
    fn openai_extended_prompt_cache_policy_does_not_send_unverified_retention() {
        for model in [
            "gpt-5.5",
            "gpt-5.5-fast",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.3-codex-spark",
        ] {
            assert_eq!(openai_prompt_cache_retention(model), None, "{model}");
        }
    }

    #[test]
    fn model_supports_vision_reflects_catalog_declarations() {
        // Frontier and balanced models accept multimodal image inputs
        assert!(model_supports_vision("gpt-5.6-sol"));
        assert!(model_supports_vision("gpt-5.6-terra"));
        assert!(model_supports_vision("gpt-5.5"));
        assert!(model_supports_vision("claude-fable-5-1"));
        // Codex Spark declares `no_vision` in catalog capabilities
        assert!(!model_supports_vision("gpt-5.3-codex-spark"));
        assert!(model_has_capability("gpt-5.3-codex-spark", "no_vision"));
    }

    #[test]
    fn catalog_entries_derive_metadata_from_their_provider() {
        for entry in provider_catalog() {
            assert_eq!(entry.metadata(), entry.provider.metadata());
            assert_eq!(entry.metadata().provider, entry.provider);
            assert_eq!(
                entry.supports_cache_tokens(),
                entry.provider.supports_cache_tokens()
            );
            // Every alias maps to a non-empty canonical id.
            assert!(!entry.canonical_model_id.is_empty());
        }
        // The catalog is the single source of truth for alias resolution.
        let opus = provider_catalog()
            .iter()
            .find(|entry| entry.alias == "opus")
            .expect("opus alias present in catalog");
        assert_eq!(opus.canonical_model_id, "claude-opus-5");
        assert_eq!(opus.provider, ProviderKind::Anthropic);
    }

    fn resolved(json: &str) -> super::ResolvedCustomProvider {
        let custom: super::openai_compat::CustomProviderConfig =
            super::parse_custom_providers(&format!("[{json}]"))
                .expect("valid custom provider json")
                .pop()
                .expect("one provider parsed");
        super::ResolvedCustomProvider::from(custom)
    }

    #[test]
    fn custom_provider_parses_models_and_auth_defaults() {
        let provider = resolved(
            r#"{"name":"Local","base_url":"http://localhost:8080/v1",
                "auth_env":"LOCAL_KEY","models":["llama-3.3","mistral-large"]}"#,
        );
        // `requires_auth` defaults to true when omitted.
        assert!(provider.requires_auth);
        assert_eq!(provider.config.provider_name, "Local");
        assert_eq!(provider.config.api_key_env, "LOCAL_KEY");
        assert_eq!(provider.config.credential_env_vars(), &["LOCAL_KEY"]);
        assert_eq!(provider.config.default_base_url, "http://localhost:8080/v1");
        assert!(provider.config.request_stream_usage);
        assert!(provider.fit_hint.is_none());
    }

    #[test]
    fn custom_provider_matches_models_case_insensitively() {
        let provider = resolved(
            r#"{"name":"Local","base_url":"http://localhost:8080/v1",
                "models":["Llama-3.3"],"requires_auth":false}"#,
        );
        assert!(provider.serves("llama-3.3"));
        assert!(provider.serves("LLAMA-3.3"));
        assert!(!provider.serves("gpt-5.6-sol"));
        // The registered casing is the canonical id sent to the endpoint.
        assert_eq!(provider.canonical_model("llama-3.3"), Some("Llama-3.3"));
        // Keyless self-host: no auth env, no key required.
        assert!(!provider.requires_auth);
        assert_eq!(provider.config.api_key_env, "");
    }

    #[test]
    fn custom_provider_preserves_read_only_fit_metadata() {
        let provider = resolved(
            r#"{"name":"LM Studio","base_url":"http://localhost:1234/v1",
                "models":["qwen2.5-coder-32b"],"requires_auth":false,
                "estimated_vram_gb":22,"quantization":"Q4_K_M"}"#,
        );
        let hint = provider.fit_hint.expect("fit hint");
        assert_eq!(hint.estimated_vram_gb, 22);
        assert_eq!(hint.quantization, "Q4_K_M");
        assert_eq!(hint.display_label(), "VRAM ~22GB Q4_K_M");
    }

    #[test]
    fn custom_provider_can_disable_stream_usage_opt_in() {
        let provider = resolved(
            r#"{"name":"LocalAI","base_url":"http://localhost:8080/v1",
                "models":["localai-model"],"requires_auth":false,"include_usage":false}"#,
        );
        assert!(!provider.config.request_stream_usage);
    }

    #[test]
    fn custom_provider_json_array_round_trips_and_rejects_garbage() {
        let parsed = super::parse_custom_providers(
            r#"[{"name":"A","base_url":"http://a/v1","models":["x"]},
                {"name":"B","base_url":"http://b/v1","models":["y"],"requires_auth":false}]"#,
        )
        .expect("two providers");
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].requires_auth);
        assert!(!parsed[1].requires_auth);
        assert!(parsed[0].include_usage);
        assert!(parsed[1].include_usage);
        // Malformed JSON is a hard parse error (surfaced, not silently dropped).
        assert!(super::parse_custom_providers("not json").is_err());
    }

    #[test]
    fn custom_provider_catalog_can_refresh_after_initial_empty_read() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        super::refresh_custom_providers_from_json("[]").expect("clear custom catalog");
        assert!(super::custom_provider_catalog().is_empty());

        super::refresh_custom_providers_from_json(
            r#"[{"name":"deepseek","base_url":"https://api.deepseek.com",
                "auth_env":"DEEPSEEK_API_KEY",
                "models":["deepseek-chat","deepseek-reasoner"]}]"#,
        )
        .expect("refresh custom catalog");

        let catalog = super::custom_provider_catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].0, "deepseek");
        assert_eq!(
            catalog[0].1,
            vec![
                "deepseek-chat".to_string(),
                "deepseek-reasoner".to_string()
            ]
        );
        assert_eq!(resolve_model_alias("deepseek-chat"), "deepseek-chat");
        assert!(super::custom_provider_for_model("deepseek-reasoner").is_some());

        super::refresh_custom_providers_from_json("[]").expect("restore empty custom catalog");
    }

    #[test]
    fn resolve_model_alias_static_registry_wins_over_custom_collision() {
        // Built-in aliases are matched before the custom catalog, so a custom
        // provider can never shadow them. `opus` (Anthropic, always enabled)
        // demonstrates this without any adapter-gate setup.
        assert_eq!(resolve_model_alias("opus"), "claude-opus-5");
        // A model with neither a registry entry nor a custom provider passes
        // through unchanged — the custom fallback never fabricates an id.
        assert_eq!(
            resolve_model_alias("totally-unknown-model"),
            "totally-unknown-model"
        );
    }

    /// A gateway's model ids carry their own slash (`anthropic/claude-x` on
    /// `OpenRouter`). A custom provider that DECLARES such an id must win over
    /// the built-in prefix guess, and the wire id it sends must be the whole
    /// declared id — not the half after the last slash. The explicit form
    /// `openrouter/anthropic/claude-x` splits at the provider NAME, not at the
    /// last slash.
    #[test]
    fn a_declared_model_with_its_own_slash_routes_to_its_provider_whole() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        super::refresh_custom_providers_from_json(
            r#"[{"name":"openrouter","base_url":"https://openrouter.ai/api/v1",
                "auth_env":"ZO_OPENROUTER_KEY",
                "models":["anthropic/claude-x","meta/llama-9"],"requires_auth":false}]"#,
        )
        .expect("load openrouter");

        // Explicit: the provider name is the first segment, the rest is the model.
        assert_eq!(
            super::split_provider_model_ref("openrouter/anthropic/claude-x"),
            Some(("openrouter", "anthropic/claude-x"))
        );
        assert_eq!(super::wire_model_id("openrouter/anthropic/claude-x"), "anthropic/claude-x");
        assert_eq!(
            super::detect_provider_kind("openrouter/anthropic/claude-x"),
            ProviderKind::OpenAi
        );
        // Bare, but declared: the declaration wins over the `anthropic/` prefix.
        assert!(super::custom_provider_for_model("anthropic/claude-x").is_some());
        assert_eq!(super::wire_model_id("anthropic/claude-x"), "anthropic/claude-x");
        assert_eq!(
            super::detect_provider_kind("anthropic/claude-x"),
            ProviderKind::OpenAi
        );
        // Undeclared `vendor/model` keeps the old reading (a real Anthropic id).
        assert_eq!(
            super::split_provider_model_ref("anthropic/claude-opus-5"),
            Some(("anthropic", "claude-opus-5"))
        );
        assert_eq!(super::wire_model_id("anthropic/claude-opus-5"), "claude-opus-5");

        super::refresh_custom_providers_from_json("[]").expect("restore empty custom providers");
    }

    /// A key the window hands zo for itself is adopted out of the process
    /// environment: zo's own requests still read it, and nothing zo spawns — an
    /// MCP server fetched with `npx …@latest`, a tool's shell, a hook — inherits
    /// it. (09-11, 1.3.36: five MCP servers under a window-launched zo held the
    /// router key.)
    #[test]
    fn an_adopted_launch_key_serves_zo_and_no_child() {
        let _lock = crate::test_env_lock();
        let _custom = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        let name = "ZO_TEST_ADOPT_ROUTER_KEY";
        let _key = EnvVarGuard::set(name, Some("dummy-adopted-value"));
        super::refresh_custom_providers_from_json(&format!(
            r#"[{{"name":"adoptrouter","base_url":"http://127.0.0.1:9/v1",
                 "models":["adopt/model"],"requires_auth":true,"auth_env":"{name}"}}]"#
        ))
        .expect("provider");

        assert_eq!(super::adopt_launch_keys("ZO_TEST_ADOPT_"), 1);
        assert!(std::env::var_os(name).is_none(), "the key left the environment");
        assert_eq!(
            super::read_env_non_empty(name).expect("readable").as_deref(),
            Some("dummy-adopted-value"),
            "zo's own requests still read it"
        );
        assert!(
            super::custom_provider_usable_catalog()
                .iter()
                .any(|(provider, _)| *provider == "adoptrouter"),
            "the provider stays usable"
        );
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf %s \"${name}\"")])
            .output()
            .expect("child");
        assert!(child.stdout.is_empty(), "a child inherited the adopted key");

        super::forget_adopted_launch_keys_for_test();
        super::refresh_custom_providers_from_json("[]").expect("clear");
    }

    /// What the fake keychain below was asked, in order.
    static KEYCHAIN_ASKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    /// A keychain holding one router key and one service key, under the
    /// services their tests name.
    fn fake_keychain(service: &str) -> super::KeychainAnswer {
        KEYCHAIN_ASKED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(service.to_string());
        if matches!(
            service,
            "test.router.ZO_TEST_KC_HELD" | "test.key.ZO_TEST_SERVICE_KEY" | "test.key.TYPESAFE_API_KEY"
        ) {
            super::KeychainAnswer::Found("dummy-keychain-value".to_string())
        } else {
            super::KeychainAnswer::Absent
        }
    }

    /// TypeSafe's key is saved in the window's settings under its own variable
    /// name — `TYPESAFE_API_KEY`, the name every TypeSafe SDK reads — so no
    /// prefix of the window's marks it and the road names it exactly. zo reads
    /// it once when first needed, keeps it out of the environment, and never
    /// asks the keychain about a neighbouring name.
    #[test]
    fn a_service_key_the_window_keeps_under_its_own_name_is_read_once_from_the_keychain() {
        let _lock = crate::test_env_lock();
        let named = "ZO_TEST_SERVICE_KEY";
        let neighbour = "ZO_TEST_SERVICE_KEY_2";
        let _named = EnvVarGuard::set(named, None);
        let _neighbour = EnvVarGuard::set(neighbour, None);
        super::forget_adopted_launch_keys_for_test();
        KEYCHAIN_ASKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        super::install_window_keychain(
            super::WindowKeyNames::Exactly(vec![named.to_string()]),
            "test.key.",
            fake_keychain,
        );

        assert_eq!(
            super::read_env_non_empty(named).expect("readable").as_deref(),
            Some("dummy-keychain-value"),
            "zo's own requests read it"
        );
        assert!(
            super::read_env_non_empty(neighbour).expect("readable").is_none(),
            "a name the road does not list has no keychain road, however close"
        );
        assert_eq!(
            super::read_env_non_empty(named).expect("readable").as_deref(),
            Some("dummy-keychain-value"),
            "the second read is the adopted copy"
        );
        assert_eq!(
            *KEYCHAIN_ASKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            ["test.key.ZO_TEST_SERVICE_KEY"],
            "asked once, and only for the name it lists"
        );
        assert!(std::env::var_os(named).is_none(), "the key never entered the environment");

        super::forget_adopted_launch_keys_for_test();
    }

    /// Each rung of the ladder names itself, and the name survives the read
    /// that caches it: the keychain is asked once per variable, so a second
    /// reader that called its answer "adopted" would tell a person looking
    /// for why their key works the wrong story (t-5805).
    #[test]
    fn the_key_ladder_names_the_rung_that_answered() {
        let _lock = crate::test_env_lock();
        let named = crate::SYSTEMONE_API_KEY_ENV;

        let _set = EnvVarGuard::set(named, Some("from-the-shell"));
        super::forget_adopted_launch_keys_for_test();
        assert_eq!(
            super::read_env_key(named).expect("a readable variable"),
            Some(("from-the-shell".to_string(), super::KeySource::Environment))
        );

        let _unset = EnvVarGuard::set(named, None);
        super::forget_adopted_launch_keys_for_test();
        assert_eq!(super::read_env_key(named).expect("no variable"), None);

        super::install_window_keychain(
            super::WindowKeyNames::Exactly(vec![named.to_string()]),
            "test.key.",
            fake_keychain,
        );
        let first = super::read_env_key(named).expect("the window's keychain");
        let again = super::read_env_key(named).expect("the same key, read back");
        assert_eq!(
            first.as_ref().map(|(_, source)| *source),
            Some(super::KeySource::WindowKeychain),
            "the rung the key actually came from"
        );
        assert_eq!(first, again, "a cached read tells the same story");
        assert_eq!(
            super::KeySource::WindowKeychain.token(),
            "window_keychain",
            "the word a report and a JSON answer name it by"
        );

        super::forget_adopted_launch_keys_for_test();
    }

    /// The road ends where the key is used: System One's client, asked with no
    /// `TYPESAFE_API_KEY` in its environment, is configured by the key the
    /// window keeps under that name — and says `NoKey` without it.
    #[test]
    fn system_ones_client_is_configured_by_the_typesafe_key_the_window_keeps() {
        let _lock = crate::test_env_lock();
        let _key = EnvVarGuard::set(crate::SYSTEMONE_API_KEY_ENV, None);
        super::forget_adopted_launch_keys_for_test();
        assert_eq!(
            crate::SystemOneConfig::from_env().err(),
            Some(crate::SystemOneFailure::NoKey),
            "no environment and no road: nothing is configured"
        );

        super::forget_adopted_launch_keys_for_test();
        super::install_window_keychain(
            super::WindowKeyNames::Exactly(vec![crate::SYSTEMONE_API_KEY_ENV.to_string()]),
            "test.key.",
            fake_keychain,
        );
        assert!(
            crate::SystemOneConfig::from_env().is_ok(),
            "the key the window keeps configures the client"
        );

        super::forget_adopted_launch_keys_for_test();
    }

    /// How many times [`flaky_keychain`] has been asked.
    static FLAKY_ASKS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);

    /// A keychain whose first read cannot be answered and whose second finds
    /// the key.
    fn flaky_keychain(_service: &str) -> super::KeychainAnswer {
        let mut asks = FLAKY_ASKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *asks += 1;
        if *asks > 1 {
            super::KeychainAnswer::Found("dummy-keychain-value".to_string())
        } else {
            super::KeychainAnswer::Unanswered
        }
    }

    /// A read the keychain could not answer — `security` never ran, an
    /// interaction this session may not have, a lock another process holds —
    /// is not the answer "this machine keeps no such key". Remembered as one,
    /// zo goes without a key it does have for as long as the process lives,
    /// which is how a routing judgment writes `no_key` beside a keychain item
    /// that is sitting right there. An item that is genuinely absent is still
    /// asked once (`a_router_key_zo_was_not_handed_…`).
    #[test]
    fn a_keychain_read_that_could_not_be_answered_is_asked_again() {
        let _lock = crate::test_env_lock();
        let named = "ZO_TEST_FLAKY_KEY";
        let _named = EnvVarGuard::set(named, None);
        super::forget_adopted_launch_keys_for_test();
        *FLAKY_ASKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
        super::install_window_keychain(
            super::WindowKeyNames::Exactly(vec![named.to_string()]),
            "test.key.",
            flaky_keychain,
        );

        assert!(
            super::read_env_non_empty(named).expect("readable").is_none(),
            "the first read could not be answered, so nothing is configured yet"
        );
        assert_eq!(
            super::read_env_non_empty(named).expect("readable").as_deref(),
            Some("dummy-keychain-value"),
            "a read that failed is asked again, and this one finds the key"
        );
        assert_eq!(
            *FLAKY_ASKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            2,
            "asked again exactly once, not on a loop"
        );

        super::forget_adopted_launch_keys_for_test();
    }

    /// A zo typed into a shell pane was handed no router key — the window
    /// hands keys to the zo it launches, and a shell is not one (09-12: the
    /// person's `AgentRouter` models never reached a zo typed at a prompt). It
    /// reads the key from the window's keychain item once, on first need, keeps
    /// it where zo's own requests read it, and leaves the environment bare. A
    /// variable outside the window's prefix never asks the keychain, and a
    /// missing item is asked once and stays missing.
    #[test]
    fn a_router_key_zo_was_not_handed_is_read_once_from_the_windows_keychain() {
        let _lock = crate::test_env_lock();
        let _custom = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        let held = "ZO_TEST_KC_HELD";
        let missing = "ZO_TEST_KC_MISSING";
        let _held = EnvVarGuard::set(held, None);
        let _missing = EnvVarGuard::set(missing, None);
        super::forget_adopted_launch_keys_for_test();
        KEYCHAIN_ASKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        super::refresh_custom_providers_from_json(&format!(
            r#"[{{"name":"kcrouter","base_url":"http://127.0.0.1:9/v1",
                 "models":["kc/model"],"requires_auth":true,"auth_env":"{held}"}},
                {{"name":"gonerouter","base_url":"http://127.0.0.1:9/v1",
                 "models":["gone/model"],"requires_auth":true,"auth_env":"{missing}"}}]"#
        ))
        .expect("providers");
        super::install_window_keychain(
            super::WindowKeyNames::Prefix("ZO_TEST_KC_".to_string()),
            "test.router.",
            fake_keychain,
        );

        let usable = super::custom_provider_usable_catalog();
        assert!(
            usable.iter().any(|(provider, _)| *provider == "kcrouter"),
            "the router whose key the window keeps is usable: {usable:?}"
        );
        assert!(
            !usable.iter().any(|(provider, _)| *provider == "gonerouter"),
            "a router with no key anywhere stays hidden"
        );
        assert_eq!(
            super::read_env_non_empty(held).expect("readable").as_deref(),
            Some("dummy-keychain-value"),
            "zo's own requests read it"
        );
        assert!(super::read_env_non_empty(missing).expect("readable").is_none());
        assert!(
            super::read_env_non_empty("ZO_TEST_OTHER_KEY").expect("readable").is_none(),
            "a variable outside the prefix has no keychain road"
        );
        let _ = super::custom_provider_usable_catalog();
        assert_eq!(
            *KEYCHAIN_ASKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            ["test.router.ZO_TEST_KC_HELD", "test.router.ZO_TEST_KC_MISSING"],
            "each variable is asked once, and only the window's"
        );
        assert!(std::env::var_os(held).is_none(), "the key never entered the environment");
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf %s \"${held}\"")])
            .output()
            .expect("child");
        assert!(child.stdout.is_empty(), "a child saw the keychain key");

        super::forget_adopted_launch_keys_for_test();
        super::refresh_custom_providers_from_json("[]").expect("clear");
    }

    #[test]
    fn qualified_provider_model_selects_custom_without_shadowing_bare_claude() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        super::refresh_custom_providers_from_json(
            r#"[{"name":"agent router","base_url":"https://agentrouter.org/v1",
                "auth_env":"ZO_AGENT_ROUTER_API_KEY",
                "models":["claude-opus-4-8"],"requires_auth":false}]"#,
        )
        .expect("load agent router");

        // Bare Claude id stays first-party / OAuth — at the lookup too, so no
        // caller can hand it to the gateway: the client built for it is
        // Anthropic's, not the custom one.
        assert_eq!(resolve_model_alias("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-5");
        let client = crate::ProviderClient::from_model_with_auth_route_and_anthropic_auth(
            "claude-opus-4-8",
            crate::AuthRoute::Auto,
            Some(crate::AuthSource::ApiKey("test-key".to_string())),
        )
        .expect("first-party client");
        assert!(
            matches!(client, crate::ProviderClient::Anthropic(_)),
            "a gateway listing the id took the bare first-party model"
        );
        assert!(super::custom_provider_for_model("claude-opus-4-8").is_none());
        assert!(super::custom_provider_for_model("opus").is_none());
        assert_eq!(
            super::detect_provider_kind("claude-opus-4-8"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            super::wire_model_id("claude-opus-4-8"),
            "claude-opus-4-8"
        );

        // Explicit provider/model opts into the custom gateway.
        let qualified = "agent router/claude-opus-4-8";
        assert_eq!(resolve_model_alias(qualified), qualified);
        assert_eq!(super::wire_model_id(qualified), "claude-opus-4-8");
        let custom = super::custom_provider_for_model(qualified).expect("qualified custom");
        assert_eq!(custom.config.provider_name, "agent router");
        assert_eq!(
            super::detect_provider_kind(qualified),
            ProviderKind::OpenAi
        );
        assert_eq!(
            super::split_provider_model_ref(qualified),
            Some(("agent router", "claude-opus-4-8"))
        );
        assert_eq!(
            super::format_provider_model_ref("agent router", "claude-opus-4-8"),
            qualified
        );

        super::refresh_custom_providers_from_json("[]").expect("clear");
    }

    /// The picker writes every connected model as `<provider>/<model>`. When a
    /// second gateway lists that same string as its own id (`OpenRouter`'s
    /// `deepseek/deepseek-chat` beside a provider named `DeepSeek`), the pick
    /// still goes to the provider it names — route, wire id and limits alike —
    /// and the gateway keeps its own id and its own `<name>/<id>` pick.
    #[test]
    fn an_explicit_pick_goes_to_the_provider_it_names_even_when_another_lists_the_string() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        super::refresh_custom_providers_from_json(
            r#"[{"name":"OpenRouter","base_url":"https://openrouter.example/v1",
                 "models":["deepseek/deepseek-chat"],"requires_auth":false,
                 "context_window":64000},
                {"name":"DeepSeek","base_url":"https://deepseek.example/v1",
                 "models":["DeepSeek-Chat"],"requires_auth":false,
                 "context_window":128000}]"#,
        )
        .expect("two providers");

        let picked = "DeepSeek/deepseek-chat";
        let routed = super::custom_provider_for_model(picked).expect("routed");
        assert_eq!(routed.config.provider_name, "DeepSeek");
        assert_eq!(super::wire_model_id(picked), "DeepSeek-Chat", "the operator's casing");
        assert_eq!(context_window_for_model(picked), 128_000);

        let gateway = "OpenRouter/deepseek/deepseek-chat";
        assert_eq!(
            super::custom_provider_for_model(gateway).expect("gateway").config.provider_name,
            "OpenRouter"
        );
        assert_eq!(super::wire_model_id(gateway), "deepseek/deepseek-chat");
        assert_eq!(context_window_for_model(gateway), 64_000);

        super::refresh_custom_providers_from_json("[]").expect("clear");
    }

    /// A connected router states each model's own limits: an entry of
    /// `models` may be `{"id", "context_window", "max_output_tokens"}` beside
    /// a plain id, in one list. The model's own numbers win over the
    /// provider-wide ones, which still speak for a model that states none — a
    /// 32k model behind a 1M one is not compacted as if it were 1M.
    #[test]
    fn a_models_entry_states_that_models_own_limits() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        let _catalog_override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);
        super::refresh_custom_providers_from_json(
            r#"[{"name":"OpenRouter","base_url":"https://openrouter.example/v1",
                 "requires_auth":false,"context_window":200000,"max_output_tokens":16000,
                 "models":["vendor/plain",
                           {"id":"vendor/Big","context_window":1048576,"max_output_tokens":65536},
                           {"id":"vendor/small","context_window":32768}]}]"#,
        )
        .expect("plain ids and model objects parse as one list");

        assert_eq!(
            super::custom_provider_catalog(),
            vec![(
                "OpenRouter",
                vec![
                    "vendor/plain".to_string(),
                    "vendor/Big".to_string(),
                    "vendor/small".to_string()
                ]
            )]
        );
        // Each model's own window, by the whole id or the named pick, in any casing.
        assert_eq!(context_window_for_model("vendor/small"), 32_768);
        assert_eq!(context_window_for_model("OpenRouter/vendor/big"), 1_048_576);
        assert_eq!(super::wire_model_id("OpenRouter/vendor/big"), "vendor/Big");
        // A model that states none reads the provider's.
        assert_eq!(context_window_for_model("vendor/plain"), 200_000);
        assert_eq!(context_window_for_model("OpenRouter/vendor/plain"), 200_000);
        // The same for the output cap, per field.
        assert_eq!(max_tokens_for_model("vendor/Big"), 65_536);
        assert_eq!(max_tokens_for_model("OpenRouter/vendor/small"), 16_000);

        super::refresh_custom_providers_from_json("[]").expect("clear");
    }

    /// A gateway serving a first-party id states its own limits for the
    /// `<gateway>/<id>` pick; the bare id keeps the first-party ones.
    #[test]
    fn an_explicit_pick_reads_the_named_providers_limits_not_the_first_party_ones() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(super::CUSTOM_PROVIDERS_ENV, None);
        let _catalog_override = EnvVarGuard::set(MODEL_CONTEXT_WINDOWS_ENV, None);
        let first_party_window = context_window_for_model("claude-opus-4-8");
        let first_party_output = max_tokens_for_model("claude-opus-4-8");
        super::refresh_custom_providers_from_json(
            r#"[{"name":"agent router","base_url":"https://agentrouter.example/v1",
                 "models":["claude-opus-4-8"],"requires_auth":false,
                 "context_window":200000,"max_output_tokens":32000}]"#,
        )
        .expect("agent router");

        let picked = "agent router/claude-opus-4-8";
        assert_eq!(context_window_for_model(picked), 200_000);
        assert_eq!(max_tokens_for_model(picked), 32_000);
        assert_eq!(context_window_for_model("claude-opus-4-8"), first_party_window);
        assert_eq!(max_tokens_for_model("claude-opus-4-8"), first_party_output);

        super::refresh_custom_providers_from_json("[]").expect("clear");
    }

    #[test]
    fn fit_hints_are_hidden_when_feature_is_disabled() {
        assert!(fit_hint_for_model("qwen2.5-coder-32b").is_none());
    }

    #[test]
    fn flatten_tool_result_content_joins_and_degrades() {
        use crate::types::{ImageSource, ToolResultContentBlock};

        let blocks = vec![
            ToolResultContentBlock::Text {
                text: "first".to_string(),
            },
            ToolResultContentBlock::Json {
                value: serde_json::json!({ "ok": true }),
            },
            ToolResultContentBlock::Image {
                source: ImageSource {
                    kind: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "ZGF0YQ==".to_string(),
                },
            },
        ];

        // Blocks join with '\n'; JSON serializes; images degrade to a placeholder.
        assert_eq!(
            super::flatten_tool_result_content(&blocks),
            "first\n{\"ok\":true}\n[image image/png]"
        );
        // Empty input yields an empty string (no leading newline).
        assert_eq!(super::flatten_tool_result_content(&[]), "");
    }

    #[test]
    fn backoff_for_attempt_doubles_caps_and_overflows() {
        use std::time::Duration;

        let initial = Duration::from_millis(500);
        let max = Duration::from_secs(8);

        // attempt 1 -> initial; then doubling each attempt …
        assert_eq!(
            super::backoff_for_attempt(1, initial, max).unwrap(),
            Duration::from_millis(500)
        );
        assert_eq!(
            super::backoff_for_attempt(2, initial, max).unwrap(),
            Duration::from_secs(1)
        );
        assert_eq!(
            super::backoff_for_attempt(4, initial, max).unwrap(),
            Duration::from_secs(4)
        );
        // … until the cap clamps it.
        assert_eq!(
            super::backoff_for_attempt(5, initial, max).unwrap(),
            Duration::from_secs(8)
        );
        // attempt 10 doubles to 256s but its shift still fits in u32, so it
        // clamps to the cap rather than overflowing (cf. attempt 40 below).
        assert_eq!(
            super::backoff_for_attempt(10, initial, max).unwrap(),
            Duration::from_secs(8)
        );

        // A multiplier shift that overflows u32 surfaces an error rather than
        // silently producing a bogus delay (drift the merge removed).
        assert!(matches!(
            super::backoff_for_attempt(40, initial, max),
            Err(crate::error::ApiError::BackoffOverflow { attempt: 40, .. })
        ));
    }

    #[test]
    fn should_restart_within_budget_adds_a_wallclock_ceiling() {
        use std::time::Duration;
        let cap = Duration::from_secs(120);

        // Before any restart (`elapsed == None`) it matches the plain predicate:
        // the first restart of a sequence is always allowed.
        assert!(super::should_restart_within_budget(
            false, true, 0, 5, None, cap
        ));
        // Within the wall-clock budget and under the attempt cap → still allowed.
        assert!(super::should_restart_within_budget(
            false,
            true,
            2,
            5,
            Some(Duration::from_secs(30)),
            cap
        ));
        // Past the wall-clock budget → denied even though attempts remain (the
        // silent-storm case: idle-timeout × restarts that never exhaust attempts
        // but hold the turn for minutes).
        assert!(!super::should_restart_within_budget(
            false,
            true,
            2,
            5,
            Some(Duration::from_secs(121)),
            cap
        ));
        // The wall-clock gate only tightens `should_restart`; it never overrides
        // commit/retryable/attempt denials.
        assert!(!super::should_restart_within_budget(
            true,
            true,
            0,
            5,
            Some(Duration::ZERO),
            cap
        ));
        assert!(!super::should_restart_within_budget(
            false,
            false,
            0,
            5,
            Some(Duration::ZERO),
            cap
        ));
        assert!(!super::should_restart_within_budget(
            false,
            true,
            5,
            5,
            Some(Duration::ZERO),
            cap
        ));
    }

    /// The 5.1 generation is recognised by id, in `[1m]` and dotted spellings
    /// too; Fable 5 and the Opus family are not in it. Always-on thinking is
    /// the wider Fable/Mythos property.
    #[test]
    fn the_preserved_thinking_generation_is_fable_and_mythos_5_1() {
        for model in [
            "claude-fable-5-1",
            "claude-fable-5.1",
            "claude-fable-5-1[1m]",
            "claude-mythos-5-1",
            "claude-mythos-preview",
        ] {
            assert!(preserved_thinking_generation(model), "{model}");
            assert!(rejects_forced_tool_choice(model), "{model}");
        }
        for model in ["claude-fable-5", "claude-opus-5", "claude-sonnet-5", "gpt-5.6-sol"] {
            assert!(!preserved_thinking_generation(model), "{model}");
            assert!(!rejects_forced_tool_choice(model), "{model}");
        }
        for model in ["claude-fable-5", "claude-fable-5-1", "claude-mythos-5-1"] {
            assert!(thinking_always_on(model), "{model}");
        }
        for model in ["claude-opus-5", "claude-opus-4-8", "claude-sonnet-5", "gpt-5.6-sol"] {
            assert!(!thinking_always_on(model), "{model}");
        }
    }
}


#[cfg(test)]
mod plan_priors_tests {
    use super::{builtin_model_context_catalog, PlanPriorsTable, RouterPriors, WorkTurnPrior};

    #[test]
    fn the_shipped_plan_priors_name_every_complexity_and_every_fixed_cost() {
        let plan = &builtin_model_context_catalog().priors.plan;
        for label in ["trivial", "small", "medium", "large", "unknown"] {
            let turn = plan.work_turn(label).unwrap_or_else(|| panic!("{label} missing"));
            assert!(turn.output_tokens > 0 && turn.requests > 0 && turn.duration_ms > 0, "{label}");
        }
        assert!(plan.pass_percent_top >= plan.pass_percent_second);
        assert!(plan.pass_percent_second >= plan.pass_percent_rest);
        assert!(plan.pass_percent_top <= 100);
        assert!(plan.judge_output_tokens > 0 && plan.classify_output_tokens > 0);
        assert!(plan.lane_brief_tokens > 0 && plan.handover_tokens > 0);
        assert!(plan.declares_anything());
        assert!(!PlanPriorsTable::default().declares_anything());
    }

    #[test]
    fn a_published_plan_layer_fills_only_what_it_leaves_empty() {
        let shipped = &builtin_model_context_catalog().priors;
        let mut published = RouterPriors::default();
        published.plan.pass_percent_top = 95;
        published.plan.by_complexity.insert(
            "medium".to_string(),
            WorkTurnPrior { output_tokens: 1, requests: 1, duration_ms: 1 },
        );
        assert!(published.declares_anything());
        published.fill_from(shipped);
        // The declared figures keep; the rest come from the shipped section.
        assert_eq!(published.plan.pass_percent_top, 95);
        assert_eq!(published.plan.pass_percent_rest, shipped.plan.pass_percent_rest);
        assert_eq!(published.plan.work_turn("medium").map(|turn| turn.requests), Some(1));
        assert_eq!(published.plan.work_turn("large"), shipped.plan.work_turn("large"));
        assert_eq!(published.small_tokens, shipped.small_tokens);
    }
}
