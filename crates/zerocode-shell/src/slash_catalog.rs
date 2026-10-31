//! The slash commands a pane's CLI takes right now, and the models it can
//! switch to — the two lists behind the composer's `/` palette and its
//! agent·model chip (docs/design/agent-conversation-claude-code-grammar-20260915.md
//! §4, 2026-09-15 "slash 명령어가 gui에서도 cli처럼 나와야함 … 실제 지금
//! 터미널에서 사용할수있는 명령어").
//!
//! "Actually available" is the rule, so every list names its source and the
//! sources are, in order of honesty:
//!
//! - **the binary itself, asked**: Claude Code announces its session in
//!   `--output-format stream-json` with `slash_commands` (built-ins, skills,
//!   plugin commands, MCP prompts — the exact set this installation offers in
//!   this checkout); zo prints `zo commands --json`. The process is killed the
//!   moment the announcement is read.
//!   Antigravity prints its list when asked `agy --print /help` (name, alias,
//!   description — the same rows its own `/help` shows).
//! - **the binary itself, read**: Claude Code's bundle carries each command's
//!   description beside its name; those give the words for the built-ins the
//!   announcement only named.
//! - **files the CLI reads**: `SKILL.md` and `commands/*.md` frontmatter
//!   (Claude Code), `~/.codex/prompts/*.md` (Codex) — the descriptions of what
//!   a person added.
//! - **the vendor's reference page, as a stamped snapshot**: Codex keeps its
//!   catalog inside a Rust TUI with no listing, so `codex-commands.json`
//!   (tools/agents/codex_slash_commands.py) stands in, and the palette says
//!   so (`source: docs`, with the version the page was read against).
//!
//! Nothing here invents a command. An agent outside the table answers an
//! empty catalog, and the palette then shows only the window's own commands.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// One row of the palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    /// With its slash, as the CLI spells it: `/compact`, `/codex:review`.
    pub name: String,
    /// The CLI's own words for it; empty when no source carried any.
    #[serde(default)]
    pub about: String,
    /// The argument hint the CLI shows, when it shows one (`[instructions]`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub args: String,
    /// `builtin`, `skill`, `plugin`, `prompt`, `mcp` — the palette groups by it.
    pub kind: &'static str,
}

/// The palette's answer for one agent in one checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlashCatalog {
    pub agent: String,
    /// Where the names came from: `session` (the binary announced them),
    /// `binary` (read off the installed bundle), `docs` (the stamped snapshot),
    /// `none` (an agent this module has no road to).
    pub source: &'static str,
    /// The CLI version the list belongs to, when the source said — for a
    /// documented list, the version installed here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// For a documented list: the version the page was read against, when it
    /// is not the one installed — the palette shows both, so a list a release
    /// behind is never passed off as the installed binary's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_version: Option<String>,
    pub commands: Vec<SlashCommand>,
}

/// One model the agent·model chip can offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelRow {
    pub id: String,
    pub display_name: String,
    pub provider: String,
}

/// How long an answer stands before the CLI is asked again. Announcing a
/// Claude Code session costs seconds and starts a request it never finishes;
/// a person opening the palette twice in a minute should not pay twice.
const CATALOG_TTL: Duration = Duration::from_secs(300);
/// The model catalog is zo's own cache with its own TTL; this only spares the
/// process spawn on a menu opened again and again.
const MODELS_TTL: Duration = Duration::from_secs(60);
/// How long `zo models --json` may run before the window ends it and keeps
/// the catalog it last read (t-45961: it had no limit, and a judgment
/// service that never answered held the model menu and the lineup read with
/// it). Past zo's own walls with room to spare — it asks its six model
/// sources in turn, each under a 6-second timeout, then the tier seat's
/// lineups together under one wall
/// ([`zerocode_core::jev::MODEL_TIERS_DEADLINE_MS`]) — so only a zo that is
/// stuck meets it, never one that is slow.
const MODELS_WAIT: Duration = Duration::from_secs(60);
/// How long the Claude Code announcement may take before the probe gives up.
const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(15);
/// The prompt the probe sends so the session starts; killed at the
/// announcement, before any answer. The words do not matter.
const PROBE_PROMPT: &str = "hi";

/// Answers by (agent, checkout), each with the moment it was read.
type CatalogCache = HashMap<(String, String), (Instant, SlashCatalog)>;
/// zo's model catalog (`zo models --json`, whole), with the moment it was
/// read — none once the window forgot it ([`forget_zo_models`]) — and how
/// many times the window forgot it: a read that began before a forget does
/// not keep what it read (t-76028 R2).
#[derive(Default)]
struct ModelsCache {
    forgotten: u64,
    force_refresh: bool,
    held: Option<(Option<Instant>, serde_json::Value)>,
}

fn catalog_cache() -> &'static Mutex<CatalogCache> {
    static CACHE: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn models_cache() -> &'static Mutex<ModelsCache> {
    static CACHE: OnceLock<Mutex<ModelsCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ModelsCache::default()))
}

/// The commands `agent` takes in `cwd`, cached for [`CATALOG_TTL`].
pub fn slash_catalog(
    agent: &str,
    cwd: &Path,
    home: &Path,
    path_var: Option<&OsStr>,
    fresh: bool,
) -> SlashCatalog {
    let key = (agent.to_string(), cwd.to_string_lossy().into_owned());
    // `fresh` skips the held answer — after a person edits their commands, on
    // the palette's explicit refresh — and the rebuilt one takes its place.
    if !fresh
        && let Ok(cache) = catalog_cache().lock()
        && let Some((at, held)) = cache.get(&key)
        && at.elapsed() < CATALOG_TTL
    {
        return held.clone();
    }
    let fresh = build_catalog(agent, cwd, home, path_var);
    if let Ok(mut cache) = catalog_cache().lock() {
        cache.insert(key, (Instant::now(), fresh.clone()));
    }
    fresh
}

fn build_catalog(agent: &str, cwd: &Path, home: &Path, path_var: Option<&OsStr>) -> SlashCatalog {
    match agent {
        "claude" => claude_catalog(cwd, home, path_var),
        "zo" => zo_catalog(home),
        "codex" => codex_catalog(home, path_var),
        "antigravity" => antigravity_catalog(path_var),
        _ => SlashCatalog {
            agent: agent.to_string(),
            source: "none",
            version: None,
            snapshot_version: None,
            commands: Vec::new(),
        },
    }
}

// ---- Claude Code ------------------------------------------------------------

/// What Claude Code's `system/init` announcement carries that this module
/// reads. Names only; the words come from the bundle and the files.
#[derive(Debug, Default, Deserialize)]
struct ClaudeInit {
    #[serde(default)]
    slash_commands: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    plugins: Vec<ClaudePlugin>,
    #[serde(default)]
    claude_code_version: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ClaudePlugin {
    name: String,
    path: PathBuf,
}

fn claude_catalog(cwd: &Path, home: &Path, path_var: Option<&OsStr>) -> SlashCatalog {
    let binary = zerocode_core::agent::resolve_on_path(path_var, "claude");
    let announced = binary
        .as_deref()
        .and_then(|binary| announce_claude(binary, cwd));
    let Some(init) = announced else {
        return SlashCatalog {
            agent: "claude".into(),
            source: "none",
            version: None,
            snapshot_version: None,
            commands: Vec::new(),
        };
    };
    let words = binary
        .as_deref()
        .map(|binary| bundle_words(binary, r#"name:"([a-z][a-z0-9-]{1,40})",(?:[^{}"]|"(?:[^"\\]|\\.)*")*?description:"((?:[^"\\]|\\.){3,240})""#))
        .unwrap_or_default();
    let commands = init
        .slash_commands
        .iter()
        .map(|name| {
            let kind = if name.starts_with("mcp__") {
                "mcp"
            } else if name.contains(':') {
                "plugin"
            } else if init.skills.iter().any(|skill| skill == name) {
                "skill"
            } else {
                "builtin"
            };
            let about = match kind {
                "builtin" => words.get(name.as_str()).cloned().unwrap_or_default(),
                "skill" | "plugin" => claude_file_words(name, cwd, home, &init.plugins),
                _ => String::new(),
            };
            SlashCommand {
                name: format!("/{name}"),
                about,
                args: String::new(),
                kind,
            }
        })
        .collect();
    SlashCatalog {
        agent: "claude".into(),
        source: "session",
        version: init.claude_code_version,
        snapshot_version: None,
        commands,
    }
}

/// Start a print-mode session in `cwd`, read its announcement, kill it.
///
/// The environment loses every `CLAUDE*` and `ZEROCODE*` variable: the probe
/// is not a nested session of the pane that asked, and the hook endpoint
/// belongs to that pane alone.
fn announce_claude(binary: &Path, cwd: &Path) -> Option<ClaudeInit> {
    let mut command = crate::proc::quiet_command(binary);
    command
        .args([
            "-p",
            PROBE_PROMPT,
            "--output-format",
            "stream-json",
            "--verbose",
            "--max-turns",
            "1",
        ])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("CLAUDE") || key.starts_with("ZEROCODE") {
            command.env_remove(&*key);
        }
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let started = Instant::now();
    let found = std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            while let Some(Ok(line)) = lines.next() {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if value.get("type").and_then(serde_json::Value::as_str) == Some("system")
                    && value.get("subtype").and_then(serde_json::Value::as_str) == Some("init")
                {
                    return serde_json::from_value::<ClaudeInit>(value).ok();
                }
            }
            None
        });
        loop {
            if reader.is_finished() {
                break;
            }
            if started.elapsed() > ANNOUNCE_TIMEOUT {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        reader.join().ok().flatten()
    });
    let _ = child.wait();
    found
}

/// The frontmatter `description` of the file behind a skill or command.
fn claude_file_words(name: &str, cwd: &Path, home: &Path, plugins: &[ClaudePlugin]) -> String {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some((plugin, rest)) = name.split_once(':')
        && let Some(found) = plugins.iter().find(|one| one.name == plugin)
    {
        candidates.push(found.path.join("skills").join(rest).join("SKILL.md"));
        candidates.push(found.path.join("commands").join(format!("{rest}.md")));
    }
    for root in [cwd.join(".claude"), home.join(".claude")] {
        candidates.push(root.join("skills").join(name).join("SKILL.md"));
        candidates.push(
            root.join("commands")
                .join(format!("{}.md", name.replace(':', "/"))),
        );
    }
    candidates
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| frontmatter_value(&text, "description"))
        .unwrap_or_default()
}

// ---- zo ---------------------------------------------------------------------

fn zo_catalog(home: &Path) -> SlashCatalog {
    let bin = crate::zo_companion::zo_path_under(home);
    let rows = crate::proc::quiet_command(&bin)
        .args(["commands", "--json"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout).ok());
    let Some(rows) = rows else {
        return SlashCatalog {
            agent: "zo".into(),
            source: "none",
            version: None,
            snapshot_version: None,
            commands: Vec::new(),
        };
    };
    let commands = rows
        .iter()
        .filter_map(|row| {
            Some(SlashCommand {
                name: row.get("name")?.as_str()?.to_string(),
                about: row
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                args: String::new(),
                kind: "builtin",
            })
        })
        .collect();
    SlashCatalog {
        agent: "zo".into(),
        source: "session",
        version: None,
        snapshot_version: None,
        commands,
    }
}

// ---- Codex ------------------------------------------------------------------

/// The stamped reference snapshot (tools/agents/codex_slash_commands.py).
#[derive(Debug, Deserialize)]
struct CodexSnapshot {
    #[serde(default)]
    cli_version_observed: Option<String>,
    #[serde(default)]
    commands: Vec<CodexSnapshotRow>,
}

#[derive(Debug, Deserialize)]
struct CodexSnapshotRow {
    name: String,
    #[serde(default)]
    args: String,
    #[serde(default)]
    about: String,
}

fn codex_catalog(home: &Path, path_var: Option<&OsStr>) -> SlashCatalog {
    let snapshot: CodexSnapshot = serde_json::from_str(include_str!("slash/codex-commands.json"))
        .unwrap_or(CodexSnapshot {
            cli_version_observed: None,
            commands: Vec::new(),
        });
    let mut commands: Vec<SlashCommand> = snapshot
        .commands
        .into_iter()
        .map(|row| SlashCommand {
            name: row.name,
            about: row.about,
            args: row.args,
            kind: "builtin",
        })
        .collect();
    // `~/.codex/prompts/<name>.md` is `/prompts:<name>`; its frontmatter
    // `description` or first line are its words.
    for (stem, text) in markdown_files(&home.join(".codex").join("prompts")) {
        commands.push(SlashCommand {
            name: format!("/prompts:{stem}"),
            about: frontmatter_value(&text, "description")
                .or_else(|| first_prose_line(&text))
                .unwrap_or_default(),
            args: String::new(),
            kind: "prompt",
        });
    }
    // The installed Codex, read off the package the `codex` shim belongs to
    // (`bin/codex.js` → `package.json`): the head names it, and names the
    // page's version beside it when the two differ.
    let installed = zerocode_core::agent::resolve_on_path(path_var, "codex")
        .and_then(|shim| codex_package_version(&shim));
    let observed = snapshot.cli_version_observed;
    let snapshot_version = match (&installed, &observed) {
        (Some(here), Some(read)) if here != read => Some(read.clone()),
        (None, Some(read)) => Some(read.clone()),
        _ => None,
    };
    SlashCatalog {
        agent: "codex".into(),
        source: "docs",
        version: installed.or(observed),
        snapshot_version,
        commands,
    }
}

/// `"version"` from the `package.json` two steps above the `codex` shim
/// (`…/@openai/codex/bin/codex.js`), following the symlink the package
/// manager laid.
fn codex_package_version(shim: &Path) -> Option<String> {
    let real = std::fs::canonicalize(shim).ok()?;
    let package = real.parent()?.parent()?.join("package.json");
    let text = std::fs::read_to_string(package).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(str::to_string)
}

// ---- Antigravity ------------------------------------------------------------

/// How long `agy --print /help` may take: the binary boots its stores first
/// (6 s measured 2026-09-15, 1.2.3) and answers locally.
const AGY_HELP_WAIT: Duration = Duration::from_secs(30);

/// Antigravity's commands are the ones it prints for `/help` — `agy --print
/// /help --output-format text` lists them as `name (alias)<TAB>description`,
/// one per line, the same rows the interactive `/help` shows. Asked of the
/// installed binary, so the list is this installation's, and held for the
/// catalog's TTL like Claude Code's announcement.
fn antigravity_catalog(path_var: Option<&OsStr>) -> SlashCatalog {
    let binary = zerocode_core::agent::resolve_on_path(path_var, "agy");
    let lines = binary.as_deref().and_then(|binary| {
        captured_lines(
            binary,
            &["--print", "/help", "--output-format", "text"],
            AGY_HELP_WAIT,
        )
    });
    let version = binary
        .as_deref()
        .and_then(|binary| captured_lines(binary, &["--version"], AGY_HELP_WAIT))
        .and_then(|lines| lines.into_iter().find(|line| !line.trim().is_empty()))
        .map(|line| line.trim().to_string());
    let commands: Vec<SlashCommand> = lines
        .as_deref()
        .map(|lines| lines.iter().filter_map(|line| agy_help_row(line)).collect())
        .unwrap_or_default();
    SlashCatalog {
        agent: "antigravity".into(),
        source: if commands.is_empty() {
            "none"
        } else {
            "session"
        },
        version,
        snapshot_version: None,
        commands,
    }
}

/// One row of `agy /help`: `/config (settings)\tOpen settings panel` — the
/// name, its alias in parentheses when it has one, a tab, the words.
fn agy_help_row(line: &str) -> Option<SlashCommand> {
    let (head, about) = line.split_once('\t')?;
    let head = head.trim();
    if !head.starts_with('/') {
        return None;
    }
    let (name, alias) = match head.split_once(" (") {
        Some((name, rest)) => (name.trim(), rest.trim_end_matches(')').trim()),
        None => (head, ""),
    };
    Some(SlashCommand {
        name: name.to_string(),
        about: if alias.is_empty() {
            about.trim().to_string()
        } else {
            format!("{} (/{alias})", about.trim())
        },
        args: String::new(),
        kind: "builtin",
    })
}

/// A short-lived command's stdout as lines, or `None` when it fails or
/// outlives `wait` — the child is killed then, never left behind.
pub(crate) fn captured_lines(binary: &Path, args: &[&str], wait: Duration) -> Option<Vec<String>> {
    captured_lines_in(binary, args, wait, &[])
}

/// [`captured_lines`] with `env` set on the child after the window's own
/// `ZEROCODE*` names are taken away.
fn captured_lines_in(
    binary: &Path,
    args: &[&str],
    wait: Duration,
    env: &[(String, String)],
) -> Option<Vec<String>> {
    let mut command = crate::proc::quiet_command_with_env(binary, env);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("ZEROCODE") && !env.iter().any(|(given, _)| *given == *key) {
            command.env_remove(&*key);
        }
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let started = Instant::now();
    let lines = std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<String>>()
        });
        while !reader.is_finished() {
            if started.elapsed() > wait {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        reader.join().ok()
    });
    let status = child.wait().ok()?;
    lines.filter(|_| status.success())
}

// ---- models -----------------------------------------------------------------

/// The models `agent` can switch to, from zo's live catalog (`zo models
/// --json`, the one place this machine merges every provider's list),
/// filtered to the provider the agent drives, with withdrawn rows omitted and
/// active generations ordered ahead of superseded ones.
pub fn agent_models(agent: &str, home: &Path) -> Vec<ModelRow> {
    agent_models_from(agent, &zo_models_catalog(home))
}

fn band_rank(band: Option<&str>) -> u8 {
    match band {
        Some("top") => 0,
        Some("second") => 1,
        Some("rest") => 2,
        None => 3,
        Some("superseded") => 4,
        Some(_) => 3,
    }
}

fn agent_models_from(agent: &str, catalog: &serde_json::Value) -> Vec<ModelRow> {
    let other_provider = match agent {
        "grok" => Some("xai"),
        "kimi" => Some("kimi"),
        _ => None,
    };
    if let Some(wanted) = other_provider {
        return catalog
            .get("otherLogins")
            .and_then(serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|row| {
                let id = row.get("id")?.as_str()?;
                let row_provider = row.get("provider")?.as_str()?;
                (row_provider == wanted).then(|| ModelRow {
                    id: id.to_string(),
                    display_name: row
                        .get("displayName")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    provider: row_provider.to_string(),
                })
            })
            .collect();
    }

    let wanted_provider = if agent == "zo" {
        None
    } else {
        let Some(provider) = zerocode_core::agent::agent_voice(agent).models_provider else {
            return Vec::new();
        };
        Some(provider)
    };

    let rows = catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut ranked: Vec<(u8, usize, ModelRow)> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            if row
                .get("unlistedSince")
                .is_some_and(|since| !since.is_null())
            {
                return None;
            }
            let id = row.get("id")?.as_str()?;
            let row_provider = row.get("provider")?.as_str()?;
            if wanted_provider.is_some_and(|wanted| wanted != row_provider) {
                return None;
            }
            let rank = band_rank(row.get("band").and_then(serde_json::Value::as_str));
            Some((
                rank,
                index,
                ModelRow {
                    id: id.to_string(),
                    display_name: row
                        .get("displayName")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    provider: row_provider.to_string(),
                },
            ))
        })
        .collect();
    ranked.sort_by_key(|(rank, index, _)| (*rank, *index));
    let mut out: Vec<ModelRow> = ranked.into_iter().map(|(_, _, row)| row).collect();

    if agent == "zo" {
        let custom = catalog
            .get("customProviders")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|provider| {
                provider.get("usable").and_then(serde_json::Value::as_bool) == Some(true)
            });
        for provider in custom {
            let Some(name) = provider.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let models = provider
                .get("models")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str);
            for id in models {
                if out.iter().any(|existing| existing.id == id) {
                    continue;
                }
                let display_name = id
                    .strip_prefix(&format!("{name}/"))
                    .unwrap_or(id)
                    .to_string();
                out.push(ModelRow {
                    id: id.to_string(),
                    display_name,
                    provider: name.to_string(),
                });
            }
        }
    }
    out
}

/// Today's lineup for `agent` — the same catalog [`agent_models`] reads, with
/// the band, rungs and efforts zo's classifier put on each row: its one
/// provider's rows, or every provider's for an agent that drives none in
/// particular (zo, which runs any of them). `None` for a catalog with none
/// of its rows (t-14437).
pub fn agent_lineup(
    agent: &str,
    home: &Path,
) -> Option<zerocode_core::summon_difficulty::lineup::Lineup> {
    use zerocode_core::summon_difficulty::lineup::Lineup;
    let catalog = zo_models_catalog(home);
    if agent == "zo" {
        Lineup::from_catalog_all(&catalog)
    } else if let Some(provider) = zerocode_core::agent::agent_voice(agent).models_provider {
        Lineup::from_catalog(&catalog, provider)
    } else {
        None
    }
}

/// What the model settings draw from zo's list (t-75762): every login model
/// row, every router zo reads (`customProviders`), each source's report and
/// the aliases a saved `model` may name — the same cached catalog the model
/// menu and the lineup read, never a second `zo models` run — and where a
/// zo pane reads its Claude login (`paneCredentialStore`), so a report made
/// from another place can be said to be one (t-76598).
pub fn zo_model_list(home: &Path) -> serde_json::Value {
    let mut list = model_list_of(&zo_models_catalog(home));
    if let Some(root) = ACCOUNTS_ROOT.get() {
        list.insert(
            "paneCredentialStore".to_string(),
            serde_json::Value::String(credential_store(&lookup_env(&listing_env(root), |name| {
                std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
            }))),
        );
    }
    serde_json::Value::Object(list)
}

/// The settings' part of zo's catalog, as the screen may hold it — its
/// reports already carry a diagnosis in place of zo's words
/// ([`diagnosed`]).
fn model_list_of(catalog: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut list = serde_json::Map::new();
    for key in [
        "models",
        "otherLogins",
        "customProviders",
        "reports",
        "aliases",
    ] {
        list.insert(
            key.to_string(),
            catalog
                .get(key)
                .filter(|value| value.is_array())
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
        );
    }
    list
}

/// zo's catalog as the window keeps it (t-76873 P1): a report's `detail` is
/// what a provider answered — an HTTP error body zo already cut, where a
/// query token, a header's value on its next line or an address cut before
/// its `@` hides from any masking rule — so it never enters the window. A
/// failed report keeps only its diagnosis: the HTTP status and the kind of
/// failure zo names in its own fields, and whether the source was skipped
/// for want of a credential.
fn diagnosed(mut catalog: serde_json::Value) -> serde_json::Value {
    let reports = catalog
        .get_mut("reports")
        .and_then(serde_json::Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_object_mut);
    for report in reports {
        let detail = report.remove("detail");
        if report.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            continue;
        }
        let detail = detail
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let kind = field(report, "failureKind", "failure_kind");
        let status = field(report, "httpStatus", "http_status");
        report.remove("failureKind");
        report.remove("failure_kind");
        report.remove("httpStatus");
        report.remove("http_status");
        report.insert("diagnosis".to_string(), diagnosis_of(kind, status, detail));
    }
    catalog
}

/// A report's field as `zo models --json` spells it (camelCase) or its cache
/// file does (snake_case, which an older window reads the same way).
fn field(
    report: &serde_json::Map<String, serde_json::Value>,
    camel: &str,
    snake: &str,
) -> Option<serde_json::Value> {
    report.get(camel).or_else(|| report.get(snake)).cloned()
}

/// A failed report as the window reads it (t-76968): by the fields zo sets
/// for it — `failureKind` (one of zo's ten kinds) and `httpStatus` — and by
/// no word of the error text, whose wording the window cannot know: zo's
/// own advice, an address or a header's name can say "connect" or
/// "timeout" in a failure that was neither. A report without the kind is an
/// older zo's: it names none, and only zo's fixed `skipped:` head, which it
/// has always written, says a source was skipped for want of a credential.
/// A kind this window does not know is `unknown`, not a guess.
fn diagnosis_of(
    kind: Option<serde_json::Value>,
    status: Option<serde_json::Value>,
    detail: &str,
) -> serde_json::Value {
    let kind = kind.as_ref().and_then(serde_json::Value::as_str);
    let status = status
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        .filter(|status| (100..=599).contains(status));
    let named = match kind {
        None => None,
        Some("no_credentials") => None,
        Some("credential_unusable") => Some("credentialUnusable"),
        Some("auth_rejected") => Some("auth"),
        Some("forbidden") => Some("forbidden"),
        Some("not_found") => Some("notFound"),
        Some("rate_limited") => Some("rateLimit"),
        Some("server_error") => Some("server"),
        Some("network") => Some("network"),
        Some("timeout") => Some("timeout"),
        Some(_) => Some("unknown"),
    };
    let skipped = match kind {
        Some(kind) => kind == "no_credentials",
        None => detail.starts_with("skipped:"),
    };
    serde_json::json!({
        "status": status,
        "kind": named,
        "skipped": skipped,
    })
}

/// Where the window keeps its accounts, named once at boot. Until it is,
/// zo is asked with the window's own environment.
static ACCOUNTS_ROOT: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn configure_accounts(config_root: PathBuf) {
    let _ = ACCOUNTS_ROOT.set(config_root);
}

/// The account variables a zo pane is launched with (t-76598), read and
/// never written: the window's `zo models` read once ran with none, so it
/// alone found no Claude login, and wrote that failure over the list a pane
/// reads too. A look, not a launch — it runs on the list's clock, so nothing
/// is materialized and a signed-out account does not stop the read.
fn listing_env(config_root: &Path) -> Vec<(String, String)> {
    crate::usage_runtime::account_vars_for(config_root, "zo")
}

/// The Claude folder variables a lookup's zo ends with (t-76873 P2 ②): the
/// ones `env` names, and those it leaves out as the zo inherits them from
/// the window — the system's default account names none, and its zo reads
/// the folder the window was started with.
fn lookup_env(
    env: &[(String, String)],
    inherited: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    use zerocode_core::account::{CONFIG_DIR_VAR, SECURE_STORAGE_CONFIG_DIR_VAR};
    let mut ends = env.to_vec();
    for name in [SECURE_STORAGE_CONFIG_DIR_VAR, CONFIG_DIR_VAR] {
        if !env.iter().any(|(key, _)| key == name)
            && let Some(folder) = inherited(name)
        {
            ends.push((name.to_string(), folder));
        }
    }
    ends
}

/// Where a zo given `env` reads its Claude login, in the words zo's report
/// says it in (`credentialStore`), by zo's own rule (`ClaudeCodeStore::
/// from_folders`, ZO m-76774): the credential folder variable decides
/// whenever it is set — empty, it names the keychain — and only its absence
/// hands over to the config folder; no folder is the keychain.
fn credential_store(env: &[(String, String)]) -> String {
    use zerocode_core::account::{CONFIG_DIR_VAR, SECURE_STORAGE_CONFIG_DIR_VAR};
    let named = |name: &'static str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, folder)| (name, folder.as_str()))
    };
    named(SECURE_STORAGE_CONFIG_DIR_VAR)
        .or_else(|| named(CONFIG_DIR_VAR))
        .filter(|(_, folder)| !folder.is_empty())
        .map_or_else(
            || "keychain".to_string(),
            |(name, folder)| format!("{name}:{folder}"),
        )
}

/// `zo models --json`, whole, cached for [`MODELS_TTL`]. A zo that is
/// missing, fails, says nothing readable or outlives [`MODELS_WAIT`] — killed
/// then — leaves the catalog last read standing, or `Null` before any was.
fn zo_models_catalog(home: &Path) -> serde_json::Value {
    catalog_through(
        models_cache(),
        home,
        ACCOUNTS_ROOT.get().map(PathBuf::as_path),
    )
}

/// A change the window just made to what zo lists — a connection saved,
/// edited or removed (t-76028 F2) — makes the next read ask zo again; until
/// zo answers, the catalog last read still stands.
pub fn forget_zo_models() {
    forget_models_in(models_cache());
}

/// Ask zo to re-query every provider registry on the next read (`zo models
/// --refresh --json`) rather than serving its disk cache: a person's explicit
/// catalog refresh, a login switch, or a fresh sign-in.
pub fn refresh_zo_models() {
    refresh_models_in(models_cache());
}

fn forget_models_in(cache: &Mutex<ModelsCache>) {
    if let Ok(mut cache) = cache.lock() {
        cache.forgotten += 1;
        if let Some((at, _)) = cache.held.as_mut() {
            *at = None;
        }
    }
}

fn refresh_models_in(cache: &Mutex<ModelsCache>) {
    if let Ok(mut cache) = cache.lock() {
        cache.forgotten += 1;
        cache.force_refresh = true;
        if let Some((at, _)) = cache.held.as_mut() {
            *at = None;
        }
    }
}

fn catalog_through(
    cache: &Mutex<ModelsCache>,
    home: &Path,
    accounts: Option<&Path>,
) -> serde_json::Value {
    let bin = crate::zo_companion::zo_path_under(home);
    catalog_read(cache, |force_refresh| {
        let env = accounts.map(listing_env).unwrap_or_default();
        let args: &[&str] = if force_refresh {
            &["models", "--refresh", "--json"]
        } else {
            &["models", "--json"]
        };
        captured_lines_in(&bin, args, MODELS_WAIT, &env)
            .and_then(|lines| serde_json::from_str(&lines.join("\n")).ok())
            .map(diagnosed)
    })
}

/// The cached catalog, or what `read` — zo, asked — answers, held for
/// [`MODELS_TTL`]; `None` from it leaves the catalog last read standing.
/// An answer to a read the window forgot the list during may predate the
/// change it forgot it for, so zo is asked again (t-76028 R2).
fn catalog_read(
    cache: &Mutex<ModelsCache>,
    read: impl Fn(bool) -> Option<serde_json::Value>,
) -> serde_json::Value {
    loop {
        let Some((forgotten, force_refresh, held)) = cache
            .lock()
            .ok()
            .map(|cache| (cache.forgotten, cache.force_refresh, cache.held.clone()))
        else {
            return read(false).unwrap_or_default();
        };
        if !force_refresh
            && let Some((_, catalog)) = held
                .as_ref()
                .filter(|(at, _)| at.is_some_and(|at| at.elapsed() < MODELS_TTL))
        {
            return catalog.clone();
        }
        let catalog = read(force_refresh)
            .or_else(|| held.map(|(_, catalog)| catalog))
            .unwrap_or_default();
        let Ok(mut cache) = cache.lock() else {
            return catalog;
        };
        if cache.forgotten == forgotten {
            cache.force_refresh = false;
            cache.held = Some((Some(Instant::now()), catalog.clone()));
            return catalog;
        }
    }
}

// ---- readers ----------------------------------------------------------------

/// `name` → `description` pairs read off a bundle with `pattern` (two
/// captures). A bundle is read once per call; callers cache the catalog.
fn bundle_words(path: &Path, pattern: &str) -> HashMap<String, String> {
    let Ok(bytes) = std::fs::read(path) else {
        return HashMap::new();
    };
    let Ok(regex) = regex::bytes::Regex::new(pattern) else {
        return HashMap::new();
    };
    let mut words = HashMap::new();
    for found in regex.captures_iter(&bytes) {
        let (Some(name), Some(about)) = (found.get(1), found.get(2)) else {
            continue;
        };
        let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
        let about = unescape_js(&String::from_utf8_lossy(about.as_bytes()));
        words.entry(name).or_insert(about);
    }
    words
}

fn unescape_js(text: &str) -> String {
    text.replace("\\\"", "\"")
        .replace("\\'", "'")
        .replace("\\n", " ")
}

/// `key: value` from a `---` frontmatter block, unquoted.
fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    let body = text.strip_prefix("---")?;
    let end = body.find("\n---")?;
    body[..end].lines().find_map(|line| {
        let (found, value) = line.split_once(':')?;
        (found.trim() == key).then(|| value.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// The first line that is prose — not frontmatter, not a heading, not blank.
fn first_prose_line(text: &str) -> Option<String> {
    let rest = text
        .strip_prefix("---")
        .and_then(|body| body.find("\n---").map(|end| &body[end + 4..]))
        .unwrap_or(text);
    rest.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.to_string())
}

/// Every `*.md` directly in `dir`, as (stem, text).
fn markdown_files(dir: &Path) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(OsStr::to_str) == Some("md")).then(|| {
                let stem = path.file_stem()?.to_str()?.to_string();
                Some((stem, std::fs::read_to_string(&path).ok()?))
            })?
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cached list and a change the window makes to it (t-76028 F2): a
    /// read inside the minute is the list held, and after the window forgets
    /// it the next read is zo's own answer — never a second cache. zo here
    /// answers with the list it is given, or not at all (t-76028 R4: no
    /// process, so every platform runs it).
    #[test]
    fn a_saved_connection_is_in_the_next_zo_list_read() {
        let routers = |names: &[&str]| {
            serde_json::json!({"models": [], "customProviders": names
                .iter()
                .map(|name| serde_json::json!({"name": name, "models": [format!("{name}/model")]}))
                .collect::<Vec<_>>()})
        };
        let listed = Mutex::new(Some(routers(&[])));
        let zo = |_: bool| listed.lock().expect("listed").clone();
        let cache = Mutex::new(ModelsCache::default());
        assert_eq!(catalog_read(&cache, zo), routers(&[]));
        *listed.lock().expect("listed") = Some(routers(&["Fixture"]));
        assert_eq!(
            catalog_read(&cache, zo),
            routers(&[]),
            "a read inside the minute is the list held"
        );
        forget_models_in(&cache);
        assert_eq!(
            catalog_read(&cache, zo),
            routers(&["Fixture"]),
            "the read after a saved connection does not list it"
        );
        *listed.lock().expect("listed") = None;
        forget_models_in(&cache);
        assert_eq!(
            catalog_read(&cache, zo),
            routers(&["Fixture"]),
            "a zo that cannot answer drops the list last read"
        );
    }

    /// A list read that began before the window forgot it (t-76028 R2) — a
    /// connection saved while zo was being asked — does not put its older
    /// answer back for a minute: it asks again, and the list held after is
    /// the one that names the saved connection. The order is fixed by
    /// channels, not by a clock.
    #[test]
    fn a_list_read_that_outlives_a_save_does_not_bring_back_the_list_before_it() {
        let before = serde_json::json!({"customProviders": []});
        let after = serde_json::json!({"customProviders": [{"name": "Fixture"}]});
        let cache = Mutex::new(ModelsCache::default());
        let (asked_tx, asked) = std::sync::mpsc::channel();
        let (answer, answer_rx) = std::sync::mpsc::channel::<()>();
        let answer_rx = Mutex::new(answer_rx);
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let read = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                catalog_read(&cache, |_| {
                    if reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        asked_tx.send(()).expect("asked");
                        answer_rx.lock().expect("answer").recv().expect("answer");
                        Some(before.clone())
                    } else {
                        Some(after.clone())
                    }
                })
            });
            asked.recv().expect("zo was asked");
            forget_models_in(&cache);
            answer.send(()).expect("answer");
            reader.join().expect("reader")
        });
        assert_eq!(
            read, after,
            "the read answered the list from before the save"
        );
        assert_eq!(
            catalog_read(&cache, |_| panic!("a read inside the minute asked zo")),
            after,
            "the list held after the save is the one from before it"
        );
    }

    /// An explicit refresh (`refresh_models_in`) passes `--refresh` to `zo models`
    /// once, even inside the minute TTL, and if requested while an unforced read
    /// is already running, follows it with a forced `--refresh` read.
    #[cfg(unix)]
    #[test]
    fn refresh_models_asks_zo_with_refresh_flag_and_survives_inflight_unforced_reads() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("home");
        let bin = crate::zo_companion::zo_path_under(home.path());
        std::fs::create_dir_all(bin.parent().expect("bin dir")).expect("bin dir");
        std::fs::write(
            &bin,
            "#!/bin/sh\nif [ \"$1 $2 $3\" = 'models --refresh --json' ]; then\n  echo '{\"models\":[{\"id\":\"gemini-4-argon\"}]}'\nelif [ \"$1 $2\" = 'models --json' ]; then\n  echo '{\"models\":[]}'\nfi\n",
        )
        .expect("zo");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("mode");

        let cache = Mutex::new(ModelsCache::default());
        assert_eq!(
            catalog_through(&cache, home.path(), None),
            serde_json::json!({"models": []})
        );
        refresh_models_in(&cache);
        assert_eq!(
            catalog_through(&cache, home.path(), None),
            serde_json::json!({"models": [{"id": "gemini-4-argon"}]}),
            "refresh_models_in must invoke `zo models --refresh --json`"
        );
        // Immediately after a forced refresh completes, the warm cache serves without re-spawning.
        assert_eq!(
            catalog_read(&cache, |_| panic!(
                "warm cache after refresh should not re-spawn"
            )),
            serde_json::json!({"models": [{"id": "gemini-4-argon"}]})
        );

        // And a forced refresh arriving mid-flight during an unforced read forces the retry to pass `true`.
        forget_models_in(&cache);
        let (asked_tx, asked) = std::sync::mpsc::channel();
        let (answer, answer_rx) = std::sync::mpsc::channel::<()>();
        let answer_rx = Mutex::new(answer_rx);
        let seen_flags = Mutex::new(Vec::new());
        let out = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                catalog_read(&cache, |forced| {
                    let first = {
                        let mut flags = seen_flags.lock().expect("flags");
                        let is_first = flags.is_empty();
                        flags.push(forced);
                        is_first
                    };
                    if first {
                        asked_tx.send(()).expect("asked");
                        answer_rx.lock().expect("answer").recv().expect("answer");
                        Some(serde_json::json!({"models": []}))
                    } else {
                        Some(serde_json::json!({"models": [{"id": "gemini-4-argon"}]}))
                    }
                })
            });
            asked.recv().expect("unforced read started");
            refresh_models_in(&cache);
            answer.send(()).expect("release unforced read");
            reader.join().expect("reader")
        });
        assert_eq!(
            out,
            serde_json::json!({"models": [{"id": "gemini-4-argon"}]})
        );
        assert_eq!(*seen_flags.lock().expect("flags"), vec![false, true]);
    }

    /// The list is `zo models --json` from the zo under the home, whole.
    /// The zo here is a POSIX script, so this one runs where one starts.
    #[cfg(unix)]
    #[test]
    fn the_zo_list_is_what_the_zo_under_the_home_prints() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("home");
        let bin = crate::zo_companion::zo_path_under(home.path());
        std::fs::create_dir_all(bin.parent().expect("bin dir")).expect("bin dir");
        std::fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1 $2\" = 'models --json' ] && echo '{\"customProviders\":[{\"name\":\"Fixture\"}]}'\n",
        )
        .expect("zo");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("mode");
        assert_eq!(
            catalog_through(&Mutex::new(ModelsCache::default()), home.path(), None),
            serde_json::json!({"customProviders": [{"name": "Fixture"}]})
        );
    }

    /// The window's `zo models` read gets a zo pane's account variables
    /// (t-76598): before, it ran with none, so on a machine whose Claude login
    /// lives in the window's account home only the window's read failed — and
    /// the settings said 「로그인 안 됨」 while every zo pane answered.
    ///
    /// The fake zo writes down the account variables alone, never the rest of
    /// the environment it inherits (t-76759), and the variables are compared
    /// with [`crate::usage_runtime::account_vars_for`] — the launch's own
    /// variables, computed without putting a login anywhere or carrying the
    /// person's `~/.codex` into the fixture account.
    #[cfg(unix)]
    #[test]
    fn the_zo_list_is_read_with_the_account_variables_a_zo_pane_gets() {
        use std::os::unix::fs::PermissionsExt;
        use zerocode_core::account::{CONFIG_DIR_VAR, SECURE_STORAGE_CONFIG_DIR_VAR};
        use zerocode_core::codex_account::HOME_VAR;
        let home = tempfile::tempdir().expect("home");
        let config = tempfile::tempdir().expect("config root");
        let codex_home = config.path().join("codex-accounts/account-a/home");
        std::fs::create_dir_all(&codex_home).expect("codex account home");
        std::fs::write(
            codex_home.join(crate::codex_accounts::AUTH_FILE),
            r#"{"OPENAI_API_KEY":"fixture"}"#,
        )
        .expect("codex auth");
        std::fs::write(
            config
                .path()
                .join(crate::codex_accounts::ACCOUNT_STORE_FILE),
            serde_json::json!({
                "accounts": [{"id": "account-a", "home_dir": codex_home, "added_at": 0}],
                "active": "account-a"
            })
            .to_string(),
        )
        .expect("codex account store");
        let bin = crate::zo_companion::zo_path_under(home.path());
        std::fs::create_dir_all(bin.parent().expect("bin dir")).expect("bin dir");
        let seen = home.path().join("seen-env");
        let pane = crate::usage_runtime::account_vars_for(config.path(), "zo");
        assert!(
            !codex_home.join("config.toml").exists() && !codex_home.join("AGENTS.md").exists(),
            "computing the pane's variables carried settings into the account"
        );
        for name in [CONFIG_DIR_VAR, SECURE_STORAGE_CONFIG_DIR_VAR, HOME_VAR] {
            assert!(
                pane.iter().any(|(key, _)| key == name),
                "the pane gets {name}: {pane:?}"
            );
        }
        // The pane's own variable names, and nothing else of what the fake
        // zo inherits.
        let names: Vec<&str> = pane.iter().map(|(key, _)| key.as_str()).collect();
        let script: String = names
            .iter()
            .map(|name| format!("[ \"${{{name}+set}}\" = set ] && printf '%s=%s\\n' {name} \"${name}\" >> '{}'\n", seen.display()))
            .collect();
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\n: > '{}'\n{script}echo '{{\"reports\":[]}}'\n",
                seen.display()
            ),
        )
        .expect("zo");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("mode");

        assert_eq!(
            catalog_through(
                &Mutex::new(ModelsCache::default()),
                home.path(),
                Some(config.path())
            ),
            serde_json::json!({"reports": []})
        );
        let seen = std::fs::read_to_string(&seen).expect("zo ran");
        let seen: HashMap<&str, &str> = seen
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();
        assert!(
            seen.keys().all(|key| names.contains(key)),
            "the fake zo wrote down more than the account variables: {:?}",
            seen.keys().collect::<Vec<_>>()
        );
        for (key, value) in &pane {
            assert_eq!(
                seen.get(key.as_str()).copied(),
                Some(value.as_str()),
                "the list's zo did not get the pane's {key}"
            );
        }
        assert_eq!(
            credential_store(&listing_env(config.path())),
            credential_store(&pane),
            "the place the settings compare a report with is not the pane's"
        );
    }

    /// The label a report is compared with is the one zo would write for the
    /// pane's variables (ZO m-76774, `ClaudeCodeStore::from_folders`): the
    /// credential folder wins whenever it is named — empty, it is the
    /// keychain — and the config folder only when it is not.
    #[test]
    fn the_pane_store_is_named_by_zos_own_rule() {
        use zerocode_core::account::{CONFIG_DIR_VAR, SECURE_STORAGE_CONFIG_DIR_VAR};
        let env = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        };
        assert_eq!(
            credential_store(&env(&[(CONFIG_DIR_VAR, "/fixture/runtime")])),
            "CLAUDE_CONFIG_DIR:/fixture/runtime"
        );
        assert_eq!(
            credential_store(&env(&[
                (CONFIG_DIR_VAR, "/fixture/runtime"),
                (SECURE_STORAGE_CONFIG_DIR_VAR, "/fixture/account")
            ])),
            "CLAUDE_SECURESTORAGE_CONFIG_DIR:/fixture/account"
        );
        assert_eq!(
            credential_store(&env(&[
                (CONFIG_DIR_VAR, "/fixture/runtime"),
                (SECURE_STORAGE_CONFIG_DIR_VAR, "")
            ])),
            "keychain",
            "an empty credential folder names the keychain, not the config folder"
        );
        assert_eq!(credential_store(&env(&[(CONFIG_DIR_VAR, "")])), "keychain");
        assert_eq!(credential_store(&[]), "keychain");
    }

    /// The label is made from the variables the list's zo ends with, not
    /// only those the window names (t-76873 P2 ②): the system's default
    /// account names no Claude folder, and the zo then reads the one it
    /// inherits from the window — the same place a pane reads, which is not
    /// 「다른 환경」. What the window names wins over what is inherited, by
    /// zo's rule, and an empty credential folder is still the keychain.
    #[test]
    fn the_pane_store_counts_the_folder_the_lookup_inherits() {
        use zerocode_core::account::{CONFIG_DIR_VAR, SECURE_STORAGE_CONFIG_DIR_VAR};
        let env = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        };
        let inheriting = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            }
        };
        let store = |given: &[(&str, &str)], inherited: &'static [(&'static str, &'static str)]| {
            credential_store(&lookup_env(&env(given), inheriting(inherited)))
        };
        assert_eq!(
            store(
                &[("ANTHROPIC_API_KEY", "")],
                &[(CONFIG_DIR_VAR, "/fixture/ambient")]
            ),
            "CLAUDE_CONFIG_DIR:/fixture/ambient",
            "the default account's lookup reads the inherited folder"
        );
        assert_eq!(
            store(
                &[],
                &[(SECURE_STORAGE_CONFIG_DIR_VAR, "/fixture/ambient-account")]
            ),
            "CLAUDE_SECURESTORAGE_CONFIG_DIR:/fixture/ambient-account"
        );
        assert_eq!(
            store(
                &[(CONFIG_DIR_VAR, "/fixture/runtime")],
                &[(SECURE_STORAGE_CONFIG_DIR_VAR, "/fixture/ambient-account")]
            ),
            "CLAUDE_SECURESTORAGE_CONFIG_DIR:/fixture/ambient-account",
            "an inherited credential folder wins over a named config folder, by zo's rule"
        );
        assert_eq!(
            store(
                &[(SECURE_STORAGE_CONFIG_DIR_VAR, "/fixture/account")],
                &[(SECURE_STORAGE_CONFIG_DIR_VAR, "/fixture/ambient-account")]
            ),
            "CLAUDE_SECURESTORAGE_CONFIG_DIR:/fixture/account",
            "an inherited folder overrode the one the window names"
        );
        assert_eq!(
            store(
                &[(SECURE_STORAGE_CONFIG_DIR_VAR, "")],
                &[(CONFIG_DIR_VAR, "/fixture/ambient")]
            ),
            "keychain",
            "an empty credential folder named by the window is the keychain"
        );
        assert_eq!(
            store(
                &[],
                &[
                    (SECURE_STORAGE_CONFIG_DIR_VAR, ""),
                    (CONFIG_DIR_VAR, "/fixture/ambient")
                ]
            ),
            "keychain",
            "an empty inherited credential folder is the keychain"
        );
        assert_eq!(store(&[], &[]), "keychain");
    }

    /// A login report's `detail` is a provider's answer — an HTTP error body
    /// zo cut at 160 characters — and no masking rule reads every credential
    /// such a body can hold: a query token without a digit, a header value on
    /// the line after its name, an address cut before its `@` (t-76873 P1).
    /// So the text never enters the window: the list keeps a diagnosis made of
    /// the status, one kind word and whether zo skipped the source, and
    /// neither the answer to the screen nor the list the window holds carries
    /// a word of the body. Synthetic values only; the zo is a POSIX script.
    #[cfg(unix)]
    #[test]
    fn a_login_failure_reaches_the_window_as_a_diagnosis_and_never_as_its_body() {
        use std::os::unix::fs::PermissionsExt;
        let secrets = [
            "fixtureonlyletters",
            "fixturemultilinevalue",
            "fixturecutpassword",
            "fixturebearerletters",
            "abcdefabcdef",
            "sk-ant-api03-TESTONLYxxxxxxxxxxxxxxxxxxxxxxxx",
        ];
        let failed = |provider: &str, detail: String| serde_json::json!({"provider": provider, "source": format!("{provider}-api"), "ok": false, "count": 0, "detail": detail});
        // A report as zo sets it (t-76968): the kind and the status in
        // fields of their own, whatever the text says.
        let reported = |provider: &str, detail: String, kind: &str, status: Option<u64>| {
            let mut report = failed(provider, detail);
            report["failureKind"] = serde_json::json!(kind);
            if let Some(status) = status {
                report["httpStatus"] = serde_json::json!(status);
            }
            report
        };
        let printed = serde_json::json!({
            "reports": [
                reported("anthropic", format!("HTTP 401 Unauthorized at https://example.test/?access_token={}", secrets[0]), "auth_rejected", Some(401)),
                reported("anthropic", format!("HTTP 403 Forbidden: echoed Authorization:\n{}", secrets[1]), "forbidden", Some(403)),
                reported("openai", format!("HTTP 404 Not Found: {{\"url\":\"https://fixtureuser:{}", secrets[2]), "not_found", Some(404)),
                reported("openai", format!("HTTP 429 Too Many Requests: Authorization: Bearer {}", secrets[3]), "rate_limited", Some(429)),
                reported("google", format!("HTTP 503 Service Unavailable: https://files.example.test/o?X-Amz-Signature={}", secrets[4]), "server_error", Some(503)),
                reported("anthropic", format!("error sending request for url (https://example.test/v1/models?key={}) · kept 3 model(s) from the previous refresh", secrets[5]), "network", None),
                reported("xai", format!("operation timed out after reading {}", secrets[0]), "timeout", None),
                reported("xai", "skipped: no login".to_string(), "no_credentials", None),
                reported("google", format!("unexpected answer {}", secrets[3]), "unknown", None),
                {"provider": "google", "source": "google-api", "ok": true, "count": 3, "detail": format!("3 model(s) · {}", secrets[0])},
                // An older zo names no kind: the window names none either,
                // reads no status out of the text, and takes only zo's
                // fixed `skipped:` head for a missing credential.
                failed("anthropic", format!("HTTP 401 Unauthorized {}", secrets[0])),
                failed("xai", "skipped: no key".to_string()),
                // The text never changes the kind: "connect", "timeout" and
                // a status in the words of a failure zo calls unknown.
                reported("openai", format!("the next connection asks again · connect timeout HTTP 503 {}", secrets[3]), "unknown", None),
                // A kind this window does not know is unknown, a status that
                // is no HTTP status is none, and the cache file's spelling
                // reads like the command's.
                reported("google", "a later zo's kind".to_string(), "a_kind_from_a_later_zo", Some(99999)),
                {"provider": "google", "source": "google-api", "ok": false, "count": 0, "detail": "x", "failure_kind": "rate_limited", "http_status": 429},
                // A login that is there but cannot be used is not one that
                // is missing: it is not skipped.
                reported("anthropic", "the Claude Code sign-in expired".to_string(), "credential_unusable", None),
            ]
        });
        let home = tempfile::tempdir().expect("home");
        let bin = crate::zo_companion::zo_path_under(home.path());
        std::fs::create_dir_all(bin.parent().expect("bin dir")).expect("bin dir");
        let answer = home.path().join("answer.json");
        std::fs::write(&answer, printed.to_string()).expect("answer");
        std::fs::write(&bin, format!("#!/bin/sh\ncat '{}'\n", answer.display())).expect("zo");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("mode");

        let cache = Mutex::new(ModelsCache::default());
        let list = model_list_of(&catalog_through(&cache, home.path(), None));
        let held = cache.lock().expect("cache").held.clone().expect("held").1;
        for (place, said) in [
            (
                "the answer to the screen",
                serde_json::to_string(&list).expect("json"),
            ),
            ("the list the window holds", held.to_string()),
        ] {
            for secret in secrets {
                assert!(!said.contains(secret), "{secret} reached {place}: {said}");
            }
            assert!(
                !said.contains("\"detail\""),
                "a body reached {place}: {said}"
            );
            for raw in ["failureKind", "failure_kind", "httpStatus", "http_status"] {
                assert!(!said.contains(raw), "{raw} reached {place}: {said}");
            }
        }
        let diagnoses: Vec<serde_json::Value> = list["reports"]
            .as_array()
            .expect("reports")
            .iter()
            .map(|report| report.get("diagnosis").cloned().unwrap_or_default())
            .collect();
        let diagnosis = |status: Option<u16>, kind: Option<&str>, skipped: bool| serde_json::json!({"status": status, "kind": kind, "skipped": skipped});
        assert_eq!(
            diagnoses,
            [
                diagnosis(Some(401), Some("auth"), false),
                diagnosis(Some(403), Some("forbidden"), false),
                diagnosis(Some(404), Some("notFound"), false),
                diagnosis(Some(429), Some("rateLimit"), false),
                diagnosis(Some(503), Some("server"), false),
                diagnosis(None, Some("network"), false),
                diagnosis(None, Some("timeout"), false),
                diagnosis(None, None, true),
                diagnosis(None, Some("unknown"), false),
                serde_json::Value::Null,
                diagnosis(None, None, false),
                diagnosis(None, None, true),
                diagnosis(None, Some("unknown"), false),
                diagnosis(None, Some("unknown"), false),
                diagnosis(Some(429), Some("rateLimit"), false),
                diagnosis(None, Some("credentialUnusable"), false),
            ]
        );
        assert_eq!(list["reports"][9]["count"], 3, "an answer lost its count");
    }

    #[test]
    fn a_bundle_yields_name_and_description_pairs_once_each() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("cli.js");
        std::fs::write(
            &bundle,
            br#"x={name:"compact",supportsNonInteractive:!0,description:"Free up context by summarizing"};y={name:"alias",description:"not a command",other:1};z={name:"compact",description:"a second spelling"}"#,
        )
        .unwrap();
        let words = bundle_words(
            &bundle,
            r#"name:"([a-z][a-z0-9-]{1,40})",(?:[^{}"]|"(?:[^"\\]|\\.)*")*?description:"((?:[^"\\]|\\.){3,240})""#,
        );
        assert_eq!(
            words["compact"], "Free up context by summarizing",
            "the first pairing stands"
        );
        assert_eq!(
            words["alias"], "not a command",
            "noise is filtered by the announced names, not here"
        );
    }

    #[test]
    fn frontmatter_gives_the_files_own_words() {
        let skill = "---\nname: deploy\ndescription: \"Ship the thing\"\nargument-hint: [env]\n---\n# Deploy\nbody";
        assert_eq!(
            frontmatter_value(skill, "description").as_deref(),
            Some("Ship the thing")
        );
        assert_eq!(frontmatter_value("no frontmatter", "description"), None);
        assert_eq!(first_prose_line(skill).as_deref(), Some("body"));
    }

    #[test]
    fn agy_help_rows_give_the_name_the_alias_and_the_words() {
        let rows: Vec<SlashCommand> = [
            "/agents\tList available custom agents",
            "/config (settings)\tOpen settings panel",
            "/usage (quota)\tView model quota usage",
            "Press ctrl+o to expand tool details.",
            "",
        ]
        .iter()
        .filter_map(|line| agy_help_row(line))
        .collect();
        assert_eq!(
            rows.iter()
                .map(|row| (row.name.as_str(), row.about.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("/agents", "List available custom agents"),
                ("/config", "Open settings panel (/settings)"),
                ("/usage", "View model quota usage (/quota)"),
            ]
        );
        assert!(rows.iter().all(|row| row.kind == "builtin"));
    }

    #[test]
    fn the_codex_snapshot_parses_and_names_its_provenance() {
        let snapshot: CodexSnapshot =
            serde_json::from_str(include_str!("slash/codex-commands.json")).expect("snapshot json");
        assert!(snapshot.commands.len() >= 40, "{}", snapshot.commands.len());
        assert!(
            snapshot
                .commands
                .iter()
                .all(|row| row.name.starts_with('/'))
        );
        assert!(snapshot.commands.iter().any(|row| row.name == "/model"));
        assert!(
            snapshot.cli_version_observed.is_some(),
            "the snapshot says which Codex it was read against"
        );
    }

    #[test]
    fn an_agent_the_module_has_no_road_to_answers_nothing_not_a_guess() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = build_catalog("kimi", dir.path(), dir.path(), None);
        assert_eq!(catalog.source, "none");
        assert!(catalog.commands.is_empty());
    }

    #[test]
    fn the_claude_announcement_is_read_for_names_kinds_and_version() {
        let init: ClaudeInit = serde_json::from_str(
            r#"{"type":"system","subtype":"init","slash_commands":["compact","design","codex:review","mcp__x__fix"],"skills":["design","codex:review"],"plugins":[{"name":"codex","path":"/p/codex"}],"claude_code_version":"2.1.272","model":"m"}"#,
        )
        .unwrap();
        assert_eq!(init.slash_commands.len(), 4);
        assert_eq!(init.claude_code_version.as_deref(), Some("2.1.272"));
        assert_eq!(init.plugins[0].path, PathBuf::from("/p/codex"));
    }

    #[test]
    fn models_follow_the_agents_provider_and_zo_lists_every_row() {
        let catalog = serde_json::json!({
            "models": [
                {"id":"claude-opus-4-7","provider":"claude","displayName":"Opus 4.7","band":"superseded"},
                {"id":"claude-opus-5","provider":"claude","displayName":"Opus 5","band":"top"},
                {"id":"gpt-6-astra","provider":"openai","displayName":"GPT-6-Astra","band":"top"},
                {"id":"gemini-3.6-flash","provider":"google","displayName":"Gemini 3.6 Flash","unlistedSince":1_793_400_000},
                {"id":"gemini-4-argon","provider":"google","displayName":"Gemini 4 Argon","band":"top"},
                {"id":"gemini-3.8-flash","provider":"google","displayName":"Gemini 3.8 Flash","band":"second"}
            ],
            "otherLogins": [
                {"id":"grok-4","provider":"xai","displayName":"Grok 4"},
                {"id":"kimi-k2","provider":"kimi","displayName":"Kimi K2"}
            ],
            "customProviders": [
                {"name":"openrouter","usable":true,"models":["openrouter/qwen-coder"]},
                {"name":"keyless","usable":false,"models":["keyless/ignored"]}
            ]
        });
        let ids = |agent: &str| -> Vec<String> {
            agent_models_from(agent, &catalog)
                .into_iter()
                .map(|row| row.id)
                .collect()
        };
        assert_eq!(ids("claude"), vec!["claude-opus-5", "claude-opus-4-7"]);
        assert_eq!(ids("codex"), vec!["gpt-6-astra"]);
        assert_eq!(
            ids("antigravity"),
            vec!["gemini-4-argon", "gemini-3.8-flash"],
            "withdrawn rows are omitted and top stands ahead of second"
        );
        assert_eq!(
            ids("zo"),
            vec![
                "claude-opus-5",
                "gpt-6-astra",
                "gemini-4-argon",
                "gemini-3.8-flash",
                "claude-opus-4-7",
                "openrouter/qwen-coder",
            ]
        );
        assert_eq!(ids("grok"), vec!["grok-4"]);
        assert_eq!(ids("kimi"), vec!["kimi-k2"]);
        assert!(ids("cursor").is_empty());

        let list = model_list_of(&catalog);
        assert_eq!(list["otherLogins"].as_array().unwrap().len(), 2);
    }
}
