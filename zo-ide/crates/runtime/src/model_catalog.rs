//! User-managed overlay for the built-in OAuth model catalog.
//!
//! The overlay lives in its own file, `<config home>/model-catalog.json`, never
//! in credential storage. It used to be the `modelCatalog` key of global
//! `settings.json`; that key is still read while the file does not exist, and
//! the first edit moves the overlay into the file whole — the key is never
//! written again. This module owns parsing, merging, validation, and locked
//! atomic persistence so TUI widgets and provider clients do not perform
//! ad-hoc JSON access.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::file_ops::SettingsFileLock;

use api::{AuthRoute, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The overlay's home, beside `settings.json`.
pub const CATALOG_FILE: &str = "model-catalog.json";
const SETTINGS_FILE: &str = "settings.json";
/// The legacy home: `settings.json`'s key, read only while [`CATALOG_FILE`]
/// does not exist.
const SETTINGS_KEY: &str = "modelCatalog";

/// Where the overlay was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlaySource {
    /// [`CATALOG_FILE`] — every edit is written here.
    File,
    /// `settings.json`'s `modelCatalog`, read because the catalog file does
    /// not exist yet. The first edit moves the overlay into the file.
    Settings,
    /// Neither declares anything.
    Undeclared,
}

impl OverlaySource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::File => "catalog file",
            Self::Settings => "settings (legacy)",
            Self::Undeclared => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogProvider {
    Anthropic,
    Openai,
    Google,
}

impl CatalogProvider {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Anthropic => "claude",
            Self::Openai => "openai",
            Self::Google => "google",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::Openai => "OpenAI",
            Self::Google => "Google",
        }
    }

    #[must_use]
    pub const fn kind(self) -> ProviderKind {
        match self {
            Self::Anthropic => ProviderKind::Anthropic,
            Self::Openai => ProviderKind::OpenAi,
            Self::Google => ProviderKind::Google,
        }
    }

    #[must_use]
    pub fn from_key(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Some(Self::Anthropic),
            "openai" | "chatgpt" | "codex" => Some(Self::Openai),
            "google" | "gemini" => Some(Self::Google),
            _ => None,
        }
    }

    /// The inverse of [`Self::kind`]; `None` for a provider the catalog
    /// screen does not list (xAI, Ollama).
    #[must_use]
    pub const fn from_kind(kind: ProviderKind) -> Option<Self> {
        match kind {
            ProviderKind::Anthropic => Some(Self::Anthropic),
            ProviderKind::OpenAi => Some(Self::Openai),
            ProviderKind::Google => Some(Self::Google),
            ProviderKind::Xai | ProviderKind::Ollama => None,
        }
    }
}

fn canonical_model_id(provider: CatalogProvider, id: &str) -> String {
    api::provider_catalog()
        .iter()
        .find(|entry| {
            entry.provider == provider.kind()
                && (entry.alias.eq_ignore_ascii_case(id)
                    || entry.canonical_model_id.eq_ignore_ascii_case(id))
        })
        .map_or_else(|| id.trim().to_ascii_lowercase(), |entry| {
            entry.canonical_model_id.to_ascii_lowercase()
        })
}

fn same_model_id(provider: CatalogProvider, left: &str, right: &str) -> bool {
    canonical_model_id(provider, left) == canonical_model_id(provider, right)
}

fn same_row_id(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRow {
    pub provider: CatalogProvider,
    pub id: String,
    pub display_name: String,
    pub auth_route: AuthRoute,
    pub builtin: bool,
    pub hidden: bool,
    /// The row came from provider discovery (`model_discovery`) rather than
    /// the shipped catalog or the user's overlay.
    pub discovered: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Overlay {
    #[serde(default)]
    models: Vec<UserModel>,
    #[serde(default)]
    hidden: Vec<ModelKey>,
    /// Alias → canonical rows in the `api` catalog's own shape, carried opaque
    /// for the same reason [`UserModel::wire`] is: that crate owns the schema
    /// and parses it. This is how an operator repoints a short alias
    /// (`gemini-flash`, `google-latest`) at a model the binary predates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserModel {
    provider: CatalogProvider,
    id: String,
    display_name: String,
    #[serde(default = "legacy_user_auth_route")]
    auth_route: AuthRoute,
    /// The id(s) the provider actually serves this model under, when they
    /// differ from `id` — either a bare string or `{"low":…,"medium":…,"high":…}`
    /// for a provider that bakes the reasoning tier into the id (Gemini does).
    ///
    /// Held as an opaque value on purpose: the schema belongs to the `api`
    /// crate's model catalog, which is where it is parsed. Restating it here
    /// would make one shape two definitions that could drift. Absent means the
    /// selection id IS the wire id, which is what every model needed until
    /// Google started shipping tiered ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wire: Option<Value>,
}

const fn legacy_user_auth_route() -> AuthRoute {
    AuthRoute::OAuth
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelKey {
    provider: CatalogProvider,
    id: String,
}

#[derive(Debug, Clone)]
pub struct ModelCatalog {
    overlay: Overlay,
    /// [`CATALOG_FILE`] — where every edit is written.
    catalog_path: PathBuf,
    /// `settings.json` — `modelUpdatePolicy`, and the legacy overlay key.
    settings_path: PathBuf,
    source: OverlaySource,
}

impl ModelCatalog {
    pub fn load() -> io::Result<Self> {
        Self::load_from_home(&crate::default_config_home())
    }

    /// Read the overlay under `home`: the catalog file when it exists,
    /// otherwise the legacy settings key. Reading never creates the file.
    pub fn load_from_home(home: &Path) -> io::Result<Self> {
        let catalog_path = home.join(CATALOG_FILE);
        let settings_path = home.join(SETTINGS_FILE);
        let (overlay, source) = match read_overlay_file(&catalog_path)? {
            Some(overlay) => (overlay, OverlaySource::File),
            None => match read_settings(&settings_path)?.remove(SETTINGS_KEY) {
                Some(declared) => (
                    serde_json::from_value(declared).map_err(|error| {
                        invalid_data(&settings_path, format_args!("{SETTINGS_KEY}: {error}"))
                    })?,
                    OverlaySource::Settings,
                ),
                None => (Overlay::default(), OverlaySource::Undeclared),
            },
        };
        Ok(Self {
            overlay,
            catalog_path,
            settings_path,
            source,
        })
    }

    #[must_use]
    pub const fn source(&self) -> OverlaySource {
        self.source
    }

    #[must_use]
    pub fn catalog_path(&self) -> &Path {
        &self.catalog_path
    }

    /// `settings.json` still carries a `modelCatalog` key that is no longer
    /// read because the catalog file exists — a copy the migration left
    /// behind, worth telling the person to remove. Reads settings on demand,
    /// so the answer costs nothing on the load path.
    #[must_use]
    pub fn settings_key_superseded(&self) -> bool {
        self.source == OverlaySource::File
            && read_settings(&self.settings_path)
                .is_ok_and(|root| root.contains_key(SETTINGS_KEY))
    }

    #[must_use]
    pub fn rows(&self, connected: &[CatalogProvider], include_hidden: bool) -> Vec<CatalogRow> {
        let mut rows = Vec::new();
        for (provider, id, display_name) in builtin_rows() {
            if !connected.contains(&provider) {
                continue;
            }
            let hidden = self.is_hidden(provider, &id);
            if !hidden || include_hidden {
                rows.push(CatalogRow {
                    provider,
                    id,
                    display_name,
                    auth_route: AuthRoute::Auto,
                    builtin: true,
                    hidden,
                    discovered: false,
                });
            }
        }
        for model in &self.overlay.models {
            if !connected.contains(&model.provider) {
                continue;
            }
            let hidden_builtin = builtin_rows().iter().any(|(provider, id, _)| {
                *provider == model.provider
                    && same_row_id(id, &model.id)
                    && self.is_hidden(model.provider, id)
            });
            if let Some(row) = rows.iter_mut().find(|row| {
                row.provider == model.provider && same_row_id(&row.id, &model.id)
            }) {
                row.display_name.clone_from(&model.display_name);
                row.auth_route = model.auth_route;
            } else if !hidden_builtin {
                rows.push(CatalogRow {
                    provider: model.provider,
                    id: model.id.clone(),
                    display_name: model.display_name.clone(),
                    auth_route: model.auth_route,
                    builtin: false,
                    hidden: false,
                    discovered: false,
                });
            }
        }
        // What the providers said they serve since this binary was built.
        // Shipped and user rows already named win; a hidden discovered id
        // stays hidden the same way a hidden built-in does.
        let policy = crate::model_discovery::UpdatePolicy::load_from(&self.settings_path);
        if let Some(discovered) = crate::model_discovery::current() {
            for model in crate::model_discovery::new_models(&discovered) {
                let Some(provider) = CatalogProvider::from_key(&model.provider) else {
                    continue;
                };
                if policy == crate::model_discovery::UpdatePolicy::Pinned
                    || !connected.contains(&provider)
                    || rows
                        .iter()
                        .any(|row| row.provider == provider && same_row_id(&row.id, &model.id))
                {
                    continue;
                }
                let hidden = self.is_hidden(provider, &model.id);
                if hidden && !include_hidden {
                    continue;
                }
                rows.push(CatalogRow {
                    provider,
                    id: model.id.clone(),
                    display_name: model.display_name.clone(),
                    auth_route: if model.api_key_only { AuthRoute::ApiKey } else { AuthRoute::Auto },
                    builtin: false,
                    hidden,
                    discovered: true,
                });
            }
        }
        rows
    }

    /// The overlay's model-catalog declarations — served ids and alias rows —
    /// in the `api` catalog's shape, for the process bridge that carries them
    /// into that crate.
    ///
    /// `None` when nothing is declared, so the caller can tell "nothing to
    /// publish" from "publish an empty catalog" — the two differ under an
    /// operator-set override, which must not be clobbered by an empty mirror.
    #[must_use]
    pub fn catalog_overlay_json(&self) -> Option<String> {
        let models = self
            .overlay
            .models
            .iter()
            .filter_map(|model| {
                let wire = model.wire.as_ref()?;
                let id = model.id.trim();
                if id.is_empty() || wire.is_null() {
                    return None;
                }
                Some(serde_json::json!({
                    "provider": model.provider.key(),
                    "ids": [id],
                    "wire": wire,
                }))
            })
            .collect::<Vec<_>>();
        if models.is_empty() && self.overlay.aliases.is_empty() {
            return None;
        }
        serde_json::to_string(&serde_json::json!({
            "models": models,
            "aliases": self.overlay.aliases,
        }))
        .ok()
    }

    #[must_use]
    pub fn builtin_hidden(&self, provider: ProviderKind, id: &str) -> bool {
        let aliases = builtin_rows()
            .into_iter()
            .filter(|(catalog_provider, alias, _)| {
                catalog_provider.kind() == provider
                    && canonical_model_id(*catalog_provider, alias)
                        == canonical_model_id(*catalog_provider, id)
            })
            .map(|(catalog_provider, alias, _)| (catalog_provider, alias))
            .collect::<Vec<_>>();
        !aliases.is_empty()
            && aliases
                .iter()
                .all(|(catalog_provider, alias)| self.is_hidden(*catalog_provider, alias))
    }

    #[must_use]
    pub fn provider_for_model(&self, id: &str) -> Option<ProviderKind> {
        let id = id.trim();
        if gateway_owns_slash_id(id) {
            return None;
        }
        if let Some((provider, model_id)) = id.split_once('/') {
            let provider = CatalogProvider::from_key(provider)?;
            return (!model_id.trim().is_empty()).then(|| provider.kind());
        }

        let providers = self
            .rows(
                &[
                    CatalogProvider::Anthropic,
                    CatalogProvider::Openai,
                    CatalogProvider::Google,
                ],
                false,
            )
            .into_iter()
            .filter(|row| same_row_id(&row.id, id))
            .map(|row| row.provider)
            .collect::<std::collections::HashSet<_>>();
        if providers.len() != 1 {
            return None;
        }
        providers.into_iter().next().map(CatalogProvider::kind)
    }

    #[must_use]
    pub fn auth_route_for_model(&self, id: &str) -> Option<AuthRoute> {
        let id = id.trim();
        if gateway_owns_slash_id(id) {
            return None;
        }
        let rows = self.rows(
            &[
                CatalogProvider::Anthropic,
                CatalogProvider::Openai,
                CatalogProvider::Google,
            ],
            false,
        );
        if let Some((provider, model_id)) = id.split_once('/') {
            let provider = CatalogProvider::from_key(provider)?;
            return rows
                .iter()
                .find(|row| row.provider == provider && same_row_id(&row.id, model_id))
                .map(|row| row.auth_route);
        }
        let mut matches = rows.iter().filter(|row| same_row_id(&row.id, id));
        let route = matches.next()?.auth_route;
        matches.next().is_none().then_some(route)
    }

    #[must_use]
    pub fn selection_token(&self, provider: CatalogProvider, id: &str) -> String {
        let collision = self
            .rows(
                &[
                    CatalogProvider::Anthropic,
                    CatalogProvider::Openai,
                    CatalogProvider::Google,
                ],
                false,
            )
            .iter()
            .any(|row| row.provider != provider && same_row_id(&row.id, id));
        if collision {
            format!("{}/{}", provider.key(), id.trim())
        } else {
            id.trim().to_string()
        }
    }

    pub fn add(&mut self, provider: CatalogProvider, id: &str, display_name: &str) -> Result<(), String> {
        self.add_with_auth_route(provider, id, display_name, AuthRoute::OAuth)
    }

    pub fn add_with_auth_route(
        &mut self,
        provider: CatalogProvider,
        id: &str,
        display_name: &str,
        auth_route: AuthRoute,
    ) -> Result<(), String> {
        validate_fields(id, display_name)?;
        if let Some(existing) = self
            .rows(&[provider], true)
            .into_iter()
            .find(|row| same_model_id(provider, &row.id, id))
        {
            return Err(duplicate_error(&existing));
        }
        self.overlay.models.push(UserModel {
            provider,
            id: id.trim().to_string(),
            display_name: display_name.trim().to_string(),
            auth_route,
            wire: None,
        });
        self.persist().map_err(|error| error.to_string())
    }

    pub fn edit(
        &mut self,
        original: &CatalogRow,
        provider: CatalogProvider,
        id: &str,
        display_name: &str,
    ) -> Result<(), String> {
        self.edit_with_auth_route(
            original,
            provider,
            id,
            display_name,
            original.auth_route,
        )
    }

    pub fn edit_with_auth_route(
        &mut self,
        original: &CatalogRow,
        provider: CatalogProvider,
        id: &str,
        display_name: &str,
        auth_route: AuthRoute,
    ) -> Result<(), String> {
        validate_fields(id, display_name)?;
        let duplicate = self
            .rows(
                &[
                    CatalogProvider::Anthropic,
                    CatalogProvider::Openai,
                    CatalogProvider::Google,
                ],
                true,
            )
            .into_iter()
            .find(|row| {
                let is_original = row.provider == original.provider
                    && same_row_id(&row.id, &original.id);
                !is_original
                    && row.provider == provider
                    && same_model_id(provider, &row.id, id)
            });
        if let Some(existing) = duplicate {
            return Err(duplicate_error(&existing));
        }
        if original.builtin {
            if original.provider != provider || !same_row_id(&original.id, id) {
                self.hide_builtin(original.provider, &original.id);
            }
            self.upsert_user(provider, id, display_name, auth_route);
        } else if let Some(model) = self.overlay.models.iter_mut().find(|model| {
            model.provider == original.provider && same_row_id(&model.id, &original.id)
        }) {
            model.provider = provider;
            model.id = id.trim().to_string();
            model.display_name = display_name.trim().to_string();
            model.auth_route = auth_route;
        }
        self.persist().map_err(|error| error.to_string())
    }

    pub fn delete_or_hide(&mut self, row: &CatalogRow) -> Result<(), String> {
        if row.builtin {
            self.hide_builtin(row.provider, &row.id);
        } else {
            self.overlay.models.retain(|model| {
                model.provider != row.provider || !same_row_id(&model.id, &row.id)
            });
        }
        self.persist().map_err(|error| error.to_string())
    }

    pub fn restore(&mut self, row: &CatalogRow) -> Result<(), String> {
        self.overlay.hidden.retain(|key| {
            key.provider != row.provider || !same_row_id(&key.id, &row.id)
        });
        self.persist().map_err(|error| error.to_string())
    }

    fn upsert_user(
        &mut self,
        provider: CatalogProvider,
        id: &str,
        display_name: &str,
        auth_route: AuthRoute,
    ) {
        if let Some(model) = self.overlay.models.iter_mut().find(|model| {
            model.provider == provider && same_row_id(&model.id, id)
        }) {
            model.display_name = display_name.trim().to_string();
            model.auth_route = auth_route;
        } else {
            self.overlay.models.push(UserModel {
                provider,
                id: id.trim().to_string(),
                display_name: display_name.trim().to_string(),
                auth_route,
                wire: None,
            });
        }
    }

    fn hide_builtin(&mut self, provider: CatalogProvider, id: &str) {
        if !self.is_hidden(provider, id) {
            self.overlay.hidden.push(ModelKey {
                provider,
                id: id.trim().to_string(),
            });
        }
    }

    /// Hidden by any name for the same model: a `hidden` entry written when
    /// rows were listed by alias (`fable`) still hides the canonical row.
    fn is_hidden(&self, provider: CatalogProvider, id: &str) -> bool {
        self.overlay
            .hidden
            .iter()
            .any(|key| key.provider == provider && same_model_id(provider, &key.id, id))
    }

    /// Write the overlay to the catalog file. An overlay read from the legacy
    /// settings key lands here whole, which is the migration: from now on the
    /// file exists and the key is not read.
    fn persist(&mut self) -> io::Result<()> {
        let _lock = SettingsFileLock::acquire(&self.catalog_path)?;
        let rendered = serde_json::to_string_pretty(&self.overlay).map_err(io::Error::other)?;
        crate::file_ops::replace_file_atomic(&self.catalog_path, format!("{rendered}\n").as_bytes())?;
        self.source = OverlaySource::File;
        Ok(())
    }
}

/// The inline form error for an ID that another row already resolves to.
///
/// Names the offending row instead of a bare "already exists": the collision is
/// on the *canonical* id, so an alias row (`opus` → `claude-opus-5`) blocks its
/// own canonical id and the bare message read as a dead end. A hidden row is
/// invisible in the list, so say so — restoring it is the fix, not renaming.
fn duplicate_error(existing: &CatalogRow) -> String {
    let hint = if existing.hidden {
        " — hidden, press r to restore"
    } else {
        " — edit that row instead"
    };
    format!(
        "Already provided by \"{}\" (id `{}`){hint}",
        existing.display_name, existing.id
    )
}

fn validate_fields(id: &str, display_name: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("Model ID cannot be empty".to_string());
    }
    if id.chars().any(char::is_control) {
        return Err("Model ID cannot contain control characters".to_string());
    }
    if display_name.trim().is_empty() {
        return Err("Display name cannot be empty".to_string());
    }
    if display_name.chars().any(char::is_control) {
        return Err("Display name cannot contain control characters".to_string());
    }
    Ok(())
}

fn invalid_data(path: &Path, detail: std::fmt::Arguments<'_>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {detail}", path.display()))
}

/// The catalog file's overlay; `None` when the file does not exist, which is
/// what sends the reader to the legacy settings key. An empty file is a file:
/// it declares nothing and still supersedes the key.
fn read_overlay_file(path: &Path) -> io::Result<Option<Overlay>> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Some(Overlay::default())),
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| invalid_data(path, format_args!("{error}"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_settings(path: &Path) -> io::Result<Map<String, Value>> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Map::new()),
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| invalid_data(path, format_args!("{error}")))?
            .as_object()
            .cloned()
            .ok_or_else(|| invalid_data(path, format_args!("must contain an object"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(error),
    }
}

/// Whether `id` is a connected gateway's model rather than this catalog's
/// `<provider>/<id>` selection token: `anthropic/claude-x` that `OpenRouter`
/// declares is `OpenRouter`'s, and an explicit `<gateway>/<model>` names the
/// gateway. Either way, the catalog claims no provider or auth route for it.
fn gateway_owns_slash_id(id: &str) -> bool {
    id.contains('/') && api::custom_provider_for_model(id).is_some()
}

/// One row per shipped canonical model, in catalog order — the id is the
/// canonical id and the name is what the catalog spells for it
/// ([`api::model_display_name`]). Derived, never listed: a row the shipped
/// catalog gains is a row here in the same build, and a label variant
/// (`opus[1m]`) is the same model once, not twice.
#[must_use]
pub fn builtin_rows() -> Vec<(CatalogProvider, String, String)> {
    let mut rows: Vec<(CatalogProvider, String, String)> = Vec::new();
    for entry in api::builtin_provider_catalog() {
        let Some(provider) = CatalogProvider::from_kind(entry.provider) else {
            continue;
        };
        if rows
            .iter()
            .any(|(seen, id, _)| *seen == provider && id.eq_ignore_ascii_case(entry.canonical_model_id))
        {
            continue;
        }
        rows.push((
            provider,
            entry.canonical_model_id.to_string(),
            api::model_display_name(entry.canonical_model_id),
        ));
    }
    rows
}

/// Human-readable family label for `model`, from the catalog's name for it
/// instead of a hardcoded release name.
///
/// Matching is on the resolved wire id, so a short alias (`opus`), a label
/// variant (`opus[1m]`), a full id (`claude-opus-5`), and a `provider/model`
/// ref all land on the same row — and bumping a catalog row to the next
/// release moves every label with it.
///
/// Returns `None` for a model the catalog does not carry (a custom provider's
/// id). Callers then show the raw id, which is honest, rather than inventing
/// a family name for it.
#[must_use]
pub fn model_family_label(model: &str) -> Option<String> {
    let wire = api::wire_model_id(model);
    let wire = wire.trim();
    if wire.is_empty() {
        return None;
    }
    api::provider_catalog()
        .iter()
        .find(|entry| {
            api::wire_model_id(entry.canonical_model_id).eq_ignore_ascii_case(wire)
                || api::wire_model_id(entry.alias).eq_ignore_ascii_case(wire)
        })
        .map(|entry| {
            let name = api::model_display_name(entry.canonical_model_id);
            match entry.provider {
                // Anthropic names carry the bare lineup ("Opus 5"); the label
                // adds the maker once. Every other provider's name already
                // names it ("GPT-5.6-Sol").
                ProviderKind::Anthropic => format!("Claude {name}"),
                _ => name,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings-declared model carries the ids its provider serves it under,
    /// and those survive a round trip through the file — this is what lets a
    /// model the binary predates be reachable without a rebuild.
    #[test]
    fn a_declared_wire_mapping_round_trips_and_publishes() {
        let home = home("wire-round-trip");
        fs::write(
            home.join(SETTINGS_FILE),
            r#"{"modelCatalog":{"models":[
                {"provider":"google","id":"gemini-3.7-flash","displayName":"Gemini 3.7 Flash",
                 "wire":{"low":"gemini-3.7-flash-low","high":"gemini-3.7-flash-high"}},
                {"provider":"openai","id":"gpt-5.7","displayName":"GPT-5.7"}
            ]}}"#,
        )
        .unwrap();

        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        let published: Value =
            serde_json::from_str(&catalog.catalog_overlay_json().expect("declared")).unwrap();
        let models = published["models"].as_array().unwrap();
        assert_eq!(models.len(), 1, "only declaring models are published");
        assert_eq!(models[0]["ids"][0], "gemini-3.7-flash");
        assert_eq!(models[0]["wire"]["high"], "gemini-3.7-flash-high");

        // Renaming through the picker must not drop the declaration: the row
        // would keep its name and quietly go back to being served under its own
        // id, which is the silent-substitution failure this whole path exists
        // to remove.
        let row = catalog
            .rows(&[CatalogProvider::Google], false)
            .into_iter()
            .find(|row| row.id == "gemini-3.7-flash")
            .expect("row");
        catalog
            .edit(&row, CatalogProvider::Google, "gemini-3.7-flash", "Flash 3.7")
            .unwrap();
        let reloaded = ModelCatalog::load_from_home(&home).unwrap();
        let published: Value =
            serde_json::from_str(&reloaded.catalog_overlay_json().expect("still declared")).unwrap();
        assert_eq!(
            published["models"][0]["wire"]["low"],
            "gemini-3.7-flash-low",
            "an edit preserves the served ids"
        );

        let _ = fs::remove_dir_all(&home);
    }

    /// Nothing declared is not the same as an empty catalog — the bridge needs
    /// to tell them apart so it never clobbers an operator's own export.
    #[test]
    fn no_declaration_publishes_nothing() {
        let home = home("wire-none");
        fs::write(
            home.join(SETTINGS_FILE),
            r#"{"modelCatalog":{"models":[{"provider":"openai","id":"gpt-5.7","displayName":"GPT-5.7"}]}}"#,
        )
        .unwrap();
        let catalog = ModelCatalog::load_from_home(&home).unwrap();
        assert!(catalog.catalog_overlay_json().is_none());
        let _ = fs::remove_dir_all(&home);
    }

    /// Alias rows travel through the overlay too, so repointing a short name is
    /// a settings edit rather than a rebuild.
    #[test]
    fn declared_aliases_reach_the_overlay_payload() {
        let home = home("alias-overlay");
        fs::write(
            home.join(SETTINGS_FILE),
            r#"{"modelCatalog":{"aliases":[{"alias":"google-latest","canonical":"gemini-3.7-flash","provider":"google"}]}}"#,
        )
        .unwrap();
        let catalog = ModelCatalog::load_from_home(&home).unwrap();
        let published: Value =
            serde_json::from_str(&catalog.catalog_overlay_json().expect("declared")).unwrap();
        assert_eq!(published["aliases"][0]["canonical"], "gemini-3.7-flash");
        assert!(published["models"].as_array().unwrap().is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    fn overlay_ids(catalog: &ModelCatalog) -> Vec<String> {
        catalog
            .rows(&[CatalogProvider::Openai], false)
            .into_iter()
            .filter(|row| !row.builtin && !row.discovered)
            .map(|row| row.id)
            .collect()
    }

    /// The catalog file is the overlay's home: while it exists, the settings
    /// key is not read at all — a stale copy the migration left behind cannot
    /// resurrect a row the file dropped.
    #[test]
    fn the_catalog_file_supersedes_the_settings_key() {
        let home = home("file-first");
        fs::write(
            home.join(SETTINGS_FILE),
            r#"{"modelCatalog":{"models":[{"provider":"openai","id":"from-settings","displayName":"From Settings"}]}}"#,
        )
        .unwrap();
        fs::write(
            home.join(CATALOG_FILE),
            r#"{"models":[{"provider":"openai","id":"from-file","displayName":"From File"}]}"#,
        )
        .unwrap();

        let catalog = ModelCatalog::load_from_home(&home).unwrap();
        assert_eq!(catalog.source(), OverlaySource::File);
        assert_eq!(catalog.catalog_path(), home.join(CATALOG_FILE));
        assert_eq!(overlay_ids(&catalog), ["from-file"]);
        assert!(catalog.settings_key_superseded(), "the stale key is worth a notice");
        let _ = fs::remove_dir_all(&home);
    }

    /// A settings-only overlay keeps working, reading never creates the file,
    /// and the first edit moves the overlay into the file whole. settings.json
    /// is read-only compat from then on: not a byte of it changes.
    #[test]
    fn the_first_edit_moves_a_legacy_settings_overlay_into_the_file() {
        let home = home("legacy-migrates");
        let settings = r#"{"theme":"dark","modelCatalog":{"models":[{"provider":"openai","id":"from-settings","displayName":"From Settings"}]}}"#;
        fs::write(home.join(SETTINGS_FILE), settings).unwrap();

        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        assert_eq!(catalog.source(), OverlaySource::Settings);
        assert!(!catalog.settings_key_superseded());
        assert!(!home.join(CATALOG_FILE).exists(), "reading never creates the file");

        catalog.add(CatalogProvider::Openai, "added-later", "Added Later").unwrap();
        assert_eq!(catalog.source(), OverlaySource::File);
        assert_eq!(fs::read_to_string(home.join(SETTINGS_FILE)).unwrap(), settings);

        let reloaded = ModelCatalog::load_from_home(&home).unwrap();
        assert_eq!(reloaded.source(), OverlaySource::File);
        assert_eq!(overlay_ids(&reloaded), ["from-settings", "added-later"]);
        assert!(reloaded.settings_key_superseded());
        let _ = fs::remove_dir_all(&home);
    }

    /// An empty file is a file: it declares nothing and still supersedes the
    /// key, so "delete every row" cannot fall back to the legacy copy.
    #[test]
    fn an_empty_catalog_file_still_supersedes_the_settings_key() {
        let home = home("empty-file");
        fs::write(
            home.join(SETTINGS_FILE),
            r#"{"modelCatalog":{"models":[{"provider":"openai","id":"from-settings","displayName":"From Settings"}]}}"#,
        )
        .unwrap();
        fs::write(home.join(CATALOG_FILE), "\n").unwrap();
        let catalog = ModelCatalog::load_from_home(&home).unwrap();
        assert_eq!(catalog.source(), OverlaySource::File);
        assert!(overlay_ids(&catalog).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn nothing_declared_reads_as_undeclared_and_a_broken_file_names_itself() {
        let home = home("undeclared");
        let catalog = ModelCatalog::load_from_home(&home).unwrap();
        assert_eq!(catalog.source(), OverlaySource::Undeclared);
        assert!(overlay_ids(&catalog).is_empty());

        fs::write(home.join(CATALOG_FILE), "{not json").unwrap();
        let error = ModelCatalog::load_from_home(&home).expect_err("malformed");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains(CATALOG_FILE), "{error}");
        let _ = fs::remove_dir_all(&home);
    }

    /// A fresh config home per test: the catalog file and `settings.json`
    /// are siblings, so a shared temp dir would let tests see each other.
    fn home(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!(
            "zo-model-catalog-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn overlay_add_edit_hide_restore_round_trips_without_touching_other_settings() {
        let home = home("roundtrip");
        fs::write(home.join(SETTINGS_FILE), r#"{"theme":"dark"}"#).unwrap();
        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        catalog.add(CatalogProvider::Google, "gemini-4.0-flash", "Gemini 4.0 Flash").unwrap();
        let user = catalog.rows(&[CatalogProvider::Google], false).into_iter().find(|row| row.id == "gemini-4.0-flash").unwrap();
        catalog.edit(&user, CatalogProvider::Google, "gemini-4.0-flash", "Future Flash").unwrap();
        catalog.delete_or_hide(&CatalogRow { provider: CatalogProvider::Google, id: "gemini-3.5-flash".into(), display_name: "Gemini 3.5 Flash".into(), auth_route: AuthRoute::Auto, builtin: true, hidden: false, discovered: false }).unwrap();
        let loaded = ModelCatalog::load_from_home(&home).unwrap();
        assert!(!loaded.rows(&[CatalogProvider::Google], false).iter().any(|row| row.id == "gemini-3.5-flash"));
        assert_eq!(loaded.provider_for_model("gemini-4.0-flash"), Some(ProviderKind::Google));
        let hidden = loaded.rows(&[CatalogProvider::Google], true).into_iter().find(|row| row.id == "gemini-3.5-flash").unwrap();
        let mut loaded = loaded;
        loaded.restore(&hidden).unwrap();
        assert!(loaded.rows(&[CatalogProvider::Google], false).iter().any(|row| row.id == "gemini-3.5-flash"));
        let root: Value = serde_json::from_str(&fs::read_to_string(home.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(root["theme"], "dark");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn auth_route_migrates_legacy_rows_and_survives_builtin_hide_restore() {
        let home = home("auth-route");
        fs::write(
            home.join(SETTINGS_FILE),
            r#"{"modelCatalog":{"models":[{"provider":"google","id":"gemini-legacy-flash","displayName":"Legacy Flash"}]}}"#,
        )
        .unwrap();

        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        let legacy = catalog
            .rows(&[CatalogProvider::Google], false)
            .into_iter()
            .find(|row| row.id == "gemini-legacy-flash")
            .unwrap();
        assert_eq!(legacy.auth_route, AuthRoute::OAuth);
        let builtin = catalog
            .rows(&[CatalogProvider::Google], false)
            .into_iter()
            .find(|row| row.id == "gemini-3.5-flash")
            .unwrap();
        assert_eq!(builtin.auth_route, AuthRoute::Auto);

        catalog
            .edit_with_auth_route(
                &builtin,
                builtin.provider,
                &builtin.id,
                &builtin.display_name,
                AuthRoute::ApiKey,
            )
            .unwrap();
        let overridden = catalog
            .rows(&[CatalogProvider::Google], false)
            .into_iter()
            .find(|row| row.id == "gemini-3.5-flash")
            .unwrap();
        assert_eq!(overridden.auth_route, AuthRoute::ApiKey);
        catalog.delete_or_hide(&overridden).unwrap();
        let hidden = catalog
            .rows(&[CatalogProvider::Google], true)
            .into_iter()
            .find(|row| row.id == "gemini-3.5-flash")
            .unwrap();
        assert!(hidden.hidden);
        assert_eq!(hidden.auth_route, AuthRoute::ApiKey);
        catalog.restore(&hidden).unwrap();

        let loaded = ModelCatalog::load_from_home(&home).unwrap();
        let restored = loaded
            .rows(&[CatalogProvider::Google], false)
            .into_iter()
            .find(|row| row.id == "gemini-3.5-flash")
            .unwrap();
        assert_eq!(restored.auth_route, AuthRoute::ApiKey);
        let root: Value = serde_json::from_str(&fs::read_to_string(home.join(CATALOG_FILE)).unwrap()).unwrap();
        assert_eq!(root["models"][1]["authRoute"], "api-key");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn promoted_builtin_preserves_existing_oauth_overlay() {
        let home = home("promoted-builtin");
        fs::write(
            home.join(SETTINGS_FILE),
            r#"{"modelCatalog":{"models":[{"provider":"google","id":"gemini-3.6-flash","displayName":"Gemini 3.6 Flash","authRoute":"oauth"}],"hidden":[{"provider":"google","id":"gemini-3.5-flash"}]}}"#,
        )
        .unwrap();

        let catalog = ModelCatalog::load_from_home(&home).unwrap();
        let visible = catalog.rows(&[CatalogProvider::Google], false);
        let promoted = visible
            .iter()
            .find(|row| row.id == "gemini-3.6-flash")
            .unwrap();

        assert!(promoted.builtin);
        assert_eq!(promoted.auth_route, AuthRoute::OAuth);
        assert!(!visible.iter().any(|row| row.id == "gemini-3.5-flash"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn editing_builtin_to_future_id_persists_replacement_and_reversible_tombstone() {
        let home = home("builtin-edit");
        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        let builtin = catalog
            .rows(&[CatalogProvider::Google], false)
            .into_iter()
            .find(|row| row.id == "gemini-3.5-flash")
            .unwrap();

        catalog
            .edit(&builtin, CatalogProvider::Google, "gemini-4.0-flash", "Gemini 4.0 Flash")
            .unwrap();

        let loaded = ModelCatalog::load_from_home(&home).unwrap();
        let visible = loaded.rows(&[CatalogProvider::Google], false);
        assert!(!visible.iter().any(|row| row.id == "gemini-3.5-flash"));
        assert!(visible.iter().any(|row| !row.builtin && row.id == "gemini-4.0-flash"));
        let hidden = loaded
            .rows(&[CatalogProvider::Google], true)
            .into_iter()
            .find(|row| row.id == "gemini-3.5-flash")
            .unwrap();
        assert!(hidden.hidden);
        let mut loaded = loaded;
        loaded.restore(&hidden).unwrap();
        assert!(loaded.rows(&[CatalogProvider::Google], false).iter().any(|row| row.id == "gemini-3.5-flash"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn same_id_builtin_override_can_be_hidden_and_is_excluded_from_smart() {
        let home = home("override-hide");
        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        let builtin = catalog.rows(&[CatalogProvider::Google], false).into_iter()
            .find(|row| row.id == "gemini-3.5-flash").unwrap();
        catalog.edit(&builtin, CatalogProvider::Google, "gemini-3.5-flash", "Preferred Flash").unwrap();
        let overridden = catalog.rows(&[CatalogProvider::Google], false).into_iter()
            .find(|row| row.id == "gemini-3.5-flash").unwrap();
        assert_eq!(overridden.display_name, "Preferred Flash");
        catalog.delete_or_hide(&overridden).unwrap();
        assert!(!catalog.rows(&[CatalogProvider::Google], false).iter().any(|row| row.id == "gemini-3.5-flash"));
        assert!(catalog.builtin_hidden(ProviderKind::Google, "gemini-3.5-flash"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn validation_rejects_empty_control_and_duplicate_ids() {
        let home = home("validation");
        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        assert!(catalog.add(CatalogProvider::Google, "", "Empty").is_err());
        assert!(catalog.add(CatalogProvider::Google, "bad\nid", "Bad").is_err());
        assert!(catalog.add(CatalogProvider::Google, "gemini-3.5-flash", "Duplicate").is_err());
        assert!(catalog.add(CatalogProvider::Google, "gemini-flash", "Alias duplicate").is_err());
        catalog
            .add(CatalogProvider::Openai, "gemini-3.5-flash", "Provider-qualified")
            .unwrap();
        catalog
            .add(CatalogProvider::Google, "shared-future-id", "Google Shared")
            .unwrap();
        catalog
            .add(CatalogProvider::Openai, "shared-future-id", "OpenAI Shared")
            .unwrap();
        assert_eq!(
            catalog.selection_token(CatalogProvider::Google, "shared-future-id"),
            "google/shared-future-id"
        );
        assert_eq!(
            catalog.provider_for_model("google/shared-future-id"),
            Some(ProviderKind::Google)
        );
        assert_eq!(
            catalog.provider_for_model("openai/shared-future-id"),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(
            catalog.selection_token(CatalogProvider::Google, "gemini-3.5-flash"),
            "google/gemini-3.5-flash"
        );
        assert_eq!(
            catalog.selection_token(CatalogProvider::Openai, "gemini-3.5-flash"),
            "openai/gemini-3.5-flash"
        );
        assert_eq!(catalog.provider_for_model("gemini-3.5-flash"), None);
        let _ = fs::remove_dir_all(&home);
    }

    /// A connected gateway's own slash id (`anthropic/claude-x` on
    /// `OpenRouter`) is not this catalog's `<provider>/<id>` token: the
    /// catalog claims neither a provider nor an auth route for it, so every
    /// client builder that asks the catalog first falls through to the
    /// gateway. An undeclared `anthropic/<id>` keeps its first-party reading,
    /// and a bare catalog id stays first-party even when the gateway lists it.
    #[test]
    fn a_gateways_own_slash_id_is_not_a_catalog_token() {
        let _lock = crate::test_env_lock();
        let prior = std::env::var_os(api::CUSTOM_PROVIDERS_ENV);
        std::env::remove_var(api::CUSTOM_PROVIDERS_ENV);
        let home = home("gateway-slash-id");
        let catalog = ModelCatalog::load_from_home(&home).unwrap();
        let (_, bare, _) = builtin_rows()
            .into_iter()
            .find(|(provider, _, _)| *provider == CatalogProvider::Anthropic)
            .expect("an Anthropic built-in row");
        let slashed = format!("anthropic/{bare}");
        api::refresh_custom_providers_from_json("[]").expect("no gateway");
        assert_eq!(
            catalog.provider_for_model(&slashed),
            Some(ProviderKind::Anthropic),
            "undeclared, `anthropic/<id>` is the catalog's own token"
        );
        assert!(catalog.auth_route_for_model(&slashed).is_some());

        api::refresh_custom_providers_from_json(&format!(
            r#"[{{"name":"slashgateway","base_url":"http://127.0.0.1:9/v1",
                 "models":["{slashed}","{bare}"],"requires_auth":false}}]"#
        ))
        .expect("custom providers");
        assert_eq!(catalog.provider_for_model(&slashed), None);
        assert_eq!(catalog.auth_route_for_model(&slashed), None);
        assert_eq!(catalog.provider_for_model(&format!("slashgateway/{slashed}")), None);
        assert_eq!(catalog.provider_for_model(&bare), Some(ProviderKind::Anthropic));

        api::refresh_custom_providers_from_json("[]").expect("clear");
        match prior {
            Some(value) => std::env::set_var(api::CUSTOM_PROVIDERS_ENV, value),
            None => std::env::remove_var(api::CUSTOM_PROVIDERS_ENV),
        }
        let _ = fs::remove_dir_all(&home);
    }

    /// Hiding the `opus` alias row hides the canonical for the smart router, and
    /// restoring it brings it back. Anthropic ships one row per distinct model,
    /// so the alias row IS the canonical's only representation — the former
    /// `opus`/`opus[1m]` pair was the same model listed twice.
    #[test]
    fn hiding_the_opus_alias_hides_its_canonical_and_restore_brings_it_back() {
        let home = home("alias-hide");
        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        let rows = catalog.rows(&[CatalogProvider::Anthropic], false);
        assert!(
            !rows.iter().any(|row| row.id == "opus[1m]"),
            "the 1M label alias must not be a duplicate catalog row"
        );
        assert!(!catalog.builtin_hidden(ProviderKind::Anthropic, "claude-opus-5"));

        let opus = rows.into_iter().find(|row| row.id == "claude-opus-5").unwrap();
        assert_eq!(opus.display_name, "Opus 5", "the name is derived from the id");
        catalog.delete_or_hide(&opus).unwrap();

        let visible = catalog.rows(&[CatalogProvider::Anthropic], false);
        assert!(!visible.iter().any(|row| row.id == "claude-opus-5"));
        assert!(catalog.builtin_hidden(ProviderKind::Anthropic, "claude-opus-5"));
        // A sibling canonical is unaffected by hiding Opus.
        assert!(visible.iter().any(|row| row.id == "claude-fable-5-1"));

        let hidden = catalog
            .rows(&[CatalogProvider::Anthropic], true)
            .into_iter()
            .find(|row| row.id == "claude-opus-5")
            .unwrap();
        catalog.restore(&hidden).unwrap();
        assert!(!catalog.builtin_hidden(ProviderKind::Anthropic, "claude-opus-5"));
        let _ = fs::remove_dir_all(&home);
    }

    /// The duplicate-ID error names the row that already covers the id, so the
    /// user can act on it instead of hitting a bare "already exists" dead end.
    #[test]
    fn duplicate_id_error_names_the_conflicting_row() {
        let home = home("dup-msg");
        let mut catalog = ModelCatalog::load_from_home(&home).unwrap();
        // `claude-opus-5` is what the built-in `opus` row resolves to.
        let error = catalog
            .add(CatalogProvider::Anthropic, "claude-opus-5", "Opus 5")
            .expect_err("canonical of an existing alias row must collide");
        assert!(error.contains("Opus 5"), "{error}");
        assert!(error.contains("opus"), "{error}");
        assert!(error.contains("edit that row"), "{error}");

        // Hidden rows are invisible in the picker, so the error must say so.
        let opus = catalog
            .rows(&[CatalogProvider::Anthropic], false)
            .into_iter()
            .find(|row| row.id == "claude-opus-5")
            .unwrap();
        catalog.delete_or_hide(&opus).unwrap();
        let error = catalog
            .add(CatalogProvider::Anthropic, "opus", "My Opus")
            .expect_err("hidden built-in still owns the id");
        assert!(error.contains("hidden"), "{error}");

        // A genuinely new id still lands.
        catalog
            .add(CatalogProvider::Anthropic, "claude-opus-4-6", "Opus 4.6")
            .expect("unregistered older Opus is not a duplicate");
        let _ = fs::remove_dir_all(&home);
    }
}
