//! Pre-mark a workspace as trusted for the agents that ask on first launch.
//!
//! Claude Code, cursor-agent, GitHub Copilot CLI, Codex, ZO and Antigravity each
//! open a "Do you trust this folder?" menu the first time they run somewhere — a menu that
//! reads one keystroke. This window pastes the launch prompt into the TUI as
//! soon as it is up, so that keystroke was the first character of somebody's
//! prompt: the menu picked an arbitrary option or quit the session (the map's
//! P0-8).
//! The bypass is to write the exact trust artifact the agent itself writes
//! after a person accepts, before the launch — the CLIs read these files at
//! startup, ahead of the menu. The person consented to the workspace when they
//! opened it here; this repeats their answer in the agent's own spelling
//! rather than asking them twice.
//!
//! A `--trust`-style flag is not a substitute, for Orca's measured reasons:
//! cursor-agent's applies only headless, Copilot has none, and Codex's
//! `--dangerously-bypass-approvals-and-sandbox` changes approval policy —
//! a different consent than "trust this project".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use zerocode_core::civil::iso_utc_of;

/// The capability table's own enum under the name this module's writers have
/// always used. Which agent opens which menu is that agent's row
/// (`zerocode_core::capabilities::TrustMenu`, `Harness::blocked`), so the
/// launch, resume and worker-split doors ask the row and never a list here.
pub type TrustPreset = zerocode_core::capabilities::TrustMenu;

/// The preset the agent called `slug` needs, read off its row — this
/// module's own test reads the table through it; the write doors ask their
/// row directly and nothing shipped holds only a name.
#[cfg(test)]
pub fn preset_of(slug: &str) -> Option<TrustPreset> {
    zerocode_core::agent_capabilities(slug).and_then(|caps| caps.trust_menu())
}

/// Write the agent's own trust artifact for this workspace, best-effort.
///
/// `env` is the environment the launch hands the process. The writers read
/// the config homes it names — `CODEX_HOME`, `CLAUDE_CONFIG_DIR` — because a
/// preset in a config the spawned agent never opens is no preset.
pub fn mark_workspace_trusted(
    preset: TrustPreset,
    workspace: &Path,
    env: &[(String, String)],
) -> std::io::Result<()> {
    match preset {
        TrustPreset::Zo => mark_zo_workspace_trusted(workspace),
        TrustPreset::Claude => claude_config_of(env).map_or(Ok(()), |config| {
            mark_claude_project_trusted(workspace, &config)
        }),
        TrustPreset::Cursor => mark_cursor_workspace_trusted(workspace),
        TrustPreset::Copilot => mark_copilot_folder_trusted(workspace),
        TrustPreset::Codex => mark_codex_project_trusted(workspace, &codex_configs_of(env)),
        TrustPreset::Antigravity => mark_antigravity_workspace_trusted(workspace),
    }
}

/// The key Claude Code files trust under, as its own writer spells it.
const CLAUDE_TRUST_ACCEPTED: &str = "hasTrustDialogAccepted";
/// Claude Code's config lock, the way `proper-lockfile` makes it: a directory
/// beside the file, its mtime refreshed while held, abandoned once this old.
const CLAUDE_CONFIG_LOCK_STALE: Duration = Duration::from_secs(10);
/// How long a launch waits on a live holder before the menu is left to the
/// person — the lock is held for one read-modify-write, milliseconds.
const CLAUDE_CONFIG_LOCK_PATIENCE: Duration = Duration::from_secs(2);
/// The gap between two tries at a held lock.
const CLAUDE_CONFIG_LOCK_RETRY: Duration = Duration::from_millis(20);

/// Claude Code 2.1.273 opens "Quick safety check: Is this a project you
/// created or one you trust?" in a folder its global config has not trusted,
/// `--dangerously-skip-permissions` or not, and a worker's briefing waits
/// behind it (measured 2026-09-17 in a pty; a trusted repository answers it
/// for every linked worktree of that repository). The answer is
/// `projects["<root>"].hasTrustDialogAccepted`, under the root Claude writes
/// itself when a person accepts: the repository for a linked worktree.
///
/// Claude rewrites that file from many processes at once, so the write takes
/// the lock Claude takes and reads again under it. A config that does not
/// exist yet is left alone: a first run is onboarding before it is a trust
/// question, and this window does not author a person's global config.
fn mark_claude_project_trusted(workspace: &Path, config: &Path) -> std::io::Result<()> {
    mark_claude_project_trusted_within(
        workspace,
        config,
        CLAUDE_CONFIG_LOCK_PATIENCE,
        CLAUDE_CONFIG_LOCK_STALE,
    )
}

/// [`mark_claude_project_trusted`] on explicit lock clocks, for tests.
fn mark_claude_project_trusted_within(
    workspace: &Path,
    config: &Path,
    patience: Duration,
    stale_after: Duration,
) -> std::io::Result<()> {
    let root = repository_trust_root(workspace)
        .to_string_lossy()
        .into_owned();
    // Asked once without the lock: a project already trusted costs nobody a wait.
    match read_claude_config(config)? {
        Some(document) if !claude_trusts(&document, &root) => {}
        _ => return Ok(()),
    }
    let _held = ClaudeConfigLock::take(config, patience, stale_after)?;
    let Some(mut document) = read_claude_config(config)? else {
        return Ok(());
    };
    if claude_trusts(&document, &root) {
        return Ok(());
    }
    let invalid = |why: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} was left alone: {why}", config.display()),
        )
    };
    document
        .as_object_mut()
        .ok_or_else(|| invalid("its top level is not an object"))?
        .entry("projects")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| invalid("projects is not an object"))?
        .entry(root)
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| invalid("the project entry is not an object"))?
        .insert(
            CLAUDE_TRUST_ACCEPTED.to_string(),
            serde_json::Value::Bool(true),
        );
    write_atomically(config, &format!("{document:#}\n"))
}

/// The config as JSON, `None` when there is no file. A file that will not
/// parse is an error, and nothing is written over it.
fn read_claude_config(config: &Path) -> std::io::Result<Option<serde_json::Value>> {
    match std::fs::read_to_string(config) {
        Ok(raw) => serde_json::from_str(&raw).map(Some).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not valid JSON ({error})", config.display()),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn claude_trusts(document: &serde_json::Value, root: &str) -> bool {
    document
        .get("projects")
        .and_then(|projects| projects.get(root))
        .and_then(|project| project.get(CLAUDE_TRUST_ACCEPTED))
        == Some(&serde_json::Value::Bool(true))
}

/// Claude Code's own lock on its global config, held for one write.
struct ClaudeConfigLock(PathBuf);

impl ClaudeConfigLock {
    fn take(config: &Path, patience: Duration, stale_after: Duration) -> std::io::Result<Self> {
        let mut name = config.as_os_str().to_owned();
        name.push(".lock");
        let lock = PathBuf::from(name);
        let deadline = Instant::now() + patience;
        loop {
            match std::fs::create_dir(&lock) {
                Ok(()) => return Ok(Self(lock)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // An abandoned lock is taken over the way `proper-lockfile`
                    // takes one over; a lock that will not go stays a held one.
                    let abandoned = std::fs::metadata(&lock)
                        .and_then(|meta| meta.modified())
                        .is_ok_and(|at| at.elapsed().is_ok_and(|age| age >= stale_after));
                    if abandoned && std::fs::remove_dir(&lock).is_ok() {
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::WouldBlock,
                            format!(
                                "Claude Code held {} for more than {} ms",
                                lock.display(),
                                patience.as_millis()
                            ),
                        ));
                    }
                    std::thread::sleep(CLAUDE_CONFIG_LOCK_RETRY);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for ClaudeConfigLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// The global config the launched Claude Code reads — its own
/// `getGlobalClaudeFile`: the legacy `.config.json` in its config home when
/// one is there, else `.claude.json` in `CLAUDE_CONFIG_DIR` or the home.
///
/// `CLAUDE_CONFIG_DIR` is the launch's word when the launch names it (an
/// empty value is the lane's removal) and this process's otherwise, since the
/// child inherits it.
fn claude_config_of(env: &[(String, String)]) -> Option<PathBuf> {
    let var = zerocode_core::account::CONFIG_DIR_VAR;
    let named = env
        .iter()
        .rev()
        .find(|(name, _)| name == var)
        .map(|(_, value)| value.clone());
    claude_config_under(
        named,
        std::env::var_os(var).map(PathBuf::from),
        dirs::home_dir(),
    )
}

/// [`claude_config_of`] with the process's answers handed in, for tests.
fn claude_config_under(
    named: Option<String>,
    inherited: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    let dir = match named {
        Some(value) => (!value.is_empty()).then(|| PathBuf::from(value)),
        None => inherited.filter(|dir| !dir.as_os_str().is_empty()),
    };
    let config_home = dir
        .clone()
        .or_else(|| home.as_ref().map(|home| home.join(".claude")))?;
    let legacy = config_home.join(".config.json");
    if legacy.is_file() {
        return Some(legacy);
    }
    Some(dir.or(home)?.join(".claude.json"))
}

/// Antigravity compares the current workspace by exact path against the
/// `trustedWorkspaces` array in
/// `~/.gemini/antigravity-cli/settings.json`. A fresh orchestration worktree
/// otherwise puts its trust modal in front of `--prompt-interactive`, and the
/// modal consumes the worker's briefing instead of the composer receiving it.
fn mark_antigravity_workspace_trusted(workspace: &Path) -> std::io::Result<()> {
    match dirs::home_dir() {
        Some(home) => mark_antigravity_workspace_trusted_under(&home, workspace),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the Antigravity settings home could not be resolved",
        )),
    }
}

/// The write under an explicit home, so tests never change the process-global
/// `HOME`. The lock covers the whole read/modify/write: several workers can be
/// summoned together, and independently appending two paths must not let the
/// last rename erase the first.
fn mark_antigravity_workspace_trusted_under(home: &Path, workspace: &Path) -> std::io::Result<()> {
    static SETTINGS_WRITE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _writing = SETTINGS_WRITE
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|held| held.into_inner());

    let settings = home
        .join(".gemini")
        .join("antigravity-cli")
        .join("settings.json");
    let mut document = match std::fs::read_to_string(&settings) {
        Ok(raw) => serde_json::from_str::<serde_json::Value>(&raw).map_err(|error| {
            invalid_antigravity_settings(&settings, format!("it is not valid JSON ({error})"))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Err(error) => return Err(error),
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| invalid_antigravity_settings(&settings, "its top level is not an object"))?;
    let exact = canonical_or_raw(workspace).to_string_lossy().into_owned();
    let trusted = match object.get_mut("trustedWorkspaces") {
        Some(serde_json::Value::Array(trusted)) => trusted,
        Some(_) => {
            return Err(invalid_antigravity_settings(
                &settings,
                "trustedWorkspaces is not an array",
            ));
        }
        None => {
            object.insert(
                "trustedWorkspaces".to_string(),
                serde_json::Value::Array(Vec::new()),
            );
            object
                .get_mut("trustedWorkspaces")
                .and_then(serde_json::Value::as_array_mut)
                .expect("the array was inserted above")
        }
    };
    if trusted.iter().any(|entry| !entry.is_string()) {
        return Err(invalid_antigravity_settings(
            &settings,
            "trustedWorkspaces contains a non-string path",
        ));
    }
    if trusted.iter().any(|entry| entry.as_str() == Some(&exact)) {
        return Ok(());
    }
    trusted.push(serde_json::Value::String(exact));
    let mut body = serde_json::to_vec_pretty(&document).map_err(|error| {
        invalid_antigravity_settings(&settings, format!("it could not be encoded ({error})"))
    })?;
    body.push(b'\n');
    crate::durable_file::replace_bytes(&settings, &body).map(|_| ())
}

fn invalid_antigravity_settings(settings: &Path, reason: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "Antigravity settings.json could not be updated at {}: {reason}",
            settings.display()
        ),
    )
}

/// ZO 1.2.7 keeps an exact-path permission map in
/// `~/.zo/trusted_workspaces.json`. A fresh worktree that is absent from this
/// map opens an interactive trust menu before enabling bracketed paste, so an
/// unattended launch waits to its readiness timeout and never receives its
/// task. ZeroCode already owns the workspace consent and launches unattended
/// agents with full access, so a new path gets that same explicit value.
fn mark_zo_workspace_trusted(workspace: &Path) -> std::io::Result<()> {
    match dirs::home_dir() {
        Some(home) => mark_zo_workspace_trusted_under(&home, workspace),
        None => Ok(()),
    }
}

fn mark_zo_workspace_trusted_under(home: &Path, workspace: &Path) -> std::io::Result<()> {
    let absolute = canonical_or_raw(workspace).to_string_lossy().into_owned();
    let dir = home.join(".zo");
    let file = dir.join("trusted_workspaces.json");
    let mut trusted = match std::fs::read_to_string(&file) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(map)) => map,
            // A file ZO itself cannot read is not ours to replace. The launch
            // will show the menu, which is safer than erasing somebody's
            // hand-written policy.
            Ok(_) | Err(_) => return Ok(()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(error) => return Err(error),
    };
    // An existing answer — including `prompt` or read-only — is the person's
    // own narrower policy and wins over this unattended default.
    if trusted.contains_key(&absolute) {
        return Ok(());
    }
    trusted.insert(
        absolute,
        serde_json::Value::String("danger-full-access".to_string()),
    );
    std::fs::create_dir_all(&dir)?;
    write_atomically(
        &file,
        &format!("{:#}\n", serde_json::Value::Object(trusted)),
    )
}

/// Cursor keeps a per-workspace marker at
/// `~/.cursor/projects/<slug>/.workspace-trusted`, payload
/// `{ trustedAt, workspacePath }` — verified by Orca against the cursor-agent
/// bundle (agent-trust-presets.ts:30-57). An existing marker is somebody's
/// standing answer and is not rewritten.
fn mark_cursor_workspace_trusted(workspace: &Path) -> std::io::Result<()> {
    match dirs::home_dir() {
        Some(home) => mark_cursor_workspace_trusted_under(&home, workspace),
        None => Ok(()),
    }
}

/// The write itself, under an explicit home — the seam the tests stand on,
/// so no test has to mutate the process-global `HOME` under a threaded
/// suite's feet.
fn mark_cursor_workspace_trusted_under(home: &Path, workspace: &Path) -> std::io::Result<()> {
    let absolute = canonical_or_raw(workspace);
    let slug = cursor_workspace_slug(&absolute.to_string_lossy());
    if slug.is_empty() {
        return Ok(());
    }
    let dir = home.join(".cursor").join("projects").join(slug);
    let file = dir.join(".workspace-trusted");
    if file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    let payload = serde_json::json!({
        "trustedAt": iso_utc_now(),
        "workspacePath": absolute.to_string_lossy(),
    });
    write_atomically(&file, &format!("{:#}\n", payload))
}

/// The `~/.cursor/projects/<slug>` directory name: the absolute path, leading
/// separators stripped, every separator run — and the characters Windows
/// forbids in file names — folded to `-` (agent-trust-presets.ts:171-177).
fn cursor_workspace_slug(absolute: &str) -> String {
    let stripped = absolute.trim_start_matches(['\\', '/']);
    let mut slug = String::with_capacity(stripped.len());
    let mut folding = false;
    for ch in stripped.chars() {
        if matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            if !folding {
                slug.push('-');
                folding = true;
            }
        } else {
            slug.push(ch);
            folding = false;
        }
    }
    slug
}

/// Copilot keeps a global `trustedFolders` array in `~/.copilot/config.json`,
/// compared after realpath resolution (agent-trust-presets.ts:59-101). The
/// array is appended in place so every other key survives untouched; a
/// config.json that will not parse is the user's to fix, and this
/// side-effect path refuses to overwrite it.
fn mark_copilot_folder_trusted(workspace: &Path) -> std::io::Result<()> {
    match dirs::home_dir() {
        Some(home) => mark_copilot_folder_trusted_under(&home, workspace),
        None => Ok(()),
    }
}

/// The write under an explicit home, for `mark_cursor_workspace_trusted_under`'s
/// reason.
fn mark_copilot_folder_trusted_under(home: &Path, workspace: &Path) -> std::io::Result<()> {
    let absolute = canonical_or_raw(workspace);
    let absolute = absolute.to_string_lossy().into_owned();
    let dir = home.join(".copilot");
    let file = dir.join("config.json");
    let mut config = match std::fs::read_to_string(&file) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(map)) => map,
            // Parsed but not an object: Orca starts over from `{}` here —
            // only a file that will not parse at all is left alone.
            Ok(_) => serde_json::Map::new(),
            Err(_) => return Ok(()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(error) => return Err(error),
    };
    let existing = config
        .get("trustedFolders")
        .and_then(|held| held.as_array())
        .cloned()
        .unwrap_or_default();
    let already = existing.iter().any(|entry| {
        entry
            .as_str()
            .is_some_and(|folder| canonical_or_raw(Path::new(folder)).to_string_lossy() == absolute)
    });
    if already {
        return Ok(());
    }
    // Non-string entries are dropped, the way Orca's filter drops them — a
    // list Copilot itself string-compares has no other kind of member.
    let mut next: Vec<serde_json::Value> = existing
        .into_iter()
        .filter(|entry| entry.is_string())
        .collect();
    next.push(serde_json::Value::String(absolute));
    config.insert("trustedFolders".to_string(), serde_json::Value::Array(next));
    std::fs::create_dir_all(&dir)?;
    write_atomically(&file, &format!("{:#}\n", serde_json::Value::Object(config)))
}

/// Codex records trust per project root in `config.toml` —
/// `[projects."<realpath>"] trust_level = "trusted"` — and treats a worktree
/// as its repository (agent-trust-presets.ts:103-118). Every config the
/// launched Codex may read gets the same line.
fn mark_codex_project_trusted(workspace: &Path, configs: &[PathBuf]) -> std::io::Result<()> {
    let root = repository_trust_root(workspace);
    let root = root.to_string_lossy();
    for config in configs {
        zerocode_hookd::codex_mirror::upsert_project_trust_level(
            config,
            &root,
            zerocode_hookd::codex_mirror::ProjectTrust::Trusted,
        )?;
    }
    Ok(())
}

/// The directory Codex and Claude Code file this workspace's trust under: the
/// repository root for a linked worktree, the workspace itself for everything
/// else (`resolveCodexProjectTrustRoot`, agent-trust-presets.ts:120-154).
///
/// The walk is deliberately suspicious, in Orca's exact steps: the `.git`
/// file's `gitdir:` must land in a `worktrees/` directory, and that
/// directory's own `gitdir` backlink must point back at this workspace —
/// workspace-controlled `.git` metadata must not broaden trust to an
/// arbitrary directory without Git's reciprocal link. Any break in the chain
/// answers with the workspace alone, which is the narrower grant.
fn repository_trust_root(workspace: &Path) -> PathBuf {
    let absolute = canonical_or_raw(workspace);
    let Ok(reference) = std::fs::read_to_string(absolute.join(".git")) else {
        return absolute;
    };
    let Some(git_dir) = reference.trim().strip_prefix("gitdir:") else {
        return absolute;
    };
    let git_dir = git_dir.trim();
    if git_dir.is_empty() {
        return absolute;
    }
    let git_dir = lexical_resolve(&absolute, git_dir);
    let Some(worktrees_dir) = git_dir.parent() else {
        return absolute;
    };
    if worktrees_dir.file_name().and_then(|name| name.to_str()) != Some("worktrees") {
        return absolute;
    }
    let Ok(backlink) = std::fs::read_to_string(git_dir.join("gitdir")) else {
        return absolute;
    };
    let backlink = backlink.trim();
    if backlink.is_empty() {
        return absolute;
    }
    let resolved_backlink = lexical_resolve(&git_dir, backlink);
    let workspace_git_file = absolute.join(".git");
    if resolved_backlink != workspace_git_file
        && canonical_or_raw(&resolved_backlink) != canonical_or_raw(&workspace_git_file)
    {
        return absolute;
    }
    let Some(repo_root) = worktrees_dir.parent().and_then(Path::parent) else {
        return absolute;
    };
    canonical_or_raw(repo_root)
}

/// `resolve(base, path)` the way Node does it — lexically, no disk asked.
fn lexical_resolve(base: &Path, path: &str) -> PathBuf {
    let raw = Path::new(path);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    };
    let mut resolved = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

/// The `config.toml`s the launch this environment describes will read: the
/// real `~/.codex` always, and the mirror home when the launch hands one out
/// under `CODEX_HOME` — the same pair Orca writes, for the same reason: a
/// preset in a config the spawned Codex never opens is no preset.
fn codex_configs_of(env: &[(String, String)]) -> Vec<PathBuf> {
    let mut configs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        configs.push(home.join(".codex").join("config.toml"));
    }
    if let Some((_, mirror)) = env.iter().find(|(name, _)| name == "CODEX_HOME") {
        configs.push(PathBuf::from(mirror).join("config.toml"));
    }
    configs
}

/// realpath when the path exists, the raw path when it does not — every
/// trust comparator on the agents' side runs realpath first, and macOS's
/// `/tmp` vs `/private/tmp` is the case that bites
/// (agent-trust-presets.ts:156-169).
fn canonical_or_raw(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The moment, spelled the way `new Date().toISOString()` spells it —
/// cursor's own writer stamps its marker in exactly this shape.
fn iso_utc_now() -> String {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    iso_utc_of(i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Temp-and-rename, Orca's `writeFileAtomically` — a trust file must never
/// be readable half-written, because the agent reads it at its own startup.
///
/// The replacement keeps the replaced file's permissions: Claude Code's global
/// config is owner-only, and a rename must not hand it the umask's.
fn write_atomically(path: &Path, body: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temp = dir.join(format!(".zerocode-trust-{}.tmp", std::process::id()));
    std::fs::write(&temp, body)?;
    if let Ok(meta) = std::fs::metadata(path)
        && let Err(error) = std::fs::set_permissions(&temp, meta.permissions())
    {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    let renamed = std::fs::rename(&temp, path);
    if renamed.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    renamed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_with_a_measured_trust_menu_has_a_preset() {
        assert_eq!(preset_of("zo"), Some(TrustPreset::Zo));
        assert_eq!(preset_of("claude"), Some(TrustPreset::Claude));
        assert_eq!(preset_of("cursor"), Some(TrustPreset::Cursor));
        assert_eq!(preset_of("copilot"), Some(TrustPreset::Copilot));
        assert_eq!(preset_of("codex"), Some(TrustPreset::Codex));
        assert_eq!(preset_of("antigravity"), Some(TrustPreset::Antigravity));
        for other in ["kimi", "openclaude", "grok", ""] {
            assert_eq!(preset_of(other), None, "{other} grew a preset");
        }
    }

    /// A linked worktree whose `.git` and backlink agree, under `temp`.
    fn linked_worktree(temp: &Path) -> (PathBuf, PathBuf) {
        let repo = temp.join("repo");
        let git_dir = repo.join(".git").join("worktrees").join("feature");
        let worktree = temp.join("checkouts").join("feature");
        std::fs::create_dir_all(&git_dir).expect("git dir");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("link");
        std::fs::write(
            git_dir.join("gitdir"),
            format!("{}\n", worktree.join(".git").display()),
        )
        .expect("backlink");
        (repo, worktree)
    }

    /// A Claude worker in a repository its config never trusted is trusted
    /// before it starts — under the repository, where Claude's own accept
    /// writes it — and nothing else in the file moves.
    #[test]
    fn claude_trusts_the_repository_of_a_worktree_and_keeps_the_rest_of_its_config() {
        let temp = tempfile::tempdir().expect("sandbox");
        let (repo, worktree) = linked_worktree(temp.path());
        let config = temp.path().join(".claude.json");
        std::fs::write(
            &config,
            "{\"numStartups\": 7, \"projects\": {\"/elsewhere\": {\"allowedTools\": [\"Bash\"]}}}\n",
        )
        .expect("seed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600))
                .expect("owner-only");
        }

        mark_workspace_trusted(
            TrustPreset::Claude,
            &worktree,
            &[(
                zerocode_core::account::CONFIG_DIR_VAR.to_string(),
                temp.path().to_string_lossy().into_owned(),
            )],
        )
        .expect("the preset writes");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        let root = canonical_or_raw(&repo).to_string_lossy().into_owned();
        assert_eq!(
            written["projects"][root.as_str()][CLAUDE_TRUST_ACCEPTED],
            true,
            "{written}"
        );
        assert_eq!(written["numStartups"], 7, "{written}");
        assert_eq!(
            written["projects"]["/elsewhere"]["allowedTools"][0], "Bash",
            "{written}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&config)
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the rewrite loosened the config to {mode:o}");
        }
        assert!(
            !temp.path().join(".claude.json.lock").exists(),
            "the lock outlived the write"
        );

        // Asked again: already trusted, so the file is not rewritten at all.
        let stamp = std::fs::metadata(&config)
            .expect("meta")
            .modified()
            .expect("mtime");
        std::thread::sleep(Duration::from_millis(20));
        mark_claude_project_trusted(&worktree, &config).expect("second pass");
        assert_eq!(
            std::fs::metadata(&config)
                .expect("meta")
                .modified()
                .expect("mtime"),
            stamp,
            "an answered question was written again"
        );
    }

    /// The lock Claude Code holds is waited on, never written through; an
    /// abandoned one is taken over; a config that is not there or will not
    /// parse is left exactly as it is.
    #[test]
    fn claude_writes_only_under_its_own_lock_and_never_authors_or_repairs_a_config() {
        let temp = tempfile::tempdir().expect("sandbox");
        let workspace = temp.path().join("w");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let config = temp.path().join(".claude.json");

        // No config yet: nothing is created.
        mark_claude_project_trusted(&workspace, &config).expect("absent is not an error");
        assert!(
            !config.exists(),
            "a global config was authored from nothing"
        );

        // A live holder: the write waits out its patience and gives up, untouched.
        std::fs::write(&config, "{}\n").expect("seed");
        let lock = temp.path().join(".claude.json.lock");
        std::fs::create_dir(&lock).expect("held by Claude");
        let refused = mark_claude_project_trusted_within(
            &workspace,
            &config,
            Duration::from_millis(60),
            CLAUDE_CONFIG_LOCK_STALE,
        );
        assert_eq!(
            refused.as_ref().map_err(std::io::Error::kind).err(),
            Some(std::io::ErrorKind::WouldBlock),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_to_string(&config).expect("read"), "{}\n");
        assert!(lock.exists(), "somebody else's lock was removed");

        // The same lock, abandoned: taken over, written, and released.
        mark_claude_project_trusted_within(&workspace, &config, Duration::ZERO, Duration::ZERO)
            .expect("an abandoned lock is taken over");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        let root = canonical_or_raw(&workspace).to_string_lossy().into_owned();
        assert_eq!(
            written["projects"][root.as_str()][CLAUDE_TRUST_ACCEPTED],
            true,
            "{written}"
        );
        assert!(!lock.exists(), "the lock outlived the write");

        // A config that will not parse is an error, and stays as it was.
        std::fs::write(&config, "{ not json").expect("corrupt");
        let other = temp.path().join("other");
        std::fs::create_dir_all(&other).expect("other");
        assert!(mark_claude_project_trusted(&other, &config).is_err());
        assert_eq!(
            std::fs::read_to_string(&config).expect("read"),
            "{ not json"
        );
    }

    /// The config a launched Claude reads, by its own rule.
    #[test]
    fn the_claude_config_is_the_one_the_launched_process_will_read() {
        let temp = tempfile::tempdir().expect("sandbox");
        let home = temp.path().join("home");
        let account = temp.path().join("account");
        std::fs::create_dir_all(home.join(".claude")).expect("home");
        std::fs::create_dir_all(&account).expect("account");
        let named = Some(account.to_string_lossy().into_owned());

        // The launch's CLAUDE_CONFIG_DIR wins over the inherited one.
        assert_eq!(
            claude_config_under(
                named.clone(),
                Some(temp.path().join("inherited")),
                Some(home.clone())
            ),
            Some(account.join(".claude.json"))
        );
        // Inherited when the launch says nothing; the home when nobody does.
        assert_eq!(
            claude_config_under(None, Some(account.clone()), Some(home.clone())),
            Some(account.join(".claude.json"))
        );
        assert_eq!(
            claude_config_under(None, None, Some(home.clone())),
            Some(home.join(".claude.json"))
        );
        // An empty value in the launch env is the lane's removal: the child
        // falls back to the home, not to what this process inherited.
        assert_eq!(
            claude_config_under(
                Some(String::new()),
                Some(account.clone()),
                Some(home.clone())
            ),
            Some(home.join(".claude.json"))
        );
        // A legacy `.config.json` in the config home is what Claude reads first.
        std::fs::write(account.join(".config.json"), "{}").expect("legacy");
        assert_eq!(
            claude_config_under(named, None, Some(home)),
            Some(account.join(".config.json"))
        );
    }

    #[test]
    fn zo_adds_only_the_new_workspace_and_preserves_a_narrower_answer() {
        let home = tempfile::tempdir().expect("home");
        let first = home.path().join("work-one");
        let second = home.path().join("work-two");
        std::fs::create_dir_all(&first).expect("first");
        std::fs::create_dir_all(&second).expect("second");
        let config_dir = home.path().join(".zo");
        std::fs::create_dir_all(&config_dir).expect("zo dir");
        let config = config_dir.join("trusted_workspaces.json");
        std::fs::write(
            &config,
            format!(
                "{{\"{}\":\"prompt\",\"/kept\":\"workspace-write\"}}\n",
                canonical_or_raw(&first).display()
            ),
        )
        .expect("seed");

        mark_zo_workspace_trusted_under(home.path(), &first).expect("keep narrow");
        mark_zo_workspace_trusted_under(home.path(), &second).expect("add second");
        let trusted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(config).expect("read")).expect("json");
        assert_eq!(
            trusted[canonical_or_raw(&first).to_string_lossy().as_ref()],
            "prompt"
        );
        assert_eq!(
            trusted[canonical_or_raw(&second).to_string_lossy().as_ref()],
            "danger-full-access"
        );
        assert_eq!(trusted["/kept"], "workspace-write");
    }

    #[test]
    fn the_cursor_slug_is_the_path_with_hostile_characters_folded() {
        assert_eq!(cursor_workspace_slug("/w/repo"), "w-repo");
        // Separator runs and Windows-forbidden characters fold to ONE dash,
        // the way the CLI's own `[\\/:*?"<>|]+` does.
        assert_eq!(
            cursor_workspace_slug("C:\\Users\\j\\my?repo"),
            "C-Users-j-my-repo"
        );
        assert_eq!(cursor_workspace_slug("//host/share"), "host-share");
    }

    #[test]
    fn the_timestamp_is_spelled_like_a_javascript_iso_string() {
        let now = iso_utc_now();
        let bytes = now.as_bytes();
        assert_eq!(now.len(), 24, "{now}");
        assert_eq!(bytes[10], b'T', "{now}");
        assert_eq!(bytes[23], b'Z', "{now}");
        assert_eq!(bytes[19], b'.', "{now}");
        // A date this window can vouch for: the epoch arithmetic against a
        // known day (2026-08-18 is day 20683).
    }

    #[test]
    fn a_worktree_trusts_its_repository_and_a_forged_one_trusts_only_itself() {
        let temp = tempfile::tempdir().expect("sandbox");
        let repo = temp.path().join("repo");
        let git_dir = repo.join(".git").join("worktrees").join("feature");
        let worktree = temp.path().join("checkouts").join("feature");
        std::fs::create_dir_all(&git_dir).expect("git dir");
        std::fs::create_dir_all(&worktree).expect("worktree");

        // No .git file at all: the workspace is its own root.
        let plain = temp.path().join("plain");
        std::fs::create_dir_all(&plain).expect("plain");
        assert_eq!(repository_trust_root(&plain), canonical_or_raw(&plain));

        // The honest pair of links: worktree -> gitdir, gitdir -> worktree.
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("link");
        std::fs::write(
            git_dir.join("gitdir"),
            format!("{}\n", worktree.join(".git").display()),
        )
        .expect("backlink");
        assert_eq!(
            repository_trust_root(&worktree),
            canonical_or_raw(&repo),
            "a linked worktree no longer trusts its repository"
        );

        // A forged .git pointing into somebody else's worktrees/ — the
        // backlink names another checkout, so the grant stays narrow.
        std::fs::write(
            git_dir.join("gitdir"),
            format!("{}\n", temp.path().join("elsewhere").join(".git").display()),
        )
        .expect("forged backlink");
        assert_eq!(
            repository_trust_root(&worktree),
            canonical_or_raw(&worktree),
            "workspace-controlled .git metadata broadened trust"
        );

        // A gitdir outside any worktrees/ directory — the main checkout's
        // ordinary shape — is its own root too.
        let stray = temp.path().join("stray");
        std::fs::create_dir_all(&stray).expect("stray");
        std::fs::write(
            stray.join(".git"),
            format!("gitdir: {}\n", temp.path().join("elsewhere-git").display()),
        )
        .expect("stray link");
        assert_eq!(repository_trust_root(&stray), canonical_or_raw(&stray));
    }

    #[test]
    fn cursor_writes_its_marker_once_and_leaves_a_standing_answer_alone() {
        let temp = tempfile::tempdir().expect("sandbox");
        let home = temp.path().join("home");
        let workspace = temp.path().join("w");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&workspace).expect("workspace");

        mark_cursor_workspace_trusted_under(&home, &workspace).expect("cursor marker");
        let slug = cursor_workspace_slug(&canonical_or_raw(&workspace).to_string_lossy());
        let marker = home
            .join(".cursor")
            .join("projects")
            .join(&slug)
            .join(".workspace-trusted");
        let payload = std::fs::read_to_string(&marker).expect("payload");
        assert!(
            payload.contains("\"trustedAt\"")
                && payload.contains("\"workspacePath\"")
                && payload.ends_with("}\n"),
            "{payload}"
        );

        std::fs::write(&marker, "{\"theirs\": true}\n").expect("their marker");
        mark_cursor_workspace_trusted_under(&home, &workspace).expect("second pass");
        assert_eq!(
            std::fs::read_to_string(&marker).expect("kept"),
            "{\"theirs\": true}\n",
            "an existing answer was rewritten"
        );
    }

    #[test]
    fn copilot_appends_without_touching_the_rest_and_refuses_a_corrupt_file() {
        let temp = tempfile::tempdir().expect("sandbox");
        let home = temp.path().join("home");
        let workspace = temp.path().join("w");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let config_file = home.join(".copilot").join("config.json");
        std::fs::create_dir_all(config_file.parent().expect("dir")).expect("copilot dir");
        std::fs::write(
            &config_file,
            "{\"loggedInUsers\": [\"me\"], \"trustedFolders\": [\"/elsewhere\", 7]}\n",
        )
        .expect("seed");

        mark_copilot_folder_trusted_under(&home, &workspace).expect("append");
        let config = std::fs::read_to_string(&config_file).expect("read");
        let reread: serde_json::Value = serde_json::from_str(&config).expect("json");
        assert_eq!(reread["loggedInUsers"][0], "me", "{reread}");
        let folders = reread["trustedFolders"].as_array().expect("folders");
        let canonical_workspace = canonical_or_raw(&workspace);
        assert!(
            folders.contains(&serde_json::Value::String("/elsewhere".into()))
                && folders.contains(&serde_json::Value::String(
                    canonical_workspace.to_string_lossy().into_owned()
                )),
            "{reread}"
        );
        assert!(
            !folders.contains(&serde_json::Value::Number(7.into())),
            "a non-string member survived the append: {reread}"
        );

        // Already trusted: the second pass is no write at all.
        mark_copilot_folder_trusted_under(&home, &workspace).expect("second pass");
        assert_eq!(
            std::fs::read_to_string(&config_file).expect("after"),
            config,
            "an already-trusted folder was appended again"
        );

        // A corrupt file is refused, not replaced — it is the user's to fix.
        std::fs::write(&config_file, "{not json").expect("corrupt");
        mark_copilot_folder_trusted_under(&home, &workspace).expect("refusal is not an error");
        assert_eq!(
            std::fs::read_to_string(&config_file)
                .expect("kept")
                .as_str(),
            "{not json",
            "a corrupt config.json was overwritten"
        );

        // And a missing one is simply begun.
        let fresh_home = temp.path().join("fresh");
        std::fs::create_dir_all(&fresh_home).expect("fresh home");
        mark_copilot_folder_trusted_under(&fresh_home, &workspace).expect("fresh write");
        let begun: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fresh_home.join(".copilot").join("config.json"))
                .expect("begun"),
        )
        .expect("begun json");
        assert_eq!(
            begun["trustedFolders"][0],
            serde_json::Value::String(canonical_workspace.to_string_lossy().into_owned()),
            "{begun}"
        );
    }

    #[test]
    fn antigravity_appends_the_exact_workspace_once_and_preserves_every_other_setting() {
        let temp = tempfile::tempdir().expect("sandbox");
        let home = temp.path().join("home");
        let physical = temp.path().join("physical-worktree");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&physical).expect("worktree");
        let config = home
            .join(".gemini")
            .join("antigravity-cli")
            .join("settings.json");
        std::fs::create_dir_all(config.parent().expect("settings dir")).expect("settings dir");
        std::fs::write(
            &config,
            r#"{
  "agentMode": "auto",
  "colorScheme": "dark",
  "useG1Credits": false,
  "trustedWorkspaces": ["/kept"]
}
"#,
        )
        .expect("seed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600))
                .expect("private seed");
        }

        #[cfg(unix)]
        let workspace = {
            let alias = temp.path().join("worktree-alias");
            std::os::unix::fs::symlink(&physical, &alias).expect("workspace symlink");
            alias.join(".")
        };
        #[cfg(not(unix))]
        let workspace = physical.clone();

        mark_antigravity_workspace_trusted_under(&home, &workspace).expect("first append");
        let first = std::fs::read_to_string(&config).expect("first settings");
        mark_antigravity_workspace_trusted_under(&home, &workspace).expect("idempotent append");
        let second = std::fs::read_to_string(&config).expect("second settings");
        assert_eq!(second, first, "the idempotent pass rewrote settings");

        let settings: serde_json::Value = serde_json::from_str(&second).expect("valid settings");
        assert_eq!(settings["agentMode"], "auto");
        assert_eq!(settings["colorScheme"], "dark");
        assert_eq!(settings["useG1Credits"], false);
        assert_eq!(
            settings["trustedWorkspaces"],
            serde_json::json!([
                "/kept",
                canonical_or_raw(&workspace).to_string_lossy().into_owned()
            ])
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&config)
                    .expect("settings metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "the private settings file became broader"
            );
        }
    }

    #[test]
    fn antigravity_reports_a_malformed_settings_file_without_replacing_it() {
        let home = tempfile::tempdir().expect("home");
        let workspace = tempfile::tempdir().expect("workspace");
        let config = home
            .path()
            .join(".gemini")
            .join("antigravity-cli")
            .join("settings.json");
        std::fs::create_dir_all(config.parent().expect("settings dir")).expect("settings dir");
        std::fs::write(&config, "{not json").expect("corrupt settings");

        let error = mark_antigravity_workspace_trusted_under(home.path(), workspace.path())
            .expect_err("a malformed settings file was silently accepted");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
        assert!(error.to_string().contains("settings.json"), "{error}");
        assert_eq!(
            std::fs::read_to_string(config).expect("kept settings"),
            "{not json",
            "the malformed settings file was replaced"
        );
    }
}
