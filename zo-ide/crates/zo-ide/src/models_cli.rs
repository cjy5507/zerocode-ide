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
    let cached = model_discovery::current();
    let now = model_discovery::now_secs();
    let ttl = model_discovery::ttl_secs();
    let due = !model_discovery::due_sources(cached.as_deref(), now, ttl).is_empty();
    let catalog: DiscoveredCatalog = if refresh || due {
        let fresh = if refresh {
            model_discovery::discover()
        } else {
            model_discovery::discover_due(now, ttl)
        };
        model_discovery::install(fresh.clone())
            .map_err(|error| format!("could not write the discovery cache: {error}"))?;
        fresh
    } else {
        cached.as_deref().cloned().unwrap_or_default()
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
    let report = Report {
        policy,
        catalog: &catalog,
        user: &user,
        rows: &rows,
        overlay: &overlay,
        merged: merged.as_deref(),
        now,
    };
    if json {
        report.json()
    } else {
        Ok(report.text())
    }
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
                .unlisted_since(row)
                .map(|since| format!(" · unlisted since {}", describe_age(since, self.now).replace(" old", " ago")))
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
            // A moved alias says where it came from; one minted for a family the
            // shipped catalog never named has no "from" and says so.
            let moved = self
                .overlay
                .alias_updates
                .iter()
                .find(|update| update.alias == alias)
                .map_or_else(String::new, |update| {
                    if update.from.is_empty() {
                        "   (new)".to_string()
                    } else {
                        format!("   (was {})", update.from)
                    }
                });
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

    /// When the source stopped listing a discovered row a session had
    /// selected — the row stays, and says so.
    fn unlisted_since(&self, row: &CatalogRow) -> Option<u64> {
        if !row.discovered {
            return None;
        }
        self.catalog
            .models
            .iter()
            .find(|model| model.id.eq_ignore_ascii_case(&row.id))
            .and_then(|model| model.unlisted_since)
    }

    /// Which layer put the row on the screen.
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

    #[test]
    fn ages_read_like_a_person_would_say_them() {
        assert_eq!(describe_age(0, 10), "never fetched");
        assert_eq!(describe_age(100, 130), "30s old");
        assert_eq!(describe_age(100, 100 + 7 * 60), "7m old");
        assert_eq!(describe_age(100, 100 + 5 * 3600), "5h old");
        assert_eq!(describe_age(100, 100 + 3 * 86_400), "3d old");
    }
}
