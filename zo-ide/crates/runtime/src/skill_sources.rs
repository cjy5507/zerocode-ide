//! Provider-skill catalog: the single place that decides *which* skill roots
//! Zo trusts and in what precedence, then enumerates the skill directories they
//! contain.
//!
//! Zo's own skills live under project/global `.zo/skills`. On top of those this
//! module surfaces the *enabled* bundled/downloaded skills of the two coding
//! agents a user may already have installed:
//!
//! - **Claude Code** — `~/.claude/settings.json` `enabledPlugins` selects which
//!   installed plugins are active; `~/.claude/plugins/installed_plugins.json`
//!   maps a plugin id to its on-disk `installPath`. A plugin's skills live under
//!   `<installPath>/skills/<name>/SKILL.md`. Direct user skills live under
//!   `~/.claude/skills`.
//! - **Codex** — `~/.codex/config.toml` `[plugins."id"].enabled = true` selects
//!   active plugins, whose skills live under a matching cache root
//!   `~/.codex/plugins/<id>/skills`. Direct user skills live under
//!   `~/.codex/skills`.
//!
//! These are already-installed, user-enabled skills — this module never
//! downloads, synthesizes, or persists a skill, and never makes a model call.
//! It only *reads* metadata for the prompt index / implicit router and hands the
//! `Skill` tool a canonical path to load a body on demand.
//!
//! Trust rules, deliberately conservative:
//! - **Precedence** (highest first): project Zo → global Zo → enabled Claude
//!   user/plugin → enabled Codex user/plugin. A name collision resolves to the
//!   highest-precedence source, so a Zo skill always wins over a provider one.
//! - **Enablement gates the cache.** A plugin present in a provider's cache but
//!   not enabled in its registry contributes nothing. Only roots selected by an
//!   enabled registry/plugin id are scanned.
//! - **Defensive parsing.** A missing or malformed registry yields no
//!   candidates rather than an error.
//! - **Containment.** For provider roots the candidate directory and its
//!   `SKILL.md` are canonicalized and required to stay inside the trusted root,
//!   so a symlink escaping the plugin tree is rejected. (Zo roots keep their
//!   documented symlink-in behavior and are not canonicalized here.)

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Test-only override for the Claude home root (`~/.claude`). When set and
/// non-empty it fully replaces the `HOME`-derived path; set to empty to
/// neutralize provider discovery. Keeps tests off the developer's real home.
const CLAUDE_HOME_ENV: &str = "ZO_CLAUDE_HOME";
/// Test-only override for the Codex home root (`~/.codex`); see
/// [`CLAUDE_HOME_ENV`].
const CODEX_HOME_ENV: &str = "ZO_CODEX_HOME";

/// Where a discovered skill came from, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// A `.zo/skills` root inside the project (or a walk-up ancestor).
    ProjectZo,
    /// A Zo global skill home (`ZO_CONFIG_HOME`/`ZO_HOME`/`~/.zo`).
    GlobalZo,
    /// Claude Code direct user skills (`~/.claude/skills`).
    ClaudeUser,
    /// An enabled Claude Code plugin's skills, tagged with its plugin id.
    ClaudePlugin { id: String },
    /// Codex direct user skills (`~/.codex/skills`).
    CodexUser,
    /// An enabled Codex plugin's skills, tagged with its plugin id.
    CodexPlugin { id: String },
}

impl SkillSource {
    /// Precedence rank, lowest wins. Claude user/plugin share a tier, as do
    /// Codex user/plugin, matching the documented ordering.
    #[must_use]
    fn precedence(&self) -> u8 {
        match self {
            Self::ProjectZo => 0,
            Self::GlobalZo => 1,
            Self::ClaudeUser | Self::ClaudePlugin { .. } => 2,
            Self::CodexUser | Self::CodexPlugin { .. } => 3,
        }
    }

    /// Whether this is a provider (non-Zo) source subject to canonicalization
    /// and containment checks.
    #[must_use]
    fn is_provider(&self) -> bool {
        !matches!(self, Self::ProjectZo | Self::GlobalZo)
    }

    /// Stable label surfaced as the `Skill` tool's `origin` output field.
    #[must_use]
    pub fn origin_label(&self) -> &'static str {
        match self {
            Self::ProjectZo => "project-zo",
            Self::GlobalZo => "global-zo",
            Self::ClaudeUser => "claude-user",
            Self::ClaudePlugin { .. } => "claude-plugin",
            Self::CodexUser => "codex-user",
            Self::CodexPlugin { .. } => "codex-plugin",
        }
    }
}

/// One discovered skill directory: its directory slug (the name used to invoke
/// it), the resolved `SKILL.md` path, and the source it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCandidate {
    /// Directory slug (`<root>/<dir_name>/SKILL.md`) — the invocation name.
    pub dir_name: String,
    /// Path to the `SKILL.md` body. Canonicalized for provider sources.
    pub skill_md: PathBuf,
    /// The trusted source this candidate was discovered under.
    pub source: SkillSource,
}

/// The ordered, trust-checked set of skill candidates for a working directory.
///
/// Candidates are stored highest-precedence first and in a deterministic order
/// within each source, so [`Self::resolve`] returns the winning skill for a
/// name collision and callers can build a stable index.
#[derive(Debug, Clone, Default)]
pub struct SkillCatalog {
    candidates: Vec<SkillCandidate>,
}

impl SkillCatalog {
    /// Discover every trusted skill candidate for `cwd`, in precedence order.
    #[must_use]
    pub fn discover(cwd: &Path) -> Self {
        let mut candidates = Vec::new();
        for (source, root) in scan_roots(cwd) {
            push_candidates_in_root(&source, &root, &mut candidates);
        }

        let mut seen = BTreeSet::new();
        candidates.retain(|candidate| seen.insert(candidate.dir_name.to_ascii_lowercase()));
        Self { candidates }
    }

    /// All candidates, highest precedence first (proposed drafts included, so
    /// the loader can still surface the "must be approved" error).
    #[must_use]
    pub fn candidates(&self) -> &[SkillCandidate] {
        &self.candidates
    }

    /// The winning candidate for `requested` (case-insensitive on the directory
    /// slug), or `None`. Because candidates are precedence-ordered this is the
    /// highest-precedence match — a Zo skill wins any provider collision.
    #[must_use]
    pub fn resolve(&self, requested: &str) -> Option<&SkillCandidate> {
        let requested = requested.trim();
        self.candidates
            .iter()
            .find(|candidate| candidate.dir_name.eq_ignore_ascii_case(requested))
    }
}

/// The trusted `(source, skills-dir)` pairs for `cwd`, in precedence order:
/// Zo project/global first, then enabled Claude, then enabled Codex.
/// Env switch for the borrowed provider catalogs. Default **off**: zo indexes
/// its own skills, and the machine's Claude Code / Codex skill directories are
/// somebody else's product surface.
pub const PROVIDER_SKILLS_ENV: &str = "ZO_PROVIDER_SKILLS";

/// Whether to also scan the local Claude Code / Codex skill catalogs.
///
/// Off by default, and the reason is a measurement: on this developer's machine
/// the borrowed catalogs added **1,204 tokens** to the skill index — pushing it
/// from 178 under its budget to 1,026 over — and **1,815 provider tokens** to
/// every single request, for skills the user never wrote and cannot see from
/// inside zo. A prompt that grows with whatever another product happens to have
/// installed is not a harness anyone can reason about.
///
/// `ZO_PROVIDER_SKILLS=1` brings them back for a machine that wants them; the
/// precedence order is unchanged (project Zo → global Zo → Claude → Codex), so
/// a zo skill still wins a name collision either way.
#[must_use]
pub fn provider_skills_enabled(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn scan_roots(cwd: &Path) -> Vec<(SkillSource, PathBuf)> {
    // Env is read here, at the edge, so the composition below stays a pure
    // function the tests can ask directly.
    scan_roots_with(
        cwd,
        provider_skills_enabled(std::env::var(PROVIDER_SKILLS_ENV).ok().as_deref()),
    )
}

fn scan_roots_with(cwd: &Path, providers: bool) -> Vec<(SkillSource, PathBuf)> {
    let mut roots = Vec::new();
    roots.extend(zo_scan_roots(cwd));
    if providers {
        roots.extend(claude_scan_roots());
        roots.extend(codex_scan_roots());
    }
    roots.sort_by_key(|(source, _)| source.precedence());
    roots
}

/// Tag each Zo skills root (from the shared walk) as project- or global-scoped.
fn zo_scan_roots(cwd: &Path) -> Vec<(SkillSource, PathBuf)> {
    let global: Vec<PathBuf> = crate::config::zo_global_config_roots()
        .into_iter()
        .map(|root| root.join("skills"))
        .collect();
    crate::prompt::skill_search_roots(cwd)
        .into_iter()
        .map(|root| {
            let source = if global.iter().any(|global_root| global_root == &root) {
                SkillSource::GlobalZo
            } else {
                SkillSource::ProjectZo
            };
            (source, root)
        })
        .collect()
}

/// Resolve a provider home from its test override, else `$HOME/<subdir>`.
/// Returns `None` when the override is explicitly empty or `HOME` is unset, so
/// provider discovery contributes nothing rather than guessing.
fn provider_home(override_env: &str, subdir: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(override_env) {
        if value.is_empty() {
            return None;
        }
        return Some(PathBuf::from(value));
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join(subdir))
}

/// Enabled Claude scan roots: the direct user root plus each enabled plugin's
/// `<installPath>/skills`, ordered by plugin id for determinism.
fn claude_scan_roots() -> Vec<(SkillSource, PathBuf)> {
    let Some(home) = provider_home(CLAUDE_HOME_ENV, ".claude") else {
        return Vec::new();
    };
    let mut roots = vec![(SkillSource::ClaudeUser, home.join("skills"))];

    let enabled = parse_claude_enabled_plugins(&home);
    let installed = parse_claude_installed_plugins(&home);
    for (id, install_path) in installed {
        if enabled.contains(&id) && install_path.is_absolute() {
            roots.push((
                SkillSource::ClaudePlugin { id },
                install_path.join("skills"),
            ));
        }
    }
    roots
}

/// Claude plugin ids explicitly enabled in `settings.json` `enabledPlugins`.
/// Defensive: any read/parse failure yields an empty set.
fn parse_claude_enabled_plugins(home: &Path) -> std::collections::BTreeSet<String> {
    let mut enabled = std::collections::BTreeSet::new();
    let Some(value) = read_json(&home.join("settings.json")) else {
        return enabled;
    };
    if let Some(map) = value.get("enabledPlugins").and_then(|v| v.as_object()) {
        for (id, flag) in map {
            if flag.as_bool() == Some(true) {
                enabled.insert(id.clone());
            }
        }
    }
    enabled
}

/// Claude plugin id → `installPath` from `plugins/installed_plugins.json`.
/// Tolerates either a flat `{ id: { installPath } }` object or one nested under
/// a `plugins` key, and an id whose value is the install-path string directly.
/// Defensive: any read/parse failure yields an empty map.
fn parse_claude_installed_plugins(home: &Path) -> BTreeMap<String, PathBuf> {
    let mut installed = BTreeMap::new();
    let path = home.join("plugins").join("installed_plugins.json");
    let Some(value) = read_json(&path) else {
        return installed;
    };
    let map = value
        .get("plugins")
        .and_then(|v| v.as_object())
        .or_else(|| value.as_object());
    let Some(map) = map else {
        return installed;
    };
    for (id, entry) in map {
        if let Some(install_path) = install_path_from_entry(entry) {
            installed.insert(id.clone(), install_path);
        }
    }
    installed
}

/// Extract an `installPath` from a registry entry. Claude Code v2 stores an
/// array of installation records per plugin; older/test fixtures may use one
/// record or a bare path. The last usable array record is the active one.
fn install_path_from_entry(entry: &serde_json::Value) -> Option<PathBuf> {
    match entry {
        serde_json::Value::String(text) => {
            (!text.is_empty()).then(|| PathBuf::from(text))
        }
        serde_json::Value::Array(entries) => {
            entries.iter().rev().find_map(install_path_from_entry)
        }
        serde_json::Value::Object(fields) => fields
            .get("installPath")
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
            .map(PathBuf::from),
        _ => None,
    }
}

/// Enabled Codex scan roots: direct user skills plus the newest cached version
/// of each plugin explicitly enabled in `config.toml`. Current Codex ids use
/// `name@marketplace` and cache under
/// `<home>/plugins/cache/<marketplace>/<name>/<version>/skills`.
fn codex_scan_roots() -> Vec<(SkillSource, PathBuf)> {
    let Some(home) = provider_home(CODEX_HOME_ENV, ".codex") else {
        return Vec::new();
    };
    let mut roots = vec![(SkillSource::CodexUser, home.join("skills"))];
    for id in parse_codex_enabled_plugins(&home) {
        let plugin_roots = if let Some((name, marketplace)) = codex_plugin_coordinates(&id) {
            newest_codex_cache_root(&home, name, marketplace)
                .into_iter()
                .collect::<Vec<_>>()
        } else if safe_provider_component(&id) {
            // Older fixtures/installations used one direct directory per id.
            vec![home.join("plugins").join(&id).join("skills")]
        } else {
            Vec::new()
        };
        for root in plugin_roots {
            roots.push((SkillSource::CodexPlugin { id: id.clone() }, root));
        }
    }
    roots
}

fn codex_plugin_coordinates(id: &str) -> Option<(&str, &str)> {
    let (name, marketplace) = id.rsplit_once('@')?;
    (safe_provider_component(name) && safe_provider_component(marketplace))
        .then_some((name, marketplace))
}

fn safe_provider_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn newest_codex_cache_root(home: &Path, name: &str, marketplace: &str) -> Option<PathBuf> {
    let plugin_root = home
        .join("plugins")
        .join("cache")
        .join(marketplace)
        .join(name);
    let mut skill_roots = std::fs::read_dir(plugin_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("skills"))
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    skill_roots.sort_by(|left, right| {
        let left_modified = std::fs::metadata(left).and_then(|meta| meta.modified()).ok();
        let right_modified = std::fs::metadata(right)
            .and_then(|meta| meta.modified())
            .ok();
        right_modified
            .cmp(&left_modified)
            .then_with(|| right.cmp(left))
    });
    skill_roots.into_iter().next()
}

/// Codex plugin ids with `enabled = true` under a `[plugins."id"]` table in
/// `config.toml`. A deliberately minimal, dependency-free scan (no TOML crate is
/// available to this crate): it recognises `[plugins."id"]` / `[plugins.id]`
/// section headers and a following `enabled = true`. Defensive — anything it
/// cannot interpret simply yields no enabled ids.
fn parse_codex_enabled_plugins(home: &Path) -> std::collections::BTreeSet<String> {
    let mut enabled = std::collections::BTreeSet::new();
    let Ok(contents) = std::fs::read_to_string(home.join("config.toml")) else {
        return enabled;
    };
    let mut current: Option<String> = None;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(id) = parse_codex_plugin_header(trimmed) {
            current = Some(id);
            continue;
        }
        if trimmed.starts_with('[') {
            // Any other table header ends the current plugin scope.
            current = None;
            continue;
        }
        if let Some(id) = &current {
            if let Some((key, value)) = trimmed.split_once('=') {
                if key.trim() == "enabled" && value.trim() == "true" {
                    enabled.insert(id.clone());
                }
            }
        }
    }
    enabled
}

/// Extract the plugin id from a `[plugins."id"]` / `[plugins.id]` table header.
fn parse_codex_plugin_header(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let rest = inner.strip_prefix("plugins.")?;
    let id = rest.trim().trim_matches('"');
    (!id.is_empty()).then(|| id.to_string())
}

/// Read and JSON-parse a file, returning `None` on any read/parse failure.
fn read_json(path: &Path) -> Option<serde_json::Value> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Enumerate skill directories under one trusted root, appending candidates in
/// sorted (deterministic) order. Provider roots additionally require the
/// candidate `SKILL.md` to canonicalize to a path contained by the canonical
/// root, rejecting symlink escapes.
fn push_candidates_in_root(
    source: &SkillSource,
    root: &Path,
    out: &mut Vec<SkillCandidate>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    dirs.sort();

    let containment_base = if source.is_provider() {
        match std::fs::canonicalize(root) {
            Ok(base) => Some(base),
            // A root we cannot canonicalize cannot be containment-checked, so we
            // trust nothing under it rather than risk an escape.
            Err(_) => return,
        }
    } else {
        None
    };

    for dir in dirs {
        let skill_md = dir.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let Some(dir_name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let resolved = match &containment_base {
            Some(base) => match std::fs::canonicalize(&skill_md) {
                Ok(canonical) if canonical.starts_with(base) => canonical,
                _ => continue,
            },
            None => skill_md,
        };

        out.push(SkillCandidate {
            dir_name: dir_name.to_string(),
            skill_md: resolved,
            source: source.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    static PROVIDER_ENV_LOCK: Mutex<()> = Mutex::new(());
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zo-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// The default is zo's own catalog, and nothing else.
    ///
    /// Measured on a developer machine: the borrowed Claude Code / Codex skill
    /// directories added **1,204 tokens** to the skill index — 178 under budget
    /// became 1,026 over — and **1,815 provider tokens** to every request, for
    /// skills the user never wrote and cannot see from inside zo. A prompt that
    /// grows with whatever another product happens to have installed is not a
    /// harness anyone can reason about.
    ///
    /// Nothing pinned this before: the catalog's tests all check its *parts*
    /// (precedence, plugin parsing), so flipping the default broke none of them.
    #[test]
    fn by_default_only_zo_roots_are_scanned() {
        let cwd = temp_root("skills-default");
        for (source, root) in super::scan_roots_with(&cwd, false) {
            assert!(
                matches!(source, SkillSource::ProjectZo | SkillSource::GlobalZo),
                "a borrowed catalog reached the default scan: {source:?} at {}",
                root.display()
            );
        }
    }

    /// Turning providers on may only ADD — it must never displace zo.
    ///
    /// Stated as a property of ONE scan, not a diff of two. `HOME` and the zo
    /// config roots are process-global, and other tests move them; comparing a
    /// `false` scan against a `true` scan taken moments later compares two
    /// different environments and fails for a reason that has nothing to do
    /// with this switch (measured: a peer test's `HOME` appeared between the
    /// two calls and added a `ProjectZo` root).
    ///
    /// The invariant that actually matters is the ordering: every zo root sorts
    /// ahead of every borrowed one, which is what makes a zo skill win a name
    /// collision when the catalogs are enabled.
    #[test]
    fn enabling_providers_never_puts_one_ahead_of_a_zo_root() {
        let cwd = temp_root("skills-opt-in");
        let roots = super::scan_roots_with(&cwd, true);
        let first_provider = roots.iter().position(|(source, _)| {
            !matches!(source, SkillSource::ProjectZo | SkillSource::GlobalZo)
        });
        let Some(first_provider) = first_provider else {
            return; // No provider homes on this machine — nothing to order.
        };
        assert!(
            roots[first_provider..].iter().all(|(source, _)| {
                !matches!(source, SkillSource::ProjectZo | SkillSource::GlobalZo)
            }),
            "a zo root sorted behind a borrowed one: {roots:?}"
        );
    }

    /// The switch reads as a switch: only an affirmative word turns it on, and
    /// anything else — unset, empty, `0`, garbage — leaves the default.
    #[test]
    fn the_provider_switch_needs_an_affirmative_word() {
        for on in ["1", "true", "yes", "on", "ON", " True "] {
            assert!(
                super::provider_skills_enabled(Some(on)),
                "{on:?} should enable"
            );
        }
        for off in ["0", "false", "no", "off", "", "  ", "maybe"] {
            assert!(
                !super::provider_skills_enabled(Some(off)),
                "{off:?} must not enable"
            );
        }
        assert!(!super::provider_skills_enabled(None), "unset is the default");
    }

    #[test]
    fn precedence_orders_zo_above_providers() {
        assert!(SkillSource::ProjectZo.precedence() < SkillSource::GlobalZo.precedence());
        assert!(
            SkillSource::GlobalZo.precedence()
                < SkillSource::ClaudeUser.precedence()
        );
        assert!(
            SkillSource::ClaudePlugin {
                id: "x".to_string()
            }
            .precedence()
                < SkillSource::CodexUser.precedence()
        );
    }

    #[test]
    fn codex_header_parses_quoted_and_bare_ids() {
        assert_eq!(
            parse_codex_plugin_header("[plugins.\"beta\"]").as_deref(),
            Some("beta")
        );
        assert_eq!(
            parse_codex_plugin_header("[plugins.gamma]").as_deref(),
            Some("gamma")
        );
        assert_eq!(parse_codex_plugin_header("[tools.other]"), None);
    }

    #[test]
    fn resolve_returns_highest_precedence_on_name_collision() {
        let catalog = SkillCatalog {
            candidates: vec![
                SkillCandidate {
                    dir_name: "shared".to_string(),
                    skill_md: PathBuf::from("/zo/shared/SKILL.md"),
                    source: SkillSource::ProjectZo,
                },
                SkillCandidate {
                    dir_name: "shared".to_string(),
                    skill_md: PathBuf::from("/claude/shared/SKILL.md"),
                    source: SkillSource::ClaudeUser,
                },
            ],
        };
        let winner = catalog.resolve("SHARED").expect("case-insensitive match");
        assert_eq!(winner.source, SkillSource::ProjectZo);
    }

    #[test]
    fn claude_installed_plugin_array_uses_latest_record() {
        let entry = serde_json::json!([
            { "installPath": "/cache/plugin/1.0.0" },
            { "installPath": "/cache/plugin/2.0.0" }
        ]);

        assert_eq!(
            install_path_from_entry(&entry),
            Some(PathBuf::from("/cache/plugin/2.0.0"))
        );
    }

    #[test]
    fn codex_enabled_plugin_uses_marketplace_cache_layout() {
        let _env_lock = crate::test_env_lock(); // Writes process env: hold the crate env lock so a parallel test never sees it mid-change.

        let _lock = PROVIDER_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = temp_root("codex-cache-layout");
        std::fs::create_dir_all(&root).expect("temp root");
        let _env = EnvVarGuard::set(CODEX_HOME_ENV, &root);
        std::fs::write(
            root.join("config.toml"),
            "[plugins.\"documents@openai-primary-runtime\"]\nenabled = true\n",
        )
        .expect("config");
        let expected = root
            .join("plugins")
            .join("cache")
            .join("openai-primary-runtime")
            .join("documents")
            .join("2.0.0")
            .join("skills");
        std::fs::create_dir_all(&expected).expect("cache skills root");

        let roots = codex_scan_roots();
        assert!(roots.iter().any(|(source, path)| {
            matches!(source, SkillSource::CodexPlugin { id } if id == "documents@openai-primary-runtime")
                && path == &expected
        }));

        std::fs::remove_dir_all(root).expect("cleanup temp root");
    }

    #[test]
    fn codex_plugin_id_with_path_components_is_rejected() {
        let _env_lock = crate::test_env_lock(); // Writes process env: hold the crate env lock so a parallel test never sees it mid-change.

        let _lock = PROVIDER_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = temp_root("codex-path-escape");
        std::fs::create_dir_all(&root).expect("temp root");
        let _env = EnvVarGuard::set(CODEX_HOME_ENV, &root);
        std::fs::write(
            root.join("config.toml"),
            "[plugins.\"../../outside@marketplace\"]\nenabled = true\n",
        )
        .expect("config");

        let roots = codex_scan_roots();
        assert_eq!(
            roots.len(),
            1,
            "an invalid plugin id must not add a root beyond direct user skills"
        );

        std::fs::remove_dir_all(root).expect("cleanup temp root");
    }
}
