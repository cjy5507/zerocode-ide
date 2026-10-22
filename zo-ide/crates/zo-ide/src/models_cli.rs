//! `zo models` — the live model catalog, on one screen.
//!
//! Three layers are shown as one table: the rows this binary shipped with,
//! the rows the person's overlay declares (`model-catalog.json`, or the legacy
//! settings key), the rows the connected providers serve today (discovery),
//! and where each family alias points after the update policy has been
//! applied. This is the surface for checking that a release the provider
//! shipped this morning is selectable this afternoon — and for seeing why it
//! is not. `--refresh` also leaves the merged catalog beside the cache, every
//! layer in precedence order, for the "why" that needs the bytes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use runtime::model_catalog::{CatalogProvider, CatalogRow, ModelCatalog, OverlaySource};
use runtime::model_discovery::{self, DiscoveredCatalog, Overlay, UpdatePolicy};
use runtime::ModelTierAssignment;
use serde_json::json;

use crate::model_wire_env;

const ALL_PROVIDERS: [CatalogProvider; 3] = [
    CatalogProvider::Anthropic,
    CatalogProvider::Openai,
    CatalogProvider::Google,
];

/// Render the catalog. `refresh` asks every source now; otherwise the
/// connection rule applies inline, since the person is waiting for the
/// answer — a source whose answer is missing, failed or older than the TTL
/// is asked again, the rest are trusted.
pub fn render(refresh: bool, json: bool) -> Result<String, String> {
    let policy = UpdatePolicy::load();
    let now = model_discovery::now_secs();
    let ttl = model_discovery::ttl_secs();
    let fresh = if refresh {
        Some(model_discovery::discover())
    } else {
        model_discovery::discover_due(now, ttl)
    };
    let catalog: DiscoveredCatalog = match fresh {
        Some(fresh) => {
            model_discovery::install(fresh.clone())
                .map_err(|error| format!("could not write the discovery cache: {error}"))?;
            fresh
        }
        None => model_discovery::current().as_deref().cloned().unwrap_or_default(),
    };
    // Make the answer live in this process so the alias column shows what a
    // session launched now would resolve — the connected providers included.
    if let Ok(cwd) = std::env::current_dir() {
        crate::runtime_support::publish_custom_providers_from_settings(&cwd);
    }
    let published = crate::runtime_support::publish_model_catalog();
    // A refresh leaves the merged catalog beside the cache — every layer in
    // precedence order, the shipped seed last — so "why does this alias
    // resolve there" has a file to read.
    let merged = refresh
        .then(|| {
            let path = model_wire_env::audit_path();
            let document = model_wire_env::audit_document(published.as_ref(), now)?;
            model_wire_env::write_audit(&path, &document)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            Ok::<PathBuf, String>(path)
        })
        .transpose()?;
    let overlay = model_discovery::overlay(&catalog, policy);
    let user = ModelCatalog::load().map_err(|error| format!("model catalog: {error}"))?;
    let rows = user.rows(&ALL_PROVIDERS, false);
    // Classified after the publish above, so discovered ids are ranked
    // beside the shipped ones.
    let listed: Vec<(api::ProviderKind, &str)> =
        rows.iter().map(|row| (row.provider.kind(), row.id.as_str())).collect();
    let tiers = judged_tiers(&listed, now);
    let report = Report {
        policy,
        catalog: &catalog,
        user: &user,
        rows: &rows,
        overlay: &overlay,
        merged: merged.as_deref(),
        tiers: &tiers,
        now,
    };
    if json {
        report.json()
    } else {
        Ok(report.text())
    }
}

/// The classifier's answer over `listed`, with the tier seat's judgment
/// applied where one stands (t-45961): each provider's lineup — its models
/// in the fact rule's order, superseded releases left out — is asked when its
/// facts or its record moved and answered from the kept file otherwise; the
/// lineups due a question are asked together under one wall, and one whose
/// question went out unanswered (HTTP 402, a timeout) is not asked again for
/// the same facts within the seat's memory. Under a seat that asks nothing
/// (`off`, or Jev switched off) this is the fact rule, and no record is read.
fn judged_tiers(listed: &[(api::ProviderKind, &str)], now_secs: u64) -> Vec<ModelTierAssignment> {
    let descriptors = runtime::tier_descriptors_of(listed);
    let by_facts = runtime::classify_model_tiers(&descriptors);
    let cwd = std::env::current_dir().unwrap_or_default();
    if !tools::tier_seat_mode(&cwd).asks() {
        return by_facts;
    }
    let lineups = runtime::tier_lineups_of(&descriptors, &by_facts);
    let outcomes = runtime::tier_outcomes_of(&runtime::read_route_outcomes_across_projects());
    let now_ms = i64::try_from(std::time::Duration::from_secs(now_secs).as_millis()).unwrap_or(i64::MAX);
    let judgment = tools::tier_judgment_for(&cwd, &lineups, &|id: &str| outcomes.get(id).copied(), now_ms);
    runtime::classify_model_tiers_judged(&descriptors, Some(&judgment))
}

/// One answer, rendered two ways.
struct Report<'a> {
    policy: UpdatePolicy,
    catalog: &'a DiscoveredCatalog,
    user: &'a ModelCatalog,
    rows: &'a [CatalogRow],
    overlay: &'a Overlay,
    /// Where `--refresh` left the merged copy.
    merged: Option<&'a Path>,
    /// The tier classifier's answer over the published catalog.
    tiers: &'a [ModelTierAssignment],
    now: u64,
}

impl Report<'_> {
    fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&json!({
            "policy": self.policy.key(),
            "fetchedAt": self.catalog.fetched_at,
            "cache": model_discovery::cache_path(),
            "overlay": {
                "source": self.user.source().label(),
                "path": self.user.catalog_path(),
                "settingsKeySuperseded": self.user.settings_key_superseded(),
            },
            "merged": self.merged,
            "reports": self.catalog.reports.iter().map(|report| json!({
                "provider": report.provider,
                "source": report.source,
                "ok": report.ok,
                "detail": report.detail,
                "count": report.count,
                "fetchedAt": report.fetched_at,
                "age": describe_age(report.fetched_at, self.now),
                "origin": report.origin,
            })).collect::<Vec<_>>(),
            "models": self.rows.iter().map(|row| json!({
                "provider": row.provider.key(),
                "id": row.id,
                "displayName": row.display_name,
                "builtin": row.builtin,
                "discovered": row.discovered,
                "from": self.row_source(row),
                "authRoute": row.auth_route,
                "unlistedSince": self.unlisted_since(row),
                "band": self.tier(row).map(|tier| tier.band.key()),
                "rungs": self.tier(row).map(|tier| tier.rungs.iter().map(|rung| rung.key()).collect::<Vec<_>>()),
                // Who placed the model in its band — the Jev seat's judgment,
                // the fact rule, or the facts for a model that arrived after
                // its provider's judgment — and how sure the judgment was, in
                // thousandths (t-45961).
                "tierFrom": self.tier(row).map(|tier| tier.source.key()),
                "tierConfidencePermille": self.tier(row).and_then(|tier| tier.confidence_permille),
                // The newer release of its line a superseded row gives way
                // to — what the window resumes a session on instead of the
                // older one (t-46882).
                "supersededBy": self.tier(row).and_then(|tier| tier.successor.as_deref()),
                "efforts": api::accepted_efforts(&row.id)
                    .map(|levels| levels.iter().map(|level| level.key()).collect::<Vec<_>>()),
            })).collect::<Vec<_>>(),
            "otherLogins": self.other_logins().iter().map(|model| json!({
                "provider": model.provider,
                "id": model.id,
                "displayName": model.display_name,
                "source": model.source,
            })).collect::<Vec<_>>(),
            "customProviders": api::custom_provider_usability_catalog()
                .into_iter()
                .map(|provider| serde_json::json!({
                    "name": provider.name,
                    "models": provider
                        .models
                        .iter()
                        .map(|model| api::format_provider_model_ref(provider.name, model))
                        .collect::<Vec<_>>(),
                    "usable": provider.usable,
                    "credentialEnvVars": provider.credential_env_vars,
                }))
                .collect::<Vec<_>>(),
            "aliases": alias_rows(),
            "aliasUpdates": self.overlay.alias_updates,
            "aliasCandidates": self.overlay.alias_candidates,
            "aliasesWithoutRelease": self.overlay.orphaned,
        }))
        .map_err(|error| error.to_string())
    }

    fn text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "model catalog · policy {} · cache {}",
            self.policy.key(),
            describe_age(self.catalog.fetched_at, self.now)
        );
        let _ = writeln!(out, "{}", self.overlay_line());
        if self.user.settings_key_superseded() {
            let _ = writeln!(
                out,
                "  settings.json still carries modelCatalog, which is no longer read — remove it"
            );
        }
        if let Some(merged) = self.merged {
            let _ = writeln!(out, "merged · {}", merged.display());
        }
        for report in &self.catalog.reports {
            let _ = writeln!(out, "  {}", report_line(report, self.now));
        }
        out.push('\n');
        let (provider, model, name) = ("provider", "model", "name");
        let _ = writeln!(out, "{provider:<10} {model:<34} {name:<26} from");
        for row in self.rows {
            let route = match row.auth_route {
                api::AuthRoute::ApiKey => " (api key)",
                api::AuthRoute::OAuth | api::AuthRoute::Auto => "",
            };
            let unlisted = self
                .login_word(row)
                .or_else(|| {
                    self.unlisted_since(row)
                        .map(|since| crate::tui::strings::unlisted_since(&crate::tui::models::local_clock(since)))
                })
                .map(|word| format!(" · {word}"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "{:<10} {:<34} {:<26} {}{}{}",
                row.provider.key(),
                row.id,
                row.display_name,
                self.row_source(row),
                route,
                unlisted
            );
        }
        // What a login zo lists but does not route through this catalog says
        // it serves (xAI's list for a Grok login, Kimi Code's catalog) —
        // shown so the login's models are seen, and named by the source that
        // listed them (t-6248).
        for model in self.other_logins() {
            let _ = writeln!(
                out,
                "{:<10} {:<34} {:<26} discovered · {}",
                model.provider, model.id, model.display_name, model.source
            );
        }
        // The providers the person connected (settings → API 라우터, or
        // `/connect`), each model as the `<provider>/<model>` a pick uses. One
        // whose key zo cannot see says which variable it is waiting for.
        for provider in api::custom_provider_usability_catalog() {
            let waiting = if provider.usable {
                String::new()
            } else {
                format!(" · key missing: {}", provider.credential_env_vars.join(" or "))
            };
            for model in &provider.models {
                let _ = writeln!(
                    out,
                    "{:<10} {:<34} {:<26} providers{}",
                    provider.name,
                    api::format_provider_model_ref(provider.name, model),
                    "",
                    waiting
                );
            }
        }
        out.push('\n');
        let (alias, model) = ("alias", "model");
        let _ = writeln!(out, "{alias:<22} → {model}");
        for (alias, canonical, provider) in alias_rows_flat() {
            let moved = self.alias_note(&alias);
            let _ = writeln!(out, "{alias:<22} → {canonical:<34} {provider}{moved}");
        }
        if !self.overlay.alias_candidates.is_empty() {
            out.push('\n');
            let _ = writeln!(out, "withheld by policy `notify` (set modelUpdatePolicy to auto to follow):");
            for update in &self.overlay.alias_candidates {
                let _ = writeln!(out, "  {:<22} {} → {}", update.alias, update.from, update.to);
            }
        }
        out.trim_end().to_string()
    }

    /// The Google login's word on a Gemini row that goes over it (t-71998),
    /// in the picker's words: its registry serves the release under another
    /// name, or nothing the login has serves it.
    fn login_word(&self, row: &CatalogRow) -> Option<String> {
        if row.provider != CatalogProvider::Google
            || row.auth_route == api::AuthRoute::ApiKey
            || self.policy == UpdatePolicy::Pinned
        {
            return None;
        }
        match model_discovery::antigravity_reach_in(self.catalog, &row.id) {
            model_discovery::AntigravityReach::Renamed(release) => {
                Some(crate::tui::strings::antigravity_renamed(&release))
            }
            model_discovery::AntigravityReach::Unserved { .. } => {
                Some(crate::tui::strings::ANTIGRAVITY_UNLISTED.to_string())
            }
            _ => None,
        }
    }

    /// When the provider's list stopped naming the row — a discovered row a
    /// session had selected, or a shipped row the provider withdrew (t-6248)
    /// — the row stays, and says so.
    fn unlisted_since(&self, row: &CatalogRow) -> Option<u64> {
        if row.builtin {
            return model_discovery::withdrawn_since_in(self.catalog, &row.id);
        }
        if !row.discovered {
            return None;
        }
        self.catalog
            .models
            .iter()
            .find(|model| model.id.eq_ignore_ascii_case(&row.id))
            .and_then(|model| model.unlisted_since)
    }

    /// What an alias line adds: a moved alias says where it came from; one
    /// minted for a family the shipped catalog never named has no "from" and
    /// says so; one whose release left its provider's list with nothing
    /// living to follow says there is none (t-6248).
    fn alias_note(&self, alias: &str) -> String {
        self.overlay
            .alias_updates
            .iter()
            .find(|update| update.alias == alias)
            .map(|update| {
                if update.from.is_empty() {
                    "   (new)".to_string()
                } else {
                    format!("   (was {})", update.from)
                }
            })
            .or_else(|| {
                self.overlay
                    .orphaned
                    .iter()
                    .find(|update| update.alias == alias)
                    .map(|update| format!("   (없음 · {} {})", update.from, crate::tui::strings::UNLISTED_SINCE))
            })
            .unwrap_or_default()
    }

    /// Discovered rows of a provider this catalog screen does not carry —
    /// the models another login lists (xAI, Kimi Code).
    fn other_logins(&self) -> Vec<&model_discovery::DiscoveredModel> {
        self.catalog
            .models
            .iter()
            .filter(|model| CatalogProvider::from_key(&model.provider).is_none())
            .collect()
    }

    /// Which layer put the row on the screen.
    fn tier(&self, row: &CatalogRow) -> Option<&ModelTierAssignment> {
        self.tiers.iter().find(|tier| tier.id.eq_ignore_ascii_case(&row.id))
    }

    fn row_source(&self, row: &CatalogRow) -> &'static str {
        if row.discovered {
            "discovered"
        } else if row.builtin {
            "shipped"
        } else {
            self.user.source().label()
        }
    }

    /// Where the overlay came from, and — for the legacy key — where the first
    /// edit will move it.
    fn overlay_line(&self) -> String {
        let label = self.user.source().label();
        let path = self.user.catalog_path().display();
        match self.user.source() {
            OverlaySource::File => format!("overlay · {label} · {path}"),
            OverlaySource::Settings => {
                format!("overlay · {label} · the first /model edit moves it to {path}")
            }
            OverlaySource::Undeclared => format!("overlay · {label} · {path} absent"),
        }
    }
}

/// One source's line: `provider source count · age · live|cache:<home>`,
/// with the failure (or the caveat beside a success) after it.
fn report_line(report: &model_discovery::SourceReport, now: u64) -> String {
    let age = describe_age(report.fetched_at, now);
    let origin = if report.origin.is_empty() { "-" } else { report.origin.as_str() };
    if report.ok {
        format!(
            "{:<10} {:<20} ok · {} model(s) · {age} · {origin}{}",
            report.provider,
            report.source,
            report.count,
            report
                .detail
                .split_once(" · ")
                .map(|(_, note)| format!(" · {note}"))
                .unwrap_or_default()
        )
    } else {
        format!(
            "{:<10} {:<20} {} · {age}{}",
            report.provider,
            report.source,
            report.detail,
            if report.count > 0 { format!(" · {origin}") } else { String::new() }
        )
    }
}

fn describe_age(fetched_at: u64, now: u64) -> String {
    if fetched_at == 0 {
        return "never fetched".to_string();
    }
    let age = now.saturating_sub(fetched_at);
    if age < 60 {
        format!("{age}s old")
    } else if age < 3600 {
        format!("{}m old", age / 60)
    } else if age < 86_400 {
        format!("{}h old", age / 3600)
    } else {
        format!("{}d old", age / 86_400)
    }
}

/// Family aliases — the short names a person types — with what they resolve
/// to now. Identity rows (an id naming itself) are noise here.
fn alias_rows_flat() -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = api::provider_catalog()
        .iter()
        .filter(|entry| !entry.alias.eq_ignore_ascii_case(entry.canonical_model_id))
        .map(|entry| {
            (
                entry.alias.to_string(),
                api::resolve_catalog_alias(entry.alias),
                provider_label(entry.provider).to_string(),
            )
        })
        .collect();
    rows.dedup_by(|left, right| left.0.eq_ignore_ascii_case(&right.0));
    rows
}

fn alias_rows() -> Vec<serde_json::Value> {
    alias_rows_flat()
        .into_iter()
        .map(|(alias, canonical, provider)| json!({"alias": alias, "canonical": canonical, "provider": provider}))
        .collect()
}

fn provider_label(kind: api::ProviderKind) -> &'static str {
    match kind {
        api::ProviderKind::Anthropic => "anthropic",
        api::ProviderKind::OpenAi => "openai",
        api::ProviderKind::Google => "google",
        api::ProviderKind::Xai => "xai",
        api::ProviderKind::Ollama => "ollama",
    }
}

#[cfg(test)]
mod tests {
    use super::{describe_age, report_line};

    /// (4) Every source gets one line: source, count, age, and whether the
    /// rows came live or from which cache — the failure or the caveat after.
    #[test]
    fn a_report_line_says_source_count_age_and_origin() {
        let report = |ok: bool, detail: &str, count: usize, origin: &str| runtime::model_discovery::SourceReport {
            provider: "openai".to_string(),
            source: runtime::model_discovery::OPENAI_SOURCE.to_string(),
            ok,
            detail: detail.to_string(),
            count,
            fetched_at: 1_000,
            origin: origin.to_string(),
        };
        let now = 1_000 + 2 * 3600;
        assert_eq!(
            report_line(&report(true, "9 model(s)", 9, "live"), now),
            "openai     chatgpt-backend      ok · 9 model(s) · 2h old · live"
        );
        assert_eq!(
            report_line(&report(true, "9 model(s) · live: HTTP 401", 9, "cache:/Users/x/.codex"), now),
            "openai     chatgpt-backend      ok · 9 model(s) · 2h old · cache:/Users/x/.codex · live: HTTP 401"
        );
        assert_eq!(
            report_line(&report(false, "http error · kept 9 model(s) from the previous refresh", 9, "previous"), now),
            "openai     chatgpt-backend      http error · kept 9 model(s) from the previous refresh · 2h old · previous"
        );
        assert_eq!(
            report_line(&report(false, "skipped: no ChatGPT login", 0, ""), now),
            "openai     chatgpt-backend      skipped: no ChatGPT login · 2h old"
        );
    }

    /// C4 (t-6248): a shipped row its provider withdrew says so on its own
    /// line — in the picker's words, with the local clock of the first list
    /// without it — and the family alias left with no living release says
    /// "없음" instead of quietly naming a dead model.
    #[test]
    fn a_withdrawn_shipped_row_and_its_orphaned_alias_say_so() {
        use runtime::model_catalog::ModelCatalog;
        use runtime::model_discovery::{AliasUpdate, DiscoveredCatalog, Overlay, UpdatePolicy, Withdrawn};
        let home = tempfile::tempdir().expect("an overlay home");
        let user = ModelCatalog::load_from_home(home.path()).expect("an empty overlay");
        let rows = user.rows(&super::ALL_PROVIDERS, false);
        let since = 1_790_121_000;
        let catalog = DiscoveredCatalog {
            fetched_at: since,
            withdrawn: vec![Withdrawn {
                provider: "openai".to_string(),
                id: "gpt-5.3-codex-spark".to_string(),
                source: runtime::model_discovery::OPENAI_SOURCE.to_string(),
                since,
            }],
            ..Default::default()
        };
        let overlay = Overlay {
            orphaned: vec![AliasUpdate {
                provider: "openai".to_string(),
                alias: "spark".to_string(),
                from: "gpt-5.3-codex-spark".to_string(),
                to: String::new(),
            }],
            ..Default::default()
        };
        let report = super::Report {
            policy: UpdatePolicy::Auto,
            catalog: &catalog,
            user: &user,
            rows: &rows,
            overlay: &overlay,
            merged: None,
            tiers: &[],
            now: since + 60,
        };
        let text = report.text();
        let mark = crate::tui::strings::unlisted_since(&crate::tui::models::local_clock(since));
        let spark_row = text
            .lines()
            .find(|line| line.starts_with("openai") && line.contains("gpt-5.3-codex-spark"))
            .expect("the shipped spark row stays");
        assert!(spark_row.ends_with(&format!("shipped · {mark}")), "{spark_row}");
        let sol_row = text
            .lines()
            .find(|line| line.starts_with("openai") && line.contains("gpt-5.6-sol "))
            .expect("the sol row");
        assert!(!sol_row.contains(crate::tui::strings::UNLISTED_SINCE), "{sol_row}");
        let alias = text
            .lines()
            .find(|line| line.starts_with("spark "))
            .expect("the spark alias line");
        assert!(alias.contains("(없음 · gpt-5.3-codex-spark 공급자 목록에서 빠짐)"), "{alias}");
        let json: serde_json::Value = serde_json::from_str(&report.json().expect("json")).unwrap();
        let spark = json["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "gpt-5.3-codex-spark")
            .expect("the spark row");
        assert_eq!(spark["unlistedSince"], since);
        assert_eq!(json["aliasesWithoutRelease"][0]["alias"], "spark");
    }

    /// C5 (t-6248): the models another login lists — xAI's for a Grok login
    /// — are seen in `zo models`, under the source that listed them, and kept
    /// out of the rows the window offers as picks.
    #[test]
    fn another_logins_models_are_listed_apart_from_the_catalog_rows() {
        use runtime::model_catalog::ModelCatalog;
        use runtime::model_discovery::{DiscoveredCatalog, DiscoveredModel, Overlay, UpdatePolicy};
        let home = tempfile::tempdir().expect("an overlay home");
        let user = ModelCatalog::load_from_home(home.path()).expect("an empty overlay");
        let rows = user.rows(&super::ALL_PROVIDERS, false);
        let catalog = DiscoveredCatalog {
            fetched_at: 1_790_121_000,
            models: vec![DiscoveredModel {
                provider: "xai".to_string(),
                id: "grok-4".to_string(),
                display_name: "grok-4".to_string(),
                source: runtime::model_discovery::XAI_SOURCE.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let overlay = Overlay::default();
        let report = super::Report {
            policy: UpdatePolicy::Auto,
            catalog: &catalog,
            user: &user,
            rows: &rows,
            overlay: &overlay,
            merged: None,
            tiers: &[],
            now: 1_790_121_060,
        };
        let text = report.text();
        let line = text.lines().find(|line| line.contains("grok-4")).expect("the xAI row is listed");
        assert!(line.starts_with("xai") && line.ends_with("discovered · xai-api"), "{line}");
        let json: serde_json::Value = serde_json::from_str(&report.json().expect("json")).unwrap();
        assert_eq!(json["otherLogins"][0]["id"], "grok-4");
        assert!(
            !json["models"].as_array().unwrap().iter().any(|row| row["id"] == "grok-4"),
            "not a pick the window offers"
        );
    }

    /// t-14437: every row carries what zo's tier classifier made of it over
    /// the whole catalog — the band, the rungs, who placed it and how sure
    /// that was (t-45961) — and the efforts it accepts: the facts the
    /// window's summons reads instead of keeping a model table of its own.
    #[test]
    fn every_row_carries_its_band_rungs_and_accepted_efforts() {
        use runtime::model_catalog::ModelCatalog;
        use runtime::model_discovery::{DiscoveredCatalog, Overlay, UpdatePolicy};
        let home = tempfile::tempdir().expect("an overlay home");
        let user = ModelCatalog::load_from_home(home.path()).expect("an empty overlay");
        let rows = user.rows(&super::ALL_PROVIDERS, false);
        let catalog = DiscoveredCatalog::default();
        let overlay = Overlay::default();
        let listed: Vec<(api::ProviderKind, &str)> =
            rows.iter().map(|row| (row.provider.kind(), row.id.as_str())).collect();
        let tiers = runtime::tier_assignments_of(&listed);
        let report = super::Report {
            policy: UpdatePolicy::Auto,
            catalog: &catalog,
            user: &user,
            rows: &rows,
            overlay: &overlay,
            merged: None,
            tiers: &tiers,
            now: 1_790_121_060,
        };
        let json: serde_json::Value = serde_json::from_str(&report.json().expect("json")).unwrap();
        let words = ["top", "second", "rest", "superseded"];
        let mut banded = 0;
        for row in json["models"].as_array().unwrap() {
            let id = row["id"].as_str().unwrap();
            let Some(tier) = tiers.iter().find(|tier| tier.id.eq_ignore_ascii_case(id)) else {
                assert!(row["band"].is_null(), "{id}: no band the classifier did not give");
                continue;
            };
            banded += 1;
            assert_eq!(row["band"], tier.band.key(), "{id}");
            assert!(words.contains(&row["band"].as_str().unwrap()), "{id}");
            let rungs: Vec<&str> = tier.rungs.iter().map(|rung| rung.key()).collect();
            assert_eq!(row["rungs"], serde_json::json!(rungs), "{id}");
            // t-45961: and who placed it there, and how sure that was.
            assert_eq!(row["tierFrom"], tier.source.key(), "{id}");
            assert_eq!(row["tierConfidencePermille"], serde_json::json!(tier.confidence_permille), "{id}");
            // t-46882: and, for a superseded release, the release that
            // stands in its place.
            assert_eq!(row["supersededBy"], serde_json::json!(tier.successor), "{id}");
            match api::accepted_efforts(id) {
                Some(levels) => {
                    let words: Vec<&str> = levels.iter().map(|level| level.key()).collect();
                    assert_eq!(row["efforts"], serde_json::json!(words), "{id}");
                }
                None => assert!(row["efforts"].is_null(), "{id}"),
            }
        }
        assert!(banded > 0, "the shipped rows are classified");
    }

    #[test]
    fn ages_read_like_a_person_would_say_them() {
        assert_eq!(describe_age(0, 10), "never fetched");
        assert_eq!(describe_age(100, 130), "30s old");
        assert_eq!(describe_age(100, 100 + 7 * 60), "7m old");
        assert_eq!(describe_age(100, 100 + 5 * 3600), "5h old");
        assert_eq!(describe_age(100, 100 + 3 * 86_400), "3d old");
    }

    /// t-71998: under the Google login a Gemini row says, in the picker's
    /// words, what the login's registry says of it — a release it serves
    /// under another name, or nothing it serves. From a synthetic registry.
    #[test]
    fn a_gemini_row_says_what_the_logins_registry_says_of_it() {
        use runtime::model_catalog::ModelCatalog;
        use runtime::model_discovery::{
            antigravity_rows, refresh_with, Answered, Overlay, UpdatePolicy, ANTIGRAVITY_SOURCE, ORIGIN_LIVE, SOURCES,
        };
        let home = tempfile::tempdir().expect("an overlay home");
        let user = ModelCatalog::load_from_home(home.path()).expect("an empty overlay");
        let rows = user.rows(&super::ALL_PROVIDERS, false);
        let document = serde_json::json!({"models": {
            "gemini-3.1-pro-low": {"displayName": "Gemini 3.1 Pro (Low)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-pro-agent": {"displayName": "Gemini 3.1 Pro (High)", "modelProvider": "MODEL_PROVIDER_GOOGLE"}
        }});
        let now = 1_792_700_000;
        let every: Vec<&'static str> = SOURCES.iter().map(|source| source.key).collect();
        let catalog = refresh_with(None, &every, now, &[], |source| {
            if source == ANTIGRAVITY_SOURCE {
                Ok(Answered {
                    source: source.to_string(),
                    models: antigravity_rows(&document, source),
                    origin: ORIGIN_LIVE.to_string(),
                    fetched_at: now,
                    note: None,
                })
            } else {
                Err((source.to_string(), "skipped: not asked in this test".to_string()))
            }
        });
        let overlay = Overlay::default();
        let report = super::Report {
            policy: UpdatePolicy::Auto,
            catalog: &catalog,
            user: &user,
            rows: &rows,
            overlay: &overlay,
            merged: None,
            tiers: &[],
            now: now + 60,
        };
        let text = report.text();
        let line = |id: &str| {
            text.lines()
                .find(|line| line.starts_with("google") && line.contains(&format!(" {id} ")))
                .unwrap_or_else(|| panic!("no {id} row in:\n{text}"))
                .to_string()
        };
        let renamed = line("gemini-3.1-pro-preview");
        assert!(renamed.ends_with(&crate::tui::strings::antigravity_renamed("gemini-3.1-pro")), "{renamed}");
        let unserved = line("gemini-3-flash");
        assert!(unserved.ends_with(crate::tui::strings::ANTIGRAVITY_UNLISTED), "{unserved}");
    }
}
