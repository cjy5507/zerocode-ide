//! The exact launch guard (design t-2516 §3, slice C).
//!
//! Opt-in. Under the contract the requested agent, model and effort are
//! validated against the catalog facts this process actually published —
//! BEFORE the trust gate, before the session opens, before any task input can
//! run — and the launch either happens exactly as requested or fails by name.
//! Nothing here clamps an effort, snaps a model to its nearest neighbour, or
//! switches a provider on the caller's behalf; the legacy policy that does
//! those things keeps doing them for a launch that never opened this door.
//!
//! The facts are the catalog's (`api`, `model_wire_env`), reached through
//! [`CatalogFacts`] so the validation table can be pinned against a fixture
//! without reading a machine's `~/.zo`. No model name is spelled here.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::effort::{effort_level_label, Effort};
use crate::ide::channel::capabilities::{public_token, Contract, Requested};

/* ---- the one table: doors, numbers, reasons -------------------------------
 *
 * | door                       | spelling                                   |
 * |----------------------------|--------------------------------------------|
 * | command line               | `--launch-contract <version>`              |
 * | environment                | `ZO_LAUNCH_CONTRACT=<version>`             |
 * | settings.json              | `"launchContract": <version>`              |
 * | request envelope (env)     | `ZO_LAUNCH_REQUEST={json}` ≤ 4096 bytes    |
 *
 * | number         | value | meaning                                          |
 * |----------------|-------|--------------------------------------------------|
 * | VERSION        | 1     | the guard this binary implements                 |
 * | EXIT_CODE      | 4     | a refused launch (0 done · 1 error · 2 limit · 3 interrupted are taken) |
 * | CHANNEL_HOLD   | 5 s   | how long a refused hosted launch keeps its channel so the host can read the verdict |
 *
 * Reason codes are the design's; each carries one sentence. */

pub const VERSION: u32 = 1;
pub const FLAG: &str = "--launch-contract";
pub const ENV: &str = "ZO_LAUNCH_CONTRACT";
pub const SETTINGS_KEY: &str = "launchContract";
pub const REQUEST_ENV: &str = "ZO_LAUNCH_REQUEST";
pub const REQUEST_MAX_BYTES: usize = 4096;
pub const EXIT_CODE: u8 = 4;
pub const CHANNEL_HOLD: Duration = Duration::from_secs(5);

pub const AGENT: &str = "zo";

pub const REASONS: &[(&str, &str)] = &[
    ("agent-mismatch", "the launch request names another agent; this binary is zo"),
    ("conflicting-selection", "the launch request and the command line disagree about the selection"),
    ("unsupported-model", "the catalog has no row for the requested model; nothing is snapped to a neighbour"),
    ("unsupported-effort", "the requested effort is not a level the model's catalog row declares; nothing is clamped"),
    ("catalog-facts-unavailable", "the model's catalog row declares no effort facts, so the requested effort cannot be validated"),
    ("catalog-revision-changed", "the catalog revision the request pinned is not the one this process published"),
    ("effective-mismatch", "the opened session does not carry the accepted selection"),
    ("exact-launch-unavailable", "this binary does not implement the requested launch contract version"),
];

/// The sentence a reason code carries.
#[must_use]
pub fn sentence(reason: &str) -> &'static str {
    REASONS.iter().find(|(code, _)| *code == reason).map_or("", |(_, words)| words)
}

/* ---- the door ---------------------------------------------------------- */

/// Which contract version was asked for: the flag, else the environment,
/// else the settings file. `Ok(None)` is nobody asking — the legacy policy.
/// `0`/`off` closes a door explicitly, so a flag can override a settings
/// default. A value that is not a version is a usage error, not a refusal.
pub fn requested_version(
    flag: Option<&str>,
    env: Option<&str>,
    settings: &Path,
) -> Result<Option<u32>, String> {
    if let Some(raw) = flag {
        return parse_version(raw, FLAG);
    }
    if let Some(raw) = env.filter(|raw| !raw.trim().is_empty()) {
        return parse_version(raw, ENV);
    }
    let stored = std::fs::read_to_string(settings)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get(SETTINGS_KEY).cloned());
    match stored {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => Ok(number.as_u64().filter(|n| *n > 0).map(|n| u32::try_from(n).unwrap_or(u32::MAX))),
        Some(Value::Bool(on)) => Ok(on.then_some(VERSION)),
        Some(Value::String(raw)) => parse_version(&raw, SETTINGS_KEY),
        Some(_) => Err(format!("settings `{SETTINGS_KEY}` must be a version number")),
    }
}

fn parse_version(raw: &str, door: &str) -> Result<Option<u32>, String> {
    let trimmed = raw.trim();
    if matches!(trimmed.to_ascii_lowercase().as_str(), "0" | "off" | "false" | "none") {
        return Ok(None);
    }
    trimmed
        .parse::<u32>()
        .ok()
        .filter(|version| *version > 0)
        .map(Some)
        .ok_or_else(|| format!("{door} needs a version number ({VERSION}); got '{trimmed}'"))
}

/* ---- the request envelope ---------------------------------------------- */

/// What the host said it was launching, as public selection context only —
/// no prompt, token, auth path or account label rides here. Every field is
/// optional; an absent envelope constrains nothing beyond the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub request_id: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub provider: Option<String>,
    pub catalog_revision: Option<String>,
}

impl Request {
    /// Parse the envelope. Unknown fields are ignored; a field that is not a
    /// bounded public token is dropped rather than copied into diagnostics.
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
            return Ok(Self::default());
        };
        if raw.len() > REQUEST_MAX_BYTES {
            return Err(format!("{REQUEST_ENV} is larger than {REQUEST_MAX_BYTES} bytes"));
        }
        let value: Value = serde_json::from_str(raw)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| format!("{REQUEST_ENV} is not a JSON object"))?;
        let field = |name: &str| value.get(name).and_then(Value::as_str).and_then(public_token);
        Ok(Self {
            request_id: field("request_id"),
            agent: field("agent"),
            model: field("model"),
            effort: field("effort"),
            provider: field("provider"),
            catalog_revision: field("catalog_revision"),
        })
    }

    pub fn from_env() -> Result<Self, String> {
        Self::parse(std::env::var(REQUEST_ENV).ok().as_deref())
    }
}

/* ---- the selection and the facts --------------------------------------- */

/// The command line's selection: the raw spelling beside the resolved id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection<'a> {
    pub requested_model: Option<&'a str>,
    /// After alias resolution against the published catalog.
    pub model: &'a str,
    pub requested_effort: Option<&'a str>,
    pub effort: Option<Effort>,
}

/// The catalog facts validation reads. One implementation reads what this
/// process published; the tests read a fixture.
pub trait CatalogFacts {
    fn resolve(&self, model: &str) -> String;
    fn known(&self, model: &str) -> bool;
    fn provider(&self, model: &str) -> &'static str;
    fn effort_levels(&self, model: &str) -> Option<Vec<api::EffortLevel>>;
    fn revision(&self, model: &str) -> Option<String>;
}

/// The catalog this process published (`cli_args::resolve_model_alias`
/// publishes before the parser resolves the first alias).
pub struct PublishedCatalog;

impl CatalogFacts for PublishedCatalog {
    fn resolve(&self, model: &str) -> String {
        crate::cli_args::resolve_model_alias(model)
    }
    fn known(&self, model: &str) -> bool {
        // Publishing is idempotent; a launch without `--model` reaches here
        // before anything else asked the catalog to publish.
        let _ = crate::runtime_support::publish_model_catalog();
        crate::model_wire_env::selection_provenance(model).0 != "unknown"
    }
    fn provider(&self, model: &str) -> &'static str {
        crate::ide::channel::capabilities::provider_name(api::detect_provider_kind(model))
    }
    fn effort_levels(&self, model: &str) -> Option<Vec<api::EffortLevel>> {
        api::declared_effort_levels(model)
    }
    fn revision(&self, model: &str) -> Option<String> {
        crate::ide::channel::capabilities::catalog_revision(model)
    }
}

/* ---- the verdict ------------------------------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: &'static str,
    pub detail: String,
}

impl Refusal {
    fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Self { reason, detail: detail.into() }
    }

    /// The one line a person or a log reads.
    #[must_use]
    pub fn line(&self) -> String {
        format!("zo: launch refused ({}) — {}: {}", self.reason, sentence(self.reason), self.detail)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// The resolved model id, frozen for this launch.
    pub model: String,
    /// The requested level's canonical spelling, when one was requested.
    pub effort: Option<&'static str>,
    pub provider: &'static str,
}

/// Validate one launch. Every refusal names its reason; nothing is retried
/// with a weaker selection.
pub fn validate(
    request: &Request,
    selection: &Selection<'_>,
    facts: &dyn CatalogFacts,
) -> Result<Accepted, Refusal> {
    if let Some(agent) = request.agent.as_deref().filter(|agent| !agent.eq_ignore_ascii_case(AGENT)) {
        return Err(Refusal::new("agent-mismatch", format!("the request names '{agent}'")));
    }
    match (request.model.as_deref(), selection.requested_model) {
        (Some(asked), Some(given))
            if !asked.eq_ignore_ascii_case(given) && facts.resolve(asked) != selection.model =>
        {
            return Err(Refusal::new(
                "conflicting-selection",
                format!("the request names model '{asked}', the command line '{given}'"),
            ));
        }
        (Some(asked), None) => {
            return Err(Refusal::new(
                "conflicting-selection",
                format!("the request names model '{asked}' and the command line gives none"),
            ));
        }
        _ => {}
    }
    match (request.effort.as_deref(), selection.requested_effort) {
        (Some(asked), Some(given)) if !asked.eq_ignore_ascii_case(given) => {
            return Err(Refusal::new(
                "conflicting-selection",
                format!("the request names effort '{asked}', the command line '{given}'"),
            ));
        }
        (Some(asked), None) => {
            return Err(Refusal::new(
                "conflicting-selection",
                format!("the request names effort '{asked}' and the command line gives none"),
            ));
        }
        _ => {}
    }
    let model = selection.model;
    if !facts.known(model) {
        let spelled = selection
            .requested_model
            .filter(|raw| *raw != model)
            .map(|raw| format!(" (requested as '{raw}')"))
            .unwrap_or_default();
        return Err(Refusal::new("unsupported-model", format!("'{model}' has no catalog row{spelled}")));
    }
    let provider = facts.provider(model);
    if let Some(expected) = request.provider.as_deref().filter(|expected| !expected.eq_ignore_ascii_case(provider)) {
        return Err(Refusal::new(
            "effective-mismatch",
            format!("the request expects provider '{expected}'; '{model}' is served by '{provider}'"),
        ));
    }
    let effort = match selection.effort {
        None => None,
        Some(policy @ (Effort::Smart | Effort::Off)) => {
            return Err(Refusal::new(
                "unsupported-effort",
                format!("'{}' is a policy, not a fixed level; an exact launch needs a level the row declares", policy.canonical()),
            ));
        }
        Some(level) => {
            let wanted = level.level().ok_or_else(|| Refusal::new("unsupported-effort", "the level has no wire encoding"))?;
            let Some(declared) = facts.effort_levels(model) else {
                return Err(Refusal::new(
                    "catalog-facts-unavailable",
                    format!("'{model}' declares no effort levels; '{}' cannot be validated", level.canonical()),
                ));
            };
            if !declared.contains(&wanted) {
                let listed: Vec<&str> = declared.iter().copied().map(effort_level_label).collect();
                return Err(Refusal::new(
                    "unsupported-effort",
                    format!("'{}' is not declared for '{model}' (declared: {})", level.canonical(), listed.join(", ")),
                ));
            }
            Some(level.canonical())
        }
    };
    if let Some(pinned) = request.catalog_revision.as_deref() {
        let published = facts.revision(model);
        if published.as_deref() != Some(pinned) {
            return Err(Refusal::new(
                "catalog-revision-changed",
                format!("the request pinned catalog revision '{pinned}'; this process published '{}'", published.unwrap_or_else(|| "unknown".into())),
            ));
        }
    }
    Ok(Accepted { model: model.to_string(), effort, provider })
}

/// After the session opened: the accepted selection must be the one the
/// session carries. Preferences, resume metadata or a runtime rebuild that
/// re-chose it end the launch here, before a turn.
pub fn confirm_effective(accepted: &Accepted, model: &str, effort: Option<&str>) -> Result<(), Refusal> {
    if model != accepted.model {
        return Err(Refusal::new(
            "effective-mismatch",
            format!("accepted model '{}', the session opened '{model}'", accepted.model),
        ));
    }
    if let Some(wanted) = accepted.effort.filter(|wanted| Some(*wanted) != effort) {
        return Err(Refusal::new(
            "effective-mismatch",
            format!("accepted effort '{wanted}', the session opened '{}'", effort.unwrap_or("none")),
        ));
    }
    Ok(())
}

/// The receipt the capability snapshot carries, for either verdict.
#[must_use]
pub fn contract(request: &Request, requested: &Requested, outcome: &Result<Accepted, Refusal>) -> Contract {
    let requested = json!({
        "agent": AGENT,
        "model": requested.model.as_deref().and_then(public_token),
        "effort": requested.effort.as_deref().and_then(public_token),
    });
    match outcome {
        Ok(accepted) => Contract {
            version: VERSION,
            request_id: request.request_id.clone(),
            requested,
            effective: json!({"agent": AGENT, "provider": accepted.provider,
                "model": public_token(&accepted.model), "effort": accepted.effort}),
            verdict: "accepted",
            reason: None,
        },
        Err(refusal) => Contract {
            version: VERSION,
            request_id: request.request_id.clone(),
            requested,
            effective: Value::Null,
            verdict: "refused",
            reason: Some(refusal.reason),
        },
    }
}

/* ---- the guard, as main reads it --------------------------------------- */

/// A launch the contract accepted, with everything the snapshot needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guarded {
    pub request: Request,
    pub accepted: Accepted,
    pub contract: Contract,
}

/// A launch the contract refused: the request it answered, the receipt a
/// host reads, and the refusal itself. Boxed inside [`Failure`] so the
/// usage-error variant does not carry its weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub request: Request,
    pub contract: Contract,
    pub refusal: Refusal,
}

/// Why a guarded launch did not proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The door was misused (a bad version, an unreadable envelope): a usage
    /// error, exit 1, nothing validated.
    Usage(String),
    /// The contract refused the selection by name: exit [`EXIT_CODE`], the
    /// verdict published where a host can read it.
    Refused(Box<Refused>),
}

/// Decide the launch. `Ok(None)` is the legacy policy — no door was opened.
pub fn guard(flag: Option<&str>, requested: &Requested, selection: &Selection<'_>, facts: &dyn CatalogFacts) -> Result<Option<Guarded>, Failure> {
    let version = requested_version(
        flag,
        std::env::var(ENV).ok().as_deref(),
        &runtime::default_config_home().join("settings.json"),
    )
    .map_err(Failure::Usage)?;
    let Some(version) = version else {
        return Ok(None);
    };
    if version != VERSION {
        return Err(Failure::Usage(format!(
            "exact-launch-unavailable: {} (asked for {version}, this zo implements {VERSION})",
            sentence("exact-launch-unavailable")
        )));
    }
    let request = Request::from_env().map_err(Failure::Usage)?;
    let outcome = validate(&request, selection, facts);
    let contract = contract(&request, requested, &outcome);
    match outcome {
        Ok(accepted) => Ok(Some(Guarded { request, accepted, contract })),
        Err(refusal) => Err(Failure::Refused(Box::new(Refused { request, contract, refusal }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::EffortLevel as L;

    /// Fixture facts: one Anthropic-shaped row with a ceiling below `ultra`,
    /// one OpenAI-shaped row with every rung, one row declaring nothing, and
    /// an alias onto the first. Placeholder ids, not a shipped ranking.
    struct Fixture;
    impl CatalogFacts for Fixture {
        fn resolve(&self, model: &str) -> String {
            if model == "fixture-alias" { "fixture-model-v2".into() } else { model.into() }
        }
        fn known(&self, model: &str) -> bool {
            ["fixture-model-v2", "fixture-openai", "fixture-bare"].contains(&model)
        }
        fn provider(&self, model: &str) -> &'static str {
            if model == "fixture-openai" { "openai" } else { "anthropic" }
        }
        fn effort_levels(&self, model: &str) -> Option<Vec<L>> {
            match model {
                "fixture-model-v2" => Some(vec![L::Low, L::Medium, L::High, L::Xhigh, L::Max]),
                "fixture-openai" => Some(vec![L::Low, L::Medium, L::High, L::Xhigh, L::Max, L::Ultra]),
                _ => None,
            }
        }
        fn revision(&self, _model: &str) -> Option<String> {
            Some("rev-1".into())
        }
    }

    fn selection<'a>(raw_model: Option<&'a str>, model: &'a str, raw_effort: Option<&'a str>) -> Selection<'a> {
        Selection { requested_model: raw_model, model, requested_effort: raw_effort, effort: raw_effort.and_then(Effort::from_token) }
    }

    /// One row: its name, the request envelope, the command line's selection,
    /// and the expected verdict — `Ok((model, effort))` or `Err(reason)`.
    type Row<'a> = (&'a str, Request, Selection<'a>, Result<(&'a str, Option<&'a str>), &'a str>);

    #[test]
    fn the_validation_table() {
        let plain = Request::default();
        let rows: Vec<Row<'_>> = vec![
            ("known alias resolves and is kept", plain.clone(), selection(Some("fixture-alias"), "fixture-model-v2", Some("high")), Ok(("fixture-model-v2", Some("high")))),
            ("literal id with no effort", plain.clone(), selection(Some("fixture-model-v2"), "fixture-model-v2", None), Ok(("fixture-model-v2", None))),
            ("unknown model is never snapped", plain.clone(), selection(Some("fixture-modle-v2"), "fixture-modle-v2", Some("high")), Err("unsupported-model")),
            ("effort the row does not declare", plain.clone(), selection(Some("fixture-alias"), "fixture-model-v2", Some("ultra")), Err("unsupported-effort")),
            ("row with every rung takes the top one", plain.clone(), selection(Some("fixture-openai"), "fixture-openai", Some("ultra")), Ok(("fixture-openai", Some("ultra")))),
            ("row without effort facts", plain.clone(), selection(Some("fixture-bare"), "fixture-bare", Some("high")), Err("catalog-facts-unavailable")),
            ("smart is a policy, not a level", plain.clone(), selection(Some("fixture-alias"), "fixture-model-v2", Some("smart")), Err("unsupported-effort")),
            ("off is a policy, not a level", plain.clone(), selection(Some("fixture-alias"), "fixture-model-v2", Some("off")), Err("unsupported-effort")),
            ("provider mismatch", Request { provider: Some("openai".into()), ..plain.clone() }, selection(Some("fixture-alias"), "fixture-model-v2", Some("high")), Err("effective-mismatch")),
            ("provider agreement", Request { provider: Some("openai".into()), ..plain.clone() }, selection(Some("fixture-openai"), "fixture-openai", Some("high")), Ok(("fixture-openai", Some("high")))),
            ("agent mismatch", Request { agent: Some("codex".into()), ..plain.clone() }, selection(Some("fixture-alias"), "fixture-model-v2", None), Err("agent-mismatch")),
            ("request model agrees by resolution", Request { model: Some("fixture-alias".into()), ..plain.clone() }, selection(Some("fixture-model-v2"), "fixture-model-v2", None), Ok(("fixture-model-v2", None))),
            ("request model conflicts with argv", Request { model: Some("fixture-openai".into()), ..plain.clone() }, selection(Some("fixture-alias"), "fixture-model-v2", None), Err("conflicting-selection")),
            ("request effort with none on argv", Request { effort: Some("high".into()), ..plain.clone() }, selection(Some("fixture-alias"), "fixture-model-v2", None), Err("conflicting-selection")),
            ("pinned revision moved", Request { catalog_revision: Some("rev-0".into()), ..plain.clone() }, selection(Some("fixture-alias"), "fixture-model-v2", Some("high")), Err("catalog-revision-changed")),
            ("pinned revision holds", Request { catalog_revision: Some("rev-1".into()), ..plain.clone() }, selection(Some("fixture-alias"), "fixture-model-v2", Some("high")), Ok(("fixture-model-v2", Some("high")))),
        ];
        for (name, request, selection, expected) in rows {
            let actual = validate(&request, &selection, &Fixture)
                .map(|accepted| (accepted.model, accepted.effort))
                .map_err(|refusal| refusal.reason);
            let expected = expected.map(|(model, effort)| (model.to_string(), effort));
            assert_eq!(actual, expected, "{name}");
        }
    }

    #[test]
    fn every_refusal_reason_is_in_the_table_and_carries_a_sentence() {
        for (code, words) in REASONS {
            assert!(!words.is_empty(), "{code}");
            assert_eq!(sentence(code), *words);
        }
        let refusal = validate(&Request::default(), &selection(Some("fixture-alias"), "fixture-model-v2", Some("ultra")), &Fixture).unwrap_err();
        assert!(REASONS.iter().any(|(code, _)| *code == refusal.reason));
        let line = refusal.line();
        assert!(line.contains("unsupported-effort") && line.contains("ultra") && line.contains("fixture-model-v2"), "{line}");
        assert!(line.contains("low, medium, high, xhigh, max"), "the declared levels are named: {line}");
    }

    #[test]
    fn the_receipt_names_both_verdicts_and_never_the_provider_wire() {
        let request = Request { request_id: Some("req-1".into()), ..Request::default() };
        let requested = Requested { model: Some("fixture-alias".into()), effort: Some("high".into()) };
        let accepted = contract(&request, &requested, &validate(&request, &selection(Some("fixture-alias"), "fixture-model-v2", Some("high")), &Fixture));
        assert_eq!(accepted.verdict, "accepted");
        assert_eq!(accepted.reason, None);
        assert_eq!(accepted.requested, json!({"agent":"zo","model":"fixture-alias","effort":"high"}));
        assert_eq!(accepted.effective, json!({"agent":"zo","provider":"anthropic","model":"fixture-model-v2","effort":"high"}));
        let refused = contract(&request, &requested, &validate(&request, &selection(Some("fixture-alias"), "fixture-model-v2", Some("ultra")), &Fixture));
        assert_eq!((refused.verdict, refused.reason), ("refused", Some("unsupported-effort")));
        assert!(refused.effective.is_null());
        assert_eq!(refused.request_id.as_deref(), Some("req-1"));
        assert_eq!(refused.version, VERSION);
    }

    #[test]
    fn the_effective_selection_must_match_what_was_accepted() {
        let accepted = Accepted { model: "fixture-model-v2".into(), effort: Some("high"), provider: "anthropic" };
        assert!(confirm_effective(&accepted, "fixture-model-v2", Some("high")).is_ok());
        assert_eq!(confirm_effective(&accepted, "fixture-preferred", Some("high")).unwrap_err().reason, "effective-mismatch");
        assert_eq!(confirm_effective(&accepted, "fixture-model-v2", Some("medium")).unwrap_err().reason, "effective-mismatch");
        let unpinned = Accepted { effort: None, ..accepted };
        assert!(confirm_effective(&unpinned, "fixture-model-v2", Some("medium")).is_ok());
    }

    #[test]
    fn the_doors_are_one_table_flag_over_env_over_settings_and_zero_closes() {
        let home = tempfile::tempdir().unwrap();
        let settings = home.path().join("settings.json");
        assert_eq!(requested_version(None, None, &settings), Ok(None));
        assert_eq!(requested_version(Some("1"), None, &settings), Ok(Some(1)));
        assert_eq!(requested_version(None, Some("1"), &settings), Ok(Some(1)));
        std::fs::write(&settings, br#"{"launchContract": 1}"#).unwrap();
        assert_eq!(requested_version(None, None, &settings), Ok(Some(1)));
        assert_eq!(requested_version(Some("0"), Some("1"), &settings), Ok(None), "the flag closes what settings opened");
        assert_eq!(requested_version(None, Some("off"), &settings), Ok(None));
        assert_eq!(requested_version(Some("2"), None, &settings), Ok(Some(2)));
        assert!(requested_version(Some("soon"), None, &settings).is_err());
        std::fs::write(&settings, br#"{"launchContract": true}"#).unwrap();
        assert_eq!(requested_version(None, None, &settings), Ok(Some(VERSION)));
        std::fs::write(&settings, br#"{"launchContract": {"no":"object"}}"#).unwrap();
        assert!(requested_version(None, None, &settings).is_err());
    }

    #[test]
    fn the_request_envelope_is_bounded_and_drops_what_is_not_a_public_token() {
        assert_eq!(Request::parse(None), Ok(Request::default()));
        assert_eq!(Request::parse(Some("   ")), Ok(Request::default()));
        let parsed = Request::parse(Some(r#"{"request_id":"req-9","agent":"zo","model":"fixture-alias","effort":"high","provider":"https://canary.example/x","secret":"CANARY","catalog_revision":"rev-1"}"#)).unwrap();
        assert_eq!(parsed.request_id.as_deref(), Some("req-9"));
        assert_eq!(parsed.model.as_deref(), Some("fixture-alias"));
        assert_eq!(parsed.provider, None, "a URL is not a provider token");
        assert_eq!(parsed.catalog_revision.as_deref(), Some("rev-1"));
        assert!(Request::parse(Some("[1,2]")).is_err());
        assert!(Request::parse(Some("not json")).is_err());
        let huge = format!("{{\"model\":\"{}\"}}", "m".repeat(REQUEST_MAX_BYTES));
        assert!(Request::parse(Some(&huge)).is_err());
    }
}
