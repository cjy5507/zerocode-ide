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
/// How long the Claude Code announcement may take before the probe gives up.
const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(15);
/// The prompt the probe sends so the session starts; killed at the
/// announcement, before any answer. The words do not matter.
const PROBE_PROMPT: &str = "hi";

/// Answers by (agent, checkout), each with the moment it was read.
type CatalogCache = HashMap<(String, String), (Instant, SlashCatalog)>;
/// zo's model rows, with the moment they were read.
type ModelsCache = Option<(Instant, Vec<serde_json::Value>)>;

fn catalog_cache() -> &'static Mutex<CatalogCache> {
    static CACHE: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn models_cache() -> &'static Mutex<ModelsCache> {
    static CACHE: OnceLock<Mutex<ModelsCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
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
    let mut command = crate::proc::quiet_command(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("ZEROCODE") {
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
/// filtered to the provider the agent drives.
pub fn agent_models(agent: &str, home: &Path) -> Vec<ModelRow> {
    let provider = zerocode_core::agent::agent_voice(agent).models_provider;
    let rows = zo_models(home);
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?;
            let row_provider = row.get("provider")?.as_str()?;
            if provider.is_some_and(|wanted| wanted != row_provider) {
                return None;
            }
            Some(ModelRow {
                id: id.to_string(),
                display_name: row
                    .get("displayName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                provider: row_provider.to_string(),
            })
        })
        .collect()
}

fn zo_models(home: &Path) -> Vec<serde_json::Value> {
    if let Ok(cache) = models_cache().lock()
        && let Some((at, rows)) = cache.as_ref()
        && at.elapsed() < MODELS_TTL
    {
        return rows.clone();
    }
    let bin = crate::zo_companion::zo_path_under(home);
    let rows = crate::proc::quiet_command(&bin)
        .args(["models", "--json"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .and_then(|catalog| catalog.get("models")?.as_array().cloned())
        .unwrap_or_default();
    if let Ok(mut cache) = models_cache().lock() {
        *cache = Some((Instant::now(), rows.clone()));
    }
    rows
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
        let rows = [
            serde_json::json!({"id":"claude-opus-5","provider":"claude","displayName":"Opus 5"}),
            serde_json::json!({"id":"gpt-6-astra","provider":"openai","displayName":"GPT-6-Astra"}),
        ];
        let pick = |agent: &str| -> Vec<String> {
            let provider = zerocode_core::agent::agent_voice(agent).models_provider;
            rows.iter()
                .filter(|row| provider.is_none_or(|wanted| row["provider"] == wanted))
                .map(|row| row["id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(pick("claude"), vec!["claude-opus-5"]);
        assert_eq!(pick("codex"), vec!["gpt-6-astra"]);
        assert_eq!(pick("zo").len(), 2);
    }
}
