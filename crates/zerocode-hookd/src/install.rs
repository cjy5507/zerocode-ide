//! Putting the hook into each agent's OWN configuration — the half of
//! "installed" that lives in files this window does not own.
//!
//! The bridge and the scripts are ours; what makes an agent actually call them
//! is an entry in that agent's own settings file, spelled the way that agent
//! spells hooks. Measured off Orca 1.4.169 (managed-agent-hook-controls chunk),
//! which manages agents through five distinct mechanisms; the nine
//! whose mechanism is a settings merge are implemented here:
//!
//! | agent        | file                              | shape                              |
//! |--------------|-----------------------------------|------------------------------------|
//! | claude       | `~/.claude/settings.json`         | nested `hooks{}`, 11 events        |
//! | cursor       | `~/.cursor/hooks.json`            | flat `{command,timeout}`, version 1|
//! | droid        | `~/.factory/settings.json`        | nested, 8 events                   |
//! | command-code | `~/.commandcode/settings.json`    | nested, 3 events, `.*` matcher     |
//! | grok         | `<grok home>/hooks/…json`         | nested, 9 events, own file         |
//! | copilot      | `<copilot home>/hooks/…json`      | `{type,bash,timeoutSec:5}`, 13 ev. |
//! | devin        | `~/.config/devin/config.json`     | nested, 8 events                   |
//! | antigravity  | `~/.gemini/config/hooks.json`     | bundle key, per-event env          |
//! | kimi         | `<kimi home>/config.toml`         | marked `[[hooks]]` block           |
//!
//! Codex is absent from this list because a settings merge is not enough for it:
//! its entries must also be TRUSTED in `config.toml` or the agent stops and asks.
//! It has its own two-lane installer in [`crate::codex_install`], reaching a
//! mirrored `CODEX_HOME` when the trust cannot be granted. Amp, Hermes,
//! OpenCode, mimo-code, pi and omp take JS plugins rather than settings entries —
//! also their own slice.
//!
//! ## The two promises every mechanism keeps
//!
//! **Only our own entries are ever touched.** A managed entry is recognized by
//! its command carrying `.zerocode/agent-hooks/<script>` — the directory
//! qualified on purpose, where Orca matches any `agent-hooks/<script>`: this
//! machine may well have Orca installed beside us, and a matcher loose enough
//! to recognize ITS entries would make our uninstall break its hooks.
//!
//! **A config that cannot be parsed is never written.** Rewriting a file we
//! could not read is how a product deletes a user's settings. The status says
//! `Error` and the file stays exactly as it was.
//!
//! ## What the installed command looks like
//!
//! Not the script path bare, but a guard around it:
//!
//! ```sh
//! if [ -f '<script>' ] && [ -r '<script>' ] && [ -x '<script>' ]; then
//!   /bin/sh '<script>'; else { command -p cat 2>/dev/null || cat; } >/dev/null 2>&1 || :; fi
//! ```
//!
//! The else-branch is the part that matters: if this app is uninstalled and the
//! script is gone, the entry left behind DRAINS the agent's stdin and exits 0 —
//! the agent keeps working with a dead hook instead of hanging on a write to a
//! command that never reads. Uninstalling us must never break the tools we
//! watched.
//!
//! On a Windows host the script is a batch file (`<slug>-hook.cmd`,
//! [`crate::hook_script_cmd`]) and the command is cmd's spelling of the same
//! guard:
//!
//! ```text
//! cmd.exe /d /c "if exist "<script>" (set "K=v" && call "<script>") else (more >nul)"
//! ```
//!
//! — the drain is `more`, a filter and not an interpreter. The host is a
//! property of the [`InstallPaths`] ([`ScriptHost`]) rather than a `cfg`, so
//! both spellings are pinned by tests on every platform. Copilot is the one
//! vendor whose definition names its interpreter (`bash`), so its command is
//! the POSIX one on every host. Which shell the other vendors run a hook
//! through on Windows has not been measured; cmd is the assumption written
//! down here, and a vendor found to run `sh` is one line in [`ScriptHost::for_vendor`].

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};
use zerocode_core::AgentKind;

/// The agents whose hook installation this module knows how to do.
pub const MANAGED_TARGETS: [AgentKind; 9] = [
    AgentKind::Claude,
    AgentKind::Cursor,
    AgentKind::Droid,
    AgentKind::CommandCode,
    AgentKind::Grok,
    AgentKind::Copilot,
    AgentKind::Devin,
    AgentKind::Antigravity,
    AgentKind::Kimi,
];

/// Which shell runs the installed command — and so which script dialect is
/// written and how the guard around it is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptHost {
    /// `/bin/sh` and a `<slug>-hook.sh` (unix, Git Bash, WSL).
    Posix,
    /// `cmd.exe` and a `<slug>-hook.cmd` (a Windows host).
    Cmd,
}

impl ScriptHost {
    /// The host this build runs on.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Cmd
        } else {
            Self::Posix
        }
    }

    /// The host a vendor's hook command runs under, given this machine's.
    /// Copilot's definition names `bash`, so it is POSIX everywhere.
    #[must_use]
    pub const fn for_vendor(self, agent: AgentKind) -> Self {
        match agent {
            AgentKind::Copilot => Self::Posix,
            _ => self,
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::Posix => "sh",
            Self::Cmd => "cmd",
        }
    }
}

/// Hook timeouts. Nested-family files count seconds and Copilot spells its own
/// field. All measured, none negotiable —
/// a unit error here is an agent that kills every hook instantly or waits
/// three hours on one.
const TIMEOUT_SECONDS: u64 = 10;
const COPILOT_TIMEOUT_SECONDS: u64 = 5;

/// One agent's answer: is the hook in its config, and if not, why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookStatus {
    pub agent: AgentKind,
    pub state: HookInstallState,
    pub config_path: String,
    /// Free text, for the cases only this side can name: a path, a trust verdict,
    /// an io error. Untranslated by nature — there is no catalog entry for
    /// "`/h/.codex/hooks.json` could not be read at offset 12".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// A note the WINDOW writes, named rather than written.
    ///
    /// Sentences this side can predict belong in the catalogs, and a sentence
    /// this side spells out is a sentence that stays Korean in the other three.
    /// So the fixed ones travel as a token and the window maps it with a literal
    /// `t()` — literal because a computed catalog key is invisible to the scanner
    /// that finds missing translations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<HookNote>,
}

/// The fixed things a hook row can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookNote {
    /// Installed, but in a `CODEX_HOME` this window owns — so it runs for agents
    /// launched here and not for the user's own `codex`.
    MirrorOnly,
    /// Written where the agent will see it, and NOT trusted. The agent will stop
    /// and ask instead of running it.
    NeedsTrust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookInstallState {
    Installed,
    /// Some events carry the hook and some do not — a person edited around us,
    /// or an older install knew fewer events.
    Partial,
    NotInstalled,
    /// The config exists and could not be read. Nothing was written.
    Error,
}

/// Where the generated scripts live: `~/.zerocode/agent-hooks/`. Beside the
/// state directory, not in it — the state directory is ours to clean, and a
/// cleanup that deleted the scripts would leave every agent config pointing at
/// the drain branch.
pub fn scripts_dir(home: &Path) -> PathBuf {
    home.join(".zerocode").join("agent-hooks")
}

pub fn script_file_name(agent: AgentKind) -> String {
    script_file_name_for(agent, ScriptHost::current().for_vendor(agent))
}

pub fn script_file_name_for(agent: AgentKind, host: ScriptHost) -> String {
    format!("{}-hook.{}", agent.slug(), host.extension())
}

pub fn script_path(home: &Path, agent: AgentKind) -> PathBuf {
    scripts_dir(home).join(script_file_name(agent))
}

pub fn script_path_for(home: &Path, agent: AgentKind, host: ScriptHost) -> PathBuf {
    scripts_dir(home).join(script_file_name_for(agent, host))
}

/// The needle that recognizes one of OUR entries — dir-qualified so another
/// product's `agent-hooks/claude-hook.sh` is not ours to remove, with the
/// extension left open the way Orca leaves it (a `.cmd` written by a Windows
/// install of this app is still ours to clean from a synced config).
fn managed_needle(agent: AgentKind) -> String {
    format!(".zerocode/agent-hooks/{}-hook.", agent.slug())
}

/// A Windows install spells the same path with backslashes; one spelling
/// before the needle looks, so an entry written on either host is
/// recognized on both.
fn is_managed_command(command: &str, agent: AgentKind) -> bool {
    command.replace('\\', "/").contains(&managed_needle(agent))
}

/// `'…'`-quote for /bin/sh, the only quoting a path in a command gets.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The guard-wrapped command described in the module doc. `env` prefixes
/// `K='v'` assignments for the agents whose script needs telling which event
/// fired.
pub fn posix_wrapper(script: &Path, env: &[(&str, &str)]) -> String {
    let quoted = sh_quote(&script.to_string_lossy());
    let prefix = env
        .iter()
        .map(|(key, value)| format!("{key}={} ", sh_quote(value)))
        .collect::<String>();
    format!(
        "if [ -f {quoted} ] && [ -r {quoted} ] && [ -x {quoted} ]; then \
         {prefix}/bin/sh {quoted}; \
         else {{ command -p cat 2>/dev/null || cat; }} >/dev/null 2>&1 || :; fi"
    )
}

/// cmd's spelling of the guarded command: the script when it is there, a
/// drain (`more`, a filter, not an interpreter) when it is not. `env`
/// prefixes `set "K=v" &&` for the agents whose script needs telling which
/// event fired. The outer quotes are cmd's own rule for `/c`: first and last
/// stripped, everything between them kept as written.
pub fn cmd_wrapper(script: &Path, env: &[(&str, &str)]) -> String {
    let path = script.to_string_lossy().replace('/', "\\");
    let prefix = env
        .iter()
        .map(|(key, value)| format!("set \"{key}={value}\" && "))
        .collect::<String>();
    format!("cmd.exe /d /c \"if exist \"{path}\" ({prefix}call \"{path}\") else (more >nul)\"")
}

/// The guarded command in the host's spelling.
pub fn wrapper(host: ScriptHost, script: &Path, env: &[(&str, &str)]) -> String {
    match host {
        ScriptHost::Posix => posix_wrapper(script, env),
        ScriptHost::Cmd => cmd_wrapper(script, env),
    }
}

/// Write the agent's script where the installed command points, in the
/// host's dialect.
pub fn write_script(home: &Path, agent: AgentKind) -> std::io::Result<()> {
    write_script_for(home, agent, ScriptHost::current().for_vendor(agent))
}

pub fn write_script_for(home: &Path, agent: AgentKind, host: ScriptHost) -> std::io::Result<()> {
    let path = script_path_for(home, agent, host);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = match host {
        ScriptHost::Posix => crate::hook_script(agent),
        ScriptHost::Cmd => crate::hook_script_cmd(agent),
    };
    std::fs::write(&path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- json files

/// A missing file is an empty config; an unreadable or non-object file is
/// `None`, and `None` means nothing gets written.
fn read_config(path: &Path) -> Option<Map<String, Value>> {
    if !path.exists() {
        return Some(Map::new());
    }
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn write_config(path: &Path, config: &Map<String, Value>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(config.clone()))
        .expect("a JSON map serializes");
    text.push('\n');
    // Through a sibling temp and a rename: the agent may read its settings at
    // any moment, and half a settings file is a parse error that turns off
    // every hook the user wrote too.
    let tmp = path.with_extension("json.zerocode-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Does this hook definition (in any of the vendor shapes) run one of our
/// commands? Checks the nested `hooks[].command`, the flat `command`, and
/// Copilot's `bash` — one predicate, because remove has to recognize every
/// shape an older install may have written.
fn definition_is_managed(definition: &Value, agent: AgentKind) -> bool {
    let record = match definition.as_object() {
        Some(record) => record,
        None => return false,
    };
    for field in ["command", "bash", "powershell"] {
        if let Some(command) = record.get(field).and_then(Value::as_str)
            && is_managed_command(command, agent)
        {
            return true;
        }
    }
    record
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| is_managed_command(command, agent))
            })
        })
}

/// Strip our entries out of one event's definition list.
fn without_managed(definitions: &[Value], agent: AgentKind) -> Vec<Value> {
    definitions
        .iter()
        .filter(|definition| !definition_is_managed(definition, agent))
        .cloned()
        .collect()
}

/// Remove our entries from every event under `hooks_key`, dropping event keys
/// that end up empty. The user's own entries pass through untouched — that is
/// the entire job.
fn clean_hooks_object(hooks: &mut Map<String, Value>, agent: AgentKind) {
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(definitions) = hooks.get(&event).and_then(Value::as_array) else {
            continue;
        };
        let kept = without_managed(definitions, agent);
        if kept.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.insert(event, Value::Array(kept));
        }
    }
}

// -------------------------------------------------- the nested-hooks family

/// One event in a nested-hooks config: its name, and the matcher when the
/// vendor requires one on tool events.
struct NestedEvent {
    name: &'static str,
    matcher: Option<&'static str>,
}

const fn plain(name: &'static str) -> NestedEvent {
    NestedEvent {
        name,
        matcher: None,
    }
}

const fn matched(name: &'static str, matcher: &'static str) -> NestedEvent {
    NestedEvent {
        name,
        matcher: Some(matcher),
    }
}

/// Claude's eleven, measured (`CLAUDE_EVENTS`, chunk :1581-1663 and Orca's
/// `hook-settings.ts:36-42`). The four tool events carry `matcher: "*"`;
/// without it Claude installs the hook for no tool at all.
const CLAUDE_EVENTS: &[NestedEvent] = &[
    // First, as Orca lists it first with the reason attached (STA-3386):
    // the ONE event a resumed or idle session emits before its first
    // prompt. Without it a reopened claude reports nothing and the pane
    // wears a phantom "working" spinner until somebody types.
    plain("SessionStart"),
    plain("UserPromptSubmit"),
    plain("Stop"),
    plain("StopFailure"),
    plain("SubagentStart"),
    plain("SubagentStop"),
    plain("TeammateIdle"),
    matched("PreToolUse", "*"),
    matched("PostToolUse", "*"),
    matched("PostToolUseFailure", "*"),
    matched("PermissionRequest", "*"),
];

const DROID_EVENTS: &[NestedEvent] = &[
    plain("SessionStart"),
    plain("UserPromptSubmit"),
    plain("Stop"),
    plain("SubagentStop"),
    matched("PreToolUse", "*"),
    matched("PostToolUse", "*"),
    matched("PermissionRequest", "*"),
    plain("Notification"),
];

const COMMAND_CODE_EVENTS: &[NestedEvent] = &[
    matched("PreToolUse", ".*"),
    matched("PostToolUse", ".*"),
    plain("Stop"),
];

const GROK_EVENTS: &[NestedEvent] = &[
    plain("SessionStart"),
    plain("UserPromptSubmit"),
    plain("Stop"),
    plain("StopFailure"),
    plain("SessionEnd"),
    matched("PreToolUse", ".*"),
    matched("PostToolUse", ".*"),
    matched("PostToolUseFailure", ".*"),
    plain("Notification"),
];

const DEVIN_EVENTS: &[NestedEvent] = &[
    plain("SessionStart"),
    plain("UserPromptSubmit"),
    plain("Stop"),
    plain("PostCompaction"),
    plain("SessionEnd"),
    matched("PreToolUse", "*"),
    matched("PostToolUse", "*"),
    matched("PermissionRequest", "*"),
];

/// The nested definition every claude-family agent reads:
/// `{matcher?, hooks: [{type: "command", command, timeout}]}`.
fn nested_definition(command: &str, matcher: Option<&str>, timeout: u64) -> Value {
    let hook = json!({"type": "command", "command": command, "timeout": timeout});
    match matcher {
        Some(matcher) => json!({"matcher": matcher, "hooks": [hook]}),
        None => json!({"hooks": [hook]}),
    }
}

/// Install into a nested-hooks config: clean our entries everywhere (an event
/// we no longer install must not keep a stale one), then append one fresh
/// definition per event AFTER whatever the user has — their hooks fire first.
fn install_nested(
    config: &mut Map<String, Value>,
    agent: AgentKind,
    events: &[NestedEvent],
    script: &Path,
    host: ScriptHost,
    timeout: u64,
) {
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    clean_hooks_object(&mut hooks, agent);
    for event in events {
        let command = wrapper(host, script, &[(crate::env_var::EVENT, event.name)]);
        let mut definitions = hooks
            .get(event.name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        definitions.push(nested_definition(&command, event.matcher, timeout));
        hooks.insert(event.name.to_string(), Value::Array(definitions));
    }
    config.insert("hooks".to_string(), Value::Object(hooks));
}

/// How many of `events` currently carry one of our entries, for the
/// installed / partial / not-installed verdict.
fn nested_present_count(
    config: &Map<String, Value>,
    agent: AgentKind,
    events: &[NestedEvent],
) -> usize {
    let Some(hooks) = config.get("hooks").and_then(Value::as_object) else {
        return 0;
    };
    events
        .iter()
        .filter(|event| {
            hooks
                .get(event.name)
                .and_then(Value::as_array)
                .is_some_and(|definitions| {
                    definitions
                        .iter()
                        .any(|definition| definition_is_managed(definition, agent))
                })
        })
        .count()
}

// -------------------------------------------------------------- entry points

/// Everything path-shaped an installer needs, resolved once by the caller.
///
/// `home` is the user's home directory. The three overrides mirror the
/// environment variables the vendors themselves honor (`GROK_HOME`,
/// `COPILOT_HOME`, `KIMI_CODE_HOME`) — resolved by the caller rather than read
/// here, so this module stays pure enough to test against a temp directory.
#[derive(Debug, Clone)]
pub struct InstallPaths {
    pub home: PathBuf,
    pub grok_home: Option<PathBuf>,
    pub copilot_home: Option<PathBuf>,
    pub kimi_home: Option<PathBuf>,
    /// OTHER settings.json files Claude's hook must also live in.
    ///
    /// The account picker launches every claude with `CLAUDE_CONFIG_DIR`
    /// pointing at a per-account directory, and claude reads its hooks from
    /// THAT directory's settings.json — never from `~/.claude`. A hook
    /// installed only into the home file is a hook no launched claude runs,
    /// which is how a working, listening bridge still drew no agent rows
    /// ("실행중인 agent claude 표시가 안나와"): the events simply never
    /// fired. The caller names the account files because only it knows where
    /// its accounts live; this module stays testable against a temp dir.
    pub claude_extra_configs: Vec<PathBuf>,
    /// The shell the installed commands run under — this machine's, unless
    /// a test pins the other one to see its spelling.
    pub host: ScriptHost,
}

impl InstallPaths {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            host: ScriptHost::current(),
            grok_home: None,
            copilot_home: None,
            kimi_home: None,
            claude_extra_configs: Vec::new(),
        }
    }

    /// The vendor-env-aware resolution the real app uses.
    pub fn from_environment(home: impl Into<PathBuf>) -> Self {
        let read = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            home: home.into(),
            grok_home: read("GROK_HOME"),
            copilot_home: read("COPILOT_HOME"),
            kimi_home: read("KIMI_CODE_HOME"),
            claude_extra_configs: Vec::new(),
            host: ScriptHost::current(),
        }
    }

    /// The same paths, carrying the extra Claude config files too.
    /// Pin the host — for tests that want the other spelling, and for a
    /// caller that knows a vendor runs its hooks through the other shell.
    #[must_use]
    pub fn with_host(mut self, host: ScriptHost) -> Self {
        self.host = host;
        self
    }

    pub fn with_claude_extras(mut self, extras: Vec<PathBuf>) -> Self {
        self.claude_extra_configs = extras;
        self
    }

    /// Every settings file Claude's hook must stand in: the home file and one
    /// per account directory. Other agents have exactly one.
    fn config_paths(&self, agent: AgentKind) -> Vec<PathBuf> {
        let mut all = vec![self.config_path(agent)];
        if agent == AgentKind::Claude {
            all.extend(self.claude_extra_configs.iter().cloned());
        }
        all
    }

    fn grok_home(&self) -> PathBuf {
        self.grok_home
            .clone()
            .unwrap_or_else(|| self.home.join(".grok"))
    }

    fn copilot_home(&self) -> PathBuf {
        self.copilot_home
            .clone()
            .unwrap_or_else(|| self.home.join(".copilot"))
    }

    fn kimi_home(&self) -> PathBuf {
        self.kimi_home
            .clone()
            .unwrap_or_else(|| self.home.join(".kimi-code"))
    }

    /// Which file an agent's hooks live in.
    fn config_path(&self, agent: AgentKind) -> PathBuf {
        match agent {
            AgentKind::Claude => self.home.join(".claude").join("settings.json"),
            AgentKind::Cursor => self.home.join(".cursor").join("hooks.json"),
            AgentKind::Droid => self.home.join(".factory").join("settings.json"),
            AgentKind::CommandCode => self.home.join(".commandcode").join("settings.json"),
            AgentKind::Grok => self.grok_home().join("hooks").join("zerocode-status.json"),
            AgentKind::Copilot => self.copilot_home().join("hooks").join("zerocode.json"),
            AgentKind::Devin => self.home.join(".config").join("devin").join("config.json"),
            AgentKind::Antigravity => self.home.join(".gemini").join("config").join("hooks.json"),
            AgentKind::Kimi => self.kimi_home().join("config.toml"),
            // Not managed here; the caller filters on MANAGED_TARGETS.
            _ => self.home.join(".zerocode").join("unmanaged"),
        }
    }
}

fn status(
    agent: AgentKind,
    state: HookInstallState,
    config_path: &Path,
    detail: Option<String>,
) -> HookStatus {
    HookStatus {
        agent,
        state,
        config_path: config_path.to_string_lossy().into_owned(),
        detail,
        note: None,
    }
}

fn unreadable(agent: AgentKind, config_path: &Path) -> HookStatus {
    status(
        agent,
        HookInstallState::Error,
        config_path,
        Some("설정 파일을 읽을 수 없어 아무것도 쓰지 않았습니다".to_string()),
    )
}

fn verdict_of(present: usize, total: usize) -> HookInstallState {
    if present == total && total > 0 {
        HookInstallState::Installed
    } else if present == 0 {
        HookInstallState::NotInstalled
    } else {
        HookInstallState::Partial
    }
}

/// The nested-family table: which events, which timeout.
fn nested_events(agent: AgentKind) -> Option<(&'static [NestedEvent], u64)> {
    match agent {
        AgentKind::Claude => Some((CLAUDE_EVENTS, TIMEOUT_SECONDS)),
        AgentKind::Droid => Some((DROID_EVENTS, TIMEOUT_SECONDS)),
        AgentKind::CommandCode => Some((COMMAND_CODE_EVENTS, TIMEOUT_SECONDS)),
        AgentKind::Grok => Some((GROK_EVENTS, TIMEOUT_SECONDS)),
        AgentKind::Devin => Some((DEVIN_EVENTS, TIMEOUT_SECONDS)),
        _ => None,
    }
}

/// Install one agent's hook: write the script, merge the entry into the
/// agent's own config, and answer with what the config now says.
pub fn install_agent(paths: &InstallPaths, agent: AgentKind) -> HookStatus {
    let config_path = paths.config_path(agent);
    let host = paths.host.for_vendor(agent);
    if write_script_for(&paths.home, agent, host).is_err() {
        return status(
            agent,
            HookInstallState::Error,
            &config_path,
            Some("훅 스크립트를 쓰지 못했습니다".to_string()),
        );
    }
    let script = script_path_for(&paths.home, agent, host);

    if let Some((events, timeout)) = nested_events(agent) {
        // Every file this agent reads hooks from — for Claude that is the
        // home file AND one per account directory, because a launched claude
        // reads only the `CLAUDE_CONFIG_DIR` it was handed.
        for file in paths.config_paths(agent) {
            let Some(mut config) = read_config(&file) else {
                return unreadable(agent, &file);
            };
            install_nested(&mut config, agent, events, &script, host, timeout);
            if write_config(&file, &config).is_err() {
                return unreadable(agent, &file);
            }
        }
        return status_of(paths, agent);
    }

    let command = wrapper(host, &script, &[]);

    match agent {
        AgentKind::Cursor => install_cursor(paths, &config_path, &command),
        AgentKind::Copilot => install_copilot(paths, &config_path),
        AgentKind::Antigravity => install_antigravity(paths, &config_path, host),
        AgentKind::Kimi => install_kimi(paths, &config_path, &command),
        _ => status(
            agent,
            HookInstallState::Error,
            &config_path,
            Some("이 에이전트의 훅 설치는 이 창이 아직 모릅니다".to_string()),
        ),
    }
}

/// Take our entries back out, leaving everything else byte-for-byte in the
/// same JSON (modulo formatting, which the merge already normalized).
pub fn remove_agent(paths: &InstallPaths, agent: AgentKind) -> HookStatus {
    let config_path = paths.config_path(agent);
    if agent == AgentKind::Kimi {
        return remove_kimi(paths, &config_path);
    }
    // Taken out of every file it was put into — the account files too, or
    // "off" leaves a hook running in each launched claude.
    for file in paths.config_paths(agent) {
        let Some(mut config) = read_config(&file) else {
            // A file that never existed has nothing of ours in it.
            if !file.exists() {
                continue;
            }
            return unreadable(agent, &file);
        };
        if agent == AgentKind::Antigravity {
            remove_antigravity_bundle(&mut config, agent);
        } else if let Some(hooks) = config.get("hooks").and_then(Value::as_object) {
            let mut hooks = hooks.clone();
            clean_hooks_object(&mut hooks, agent);
            config.insert("hooks".to_string(), Value::Object(hooks));
        }
        if file.exists() && write_config(&file, &config).is_err() {
            return unreadable(agent, &file);
        }
    }
    status_of(paths, agent)
}

/// What the agent's config says right now, without touching it.
pub fn status_of(paths: &InstallPaths, agent: AgentKind) -> HookStatus {
    let config_path = paths.config_path(agent);
    if agent == AgentKind::Kimi {
        return kimi_status(paths, &config_path);
    }
    // Summed across every file the agent may read — one per account for
    // Claude, one file for everyone else. "Installed" means every file
    // carries every event: a hook standing in `~/.claude` but missing from
    // an account file is exactly the half-truth this report exists to
    // catch, and it reads as Partial.
    let mut present = 0;
    let mut total = 0;
    for file in paths.config_paths(agent) {
        let Some(config) = read_config(&file) else {
            return unreadable(agent, &file);
        };
        let (here, of) = match agent {
            AgentKind::Cursor => (
                nested_or_flat_present(&config, agent, CURSOR_EVENTS),
                CURSOR_EVENTS.len(),
            ),
            AgentKind::Copilot => (
                nested_or_flat_present(&config, agent, COPILOT_EVENTS),
                COPILOT_EVENTS.len(),
            ),
            AgentKind::Antigravity => (
                antigravity_present(&config, agent),
                ANTIGRAVITY_EVENTS.len(),
            ),
            _ => match nested_events(agent) {
                Some((events, _)) => (nested_present_count(&config, agent, events), events.len()),
                None => (0, 0),
            },
        };
        present += here;
        total += of;
    }
    status(agent, verdict_of(present, total), &config_path, None)
}

// ------------------------------------------------------------------- cursor

/// Cursor's eight, measured (`CURSOR_EVENTS`, chunk :5515-5526). Its file is
/// its own `hooks.json` with a `version` field and FLAT definitions —
/// `{command, timeout}`, no `type`, no nesting.
const CURSOR_EVENTS: &[&str] = &[
    "beforeSubmitPrompt",
    "stop",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "beforeShellExecution",
    "beforeMCPExecution",
    "afterAgentResponse",
];

fn flat_present_in(hooks: &Map<String, Value>, agent: AgentKind, events: &[&str]) -> usize {
    events
        .iter()
        .filter(|event| {
            hooks
                .get(**event)
                .and_then(Value::as_array)
                .is_some_and(|definitions| {
                    definitions
                        .iter()
                        .any(|definition| definition_is_managed(definition, agent))
                })
        })
        .count()
}

fn nested_or_flat_present(config: &Map<String, Value>, agent: AgentKind, events: &[&str]) -> usize {
    config
        .get("hooks")
        .and_then(Value::as_object)
        .map_or(0, |hooks| flat_present_in(hooks, agent, events))
}

fn install_cursor(paths: &InstallPaths, config_path: &Path, command: &str) -> HookStatus {
    let agent = AgentKind::Cursor;
    let Some(mut config) = read_config(config_path) else {
        return unreadable(agent, config_path);
    };
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    clean_hooks_object(&mut hooks, agent);
    for event in CURSOR_EVENTS {
        let mut definitions = hooks
            .get(*event)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        definitions.push(json!({"command": command, "timeout": TIMEOUT_SECONDS}));
        hooks.insert((*event).to_string(), Value::Array(definitions));
    }
    config.insert("hooks".to_string(), Value::Object(hooks));
    // Cursor refuses a hooks file with no version; an existing one keeps its
    // own.
    if !config.contains_key("version") {
        config.insert("version".to_string(), json!(1));
    }
    if write_config(config_path, &config).is_err() {
        return unreadable(agent, config_path);
    }
    status_of(paths, agent)
}

// ------------------------------------------------------------------ copilot

/// Copilot's thirteen, measured (`COPILOT_EVENTS`, chunk :5990-6020) —
/// including the one spelled `subagentStart` in lowercase, which is the
/// vendor's own file and not ours to tidy.
const COPILOT_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "subagentStart",
    "SubagentStop",
    "PreCompact",
    "Stop",
    "ErrorOccurred",
    "PermissionRequest",
    "Notification",
];

/// Copilot's file is entirely ours (`hooks/zerocode.json` in its config
/// directory — it reads every file in `hooks/`), and its definitions name the
/// interpreter: `{type: "command", bash: <command>, timeoutSec: 5}`. One
/// command per event, each carrying the event's name in the environment,
/// because Copilot's payload does not say which event fired.
fn install_copilot(paths: &InstallPaths, config_path: &Path) -> HookStatus {
    let agent = AgentKind::Copilot;
    let Some(mut config) = read_config(config_path) else {
        return unreadable(agent, config_path);
    };
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    clean_hooks_object(&mut hooks, agent);
    // Copilot names its interpreter (`bash`): the POSIX script and the POSIX
    // guard, whatever the host.
    let script = script_path_for(&paths.home, agent, ScriptHost::Posix);
    for event in COPILOT_EVENTS {
        let command = posix_wrapper(&script, &[(crate::env_var::COPILOT_EVENT, event)]);
        let mut definitions = hooks
            .get(*event)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        definitions.push(json!({
            "type": "command",
            "bash": command,
            "timeoutSec": COPILOT_TIMEOUT_SECONDS,
        }));
        hooks.insert((*event).to_string(), Value::Array(definitions));
    }
    config.insert("hooks".to_string(), Value::Object(hooks));
    if write_config(config_path, &config).is_err() {
        return unreadable(agent, config_path);
    }
    status_of(paths, agent)
}

// -------------------------------------------------------------- antigravity

/// Status events only: agy 1.2.11 requires a permission decision from
/// PreToolUse, so an observing hook's empty reply would deny every tool.
/// PreInvocation still marks work and PostToolUse reports completed tools;
/// the window no longer receives a per-tool start before the tool finishes.
const ANTIGRAVITY_EVENTS: &[(&str, bool)] = &[
    ("PreInvocation", false),
    ("PostInvocation", false),
    ("Stop", false),
    ("PostToolUse", true),
];

/// Our entries live under ONE bundle key in `~/.gemini/config/hooks.json` —
/// `zerocode-status` — rather than merged into shared event arrays. Both
/// removal and reinstall preserve any user definitions added under our key.
const ANTIGRAVITY_BUNDLE: &str = "zerocode-status";

fn install_antigravity(paths: &InstallPaths, config_path: &Path, host: ScriptHost) -> HookStatus {
    let agent = AgentKind::Antigravity;
    let Some(mut config) = read_config(config_path) else {
        return unreadable(agent, config_path);
    };
    let script = script_path_for(&paths.home, agent, host);
    // Clean every old managed event, including ones we no longer install.
    remove_antigravity_bundle(&mut config, agent);
    let mut bundle = config
        .get(ANTIGRAVITY_BUNDLE)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (event, is_tool) in ANTIGRAVITY_EVENTS {
        let command = wrapper(host, &script, &[(crate::env_var::ANTIGRAVITY_EVENT, event)]);
        let definition = if *is_tool {
            nested_definition(&command, Some("*"), TIMEOUT_SECONDS)
        } else {
            json!({"type": "command", "command": command, "timeout": TIMEOUT_SECONDS})
        };
        let mut definitions = bundle
            .get(*event)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        definitions.push(definition);
        bundle.insert((*event).to_string(), Value::Array(definitions));
    }
    config.insert(ANTIGRAVITY_BUNDLE.to_string(), Value::Object(bundle));
    if write_config(config_path, &config).is_err() {
        return unreadable(agent, config_path);
    }
    status_of(paths, agent)
}

fn remove_antigravity_bundle(config: &mut Map<String, Value>, agent: AgentKind) {
    let Some(bundle) = config.get(ANTIGRAVITY_BUNDLE).and_then(Value::as_object) else {
        return;
    };
    let mut bundle = bundle.clone();
    clean_hooks_object(&mut bundle, agent);
    if bundle.is_empty() {
        config.remove(ANTIGRAVITY_BUNDLE);
    } else {
        // Somebody wrote their own entries under our key; they stay.
        config.insert(ANTIGRAVITY_BUNDLE.to_string(), Value::Object(bundle));
    }
}

fn antigravity_present(config: &Map<String, Value>, agent: AgentKind) -> usize {
    let Some(bundle) = config.get(ANTIGRAVITY_BUNDLE).and_then(Value::as_object) else {
        return 0;
    };
    ANTIGRAVITY_EVENTS
        .iter()
        .filter(|(event, _)| {
            bundle
                .get(*event)
                .and_then(Value::as_array)
                .is_some_and(|definitions| {
                    definitions
                        .iter()
                        .any(|definition| definition_is_managed(definition, agent))
                })
        })
        .count()
}

// --------------------------------------------------------------------- kimi

/// Kimi's seven, measured (`KIMI_HOOK_EVENTS`, chunk :7553-7561).
const KIMI_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Stop",
    "StopFailure",
];

/// Kimi's config is TOML, and this module does not parse TOML — it does not
/// need to. Our entries live between two marker comments, appended at the end
/// and replaced as a block, so the user's own text above is never interpreted,
/// only preserved. The one thing we escape is our own command string.
const KIMI_BLOCK_START: &str =
    "# >>> zerocode-managed-kimi-hooks (managed by ZeroCode; do not edit) >>>";
const KIMI_BLOCK_END: &str = "# <<< zerocode-managed-kimi-hooks <<<";

fn toml_basic_string(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

fn kimi_block(command: &str) -> String {
    let command = toml_basic_string(command);
    let mut lines = vec![KIMI_BLOCK_START.to_string()];
    for event in KIMI_EVENTS {
        lines.push("[[hooks]]".to_string());
        lines.push(format!("event = \"{event}\""));
        lines.push(format!("command = {command}"));
        lines.push(format!("timeout = {TIMEOUT_SECONDS}"));
    }
    lines.push(KIMI_BLOCK_END.to_string());
    lines.join("\n")
}

/// The text without our block. `None` when there was no block to take out.
fn kimi_without_block(text: &str) -> Option<String> {
    let start = text.find(KIMI_BLOCK_START)?;
    let after_start = &text[start..];
    let end = after_start
        .find(KIMI_BLOCK_END)
        .map(|at| start + at + KIMI_BLOCK_END.len())
        .unwrap_or(text.len());
    let mut kept = String::new();
    kept.push_str(text[..start].trim_end());
    let tail = text[end..].trim_start_matches(['\r', '\n']);
    if !tail.trim().is_empty() {
        kept.push('\n');
        kept.push_str(tail);
    }
    Some(kept)
}

fn install_kimi(paths: &InstallPaths, config_path: &Path, command: &str) -> HookStatus {
    let agent = AgentKind::Kimi;
    let text = if config_path.exists() {
        match std::fs::read_to_string(config_path) {
            Ok(text) => text,
            Err(_) => return unreadable(agent, config_path),
        }
    } else {
        String::new()
    };
    let stripped = kimi_without_block(&text).unwrap_or(text);
    let stripped = stripped.trim_end();
    let block = kimi_block(command);
    let next = if stripped.is_empty() {
        format!("{block}\n")
    } else {
        format!("{stripped}\n\n{block}\n")
    };
    if write_kimi(config_path, &next).is_err() {
        return unreadable(agent, config_path);
    }
    status_of(paths, agent)
}

/// Atomic, and with a `.bak` of what was there — measured off Orca's own kimi
/// writer, and warranted here more than anywhere: this file is the user's
/// whole kimi configuration and we are editing it as TEXT.
fn write_kimi(config_path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if config_path.exists() {
        let _ = std::fs::copy(config_path, config_path.with_extension("toml.bak"));
    }
    let tmp = config_path.with_extension("toml.zerocode-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, config_path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn remove_kimi(paths: &InstallPaths, config_path: &Path) -> HookStatus {
    let agent = AgentKind::Kimi;
    if !config_path.exists() {
        return status(agent, HookInstallState::NotInstalled, config_path, None);
    }
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return unreadable(agent, config_path);
    };
    if let Some(stripped) = kimi_without_block(&text) {
        let next = if stripped.trim().is_empty() {
            String::new()
        } else {
            format!("{}\n", stripped.trim_end())
        };
        if write_kimi(config_path, &next).is_err() {
            return unreadable(agent, config_path);
        }
    }
    status_of(paths, agent)
}

fn kimi_status(_paths: &InstallPaths, config_path: &Path) -> HookStatus {
    let agent = AgentKind::Kimi;
    if !config_path.exists() {
        return status(agent, HookInstallState::NotInstalled, config_path, None);
    }
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return unreadable(agent, config_path);
    };
    let Some(start) = text.find(KIMI_BLOCK_START) else {
        return status(agent, HookInstallState::NotInstalled, config_path, None);
    };
    let block = &text[start..];
    let present = KIMI_EVENTS
        .iter()
        .filter(|event| block.contains(&format!("event = \"{event}\"")))
        .count();
    status(
        agent,
        verdict_of(present, KIMI_EVENTS.len()),
        config_path,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The POSIX host, pinned: these tests read the `/bin/sh` spelling on
    /// every platform; the Windows spelling has its own test below.
    fn paths() -> (tempfile::TempDir, InstallPaths) {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = InstallPaths::new(home.path()).with_host(ScriptHost::Posix);
        (home, paths)
    }

    /// The Windows host: a `.cmd` script in the batch dialect, a cmd-spelled
    /// guard around it, the event in a `set`, no PowerShell anywhere — and
    /// the same entries recognized as ours (removal, status) through the
    /// backslash-spelled path.
    #[test]
    fn a_windows_install_writes_a_batch_hook_and_a_cmd_guarded_command() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = InstallPaths::new(home.path()).with_host(ScriptHost::Cmd);
        let outcome = install_agent(&paths, AgentKind::Claude);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");
        let config = read_json(&paths.config_path(AgentKind::Claude));
        let command = config["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command")
            .to_string();
        assert!(
            command.starts_with("cmd.exe /d /c \"if exist \""),
            "{command}"
        );
        assert!(
            command.contains("claude-hook.cmd\") else (more >nul)\""),
            "{command}"
        );
        assert!(
            command.contains("set \"ZEROCODE_HOOK_EVENT=Stop\" && call \""),
            "{command}"
        );
        assert!(
            !command.to_ascii_lowercase().contains("powershell"),
            "{command}"
        );
        assert!(
            is_managed_command(&command, AgentKind::Claude),
            "not recognized as ours: {command}"
        );
        assert!(is_managed_command(
            r#"cmd.exe /d /c "if exist "C:\Users\x\.zerocode\agent-hooks\claude-hook.cmd" (call "C:\Users\x\.zerocode\agent-hooks\claude-hook.cmd") else (more >nul)""#,
            AgentKind::Claude
        ));
        assert!(!is_managed_command(
            r#"cmd.exe /d /c "C:\Users\x\.orca\agent-hooks\claude-hook.cmd""#,
            AgentKind::Claude
        ));
        let script = std::fs::read_to_string(script_path_for(
            home.path(),
            AgentKind::Claude,
            ScriptHost::Cmd,
        ))
        .expect("script");
        assert!(script.starts_with("@echo off\r\n"), "{script}");
        assert!(script.contains("curl.exe"), "{script}");
        assert!(
            !home
                .path()
                .join(".zerocode/agent-hooks/claude-hook.sh")
                .exists()
        );
        // Status reads the same entries back, and removal takes them out.
        assert_eq!(
            status_of(&paths, AgentKind::Claude).state,
            HookInstallState::Installed
        );
        assert_eq!(
            remove_agent(&paths, AgentKind::Claude).state,
            HookInstallState::NotInstalled
        );
        // Copilot names bash: POSIX on this host too.
        let outcome = install_agent(&paths, AgentKind::Copilot);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");
        let config = read_json(&paths.config_path(AgentKind::Copilot));
        let bash = config["hooks"]["Stop"][0]["bash"].as_str().expect("bash");
        assert!(bash.starts_with("if [ -f "), "{bash}");
        assert!(bash.contains("copilot-hook.sh"), "{bash}");
        // Antigravity: the batch dialect, the event set for cmd.
        let outcome = install_agent(&paths, AgentKind::Antigravity);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");
        let config = read_json(&paths.config_path(AgentKind::Antigravity));
        let stop = config[ANTIGRAVITY_BUNDLE]["Stop"][0]["command"]
            .as_str()
            .unwrap();
        assert!(
            stop.contains("set \"ZEROCODE_ANTIGRAVITY_EVENT=Stop\" && call"),
            "{stop}"
        );
        assert!(stop.contains("antigravity-hook.cmd"), "{stop}");
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
    }

    #[test]
    fn a_fresh_claude_install_carries_all_eleven_events_and_the_guarded_command() {
        let (home, paths) = paths();
        let outcome = install_agent(&paths, AgentKind::Claude);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");

        let config = read_json(&paths.config_path(AgentKind::Claude));
        let hooks = config["hooks"].as_object().expect("hooks object");
        assert_eq!(hooks.len(), 11, "not the eleven measured events: {hooks:?}");
        // The tool events carry the matcher Claude requires; the others none.
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "*");
        assert!(hooks["Stop"][0].get("matcher").is_none());
        let command = hooks["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        // The guard and the drain: a machine this app left keeps a harmless
        // entry, not a hanging one.
        assert!(command.starts_with("if [ -f "), "{command}");
        assert!(command.contains("command -p cat"), "{command}");
        assert!(command.contains(".zerocode/agent-hooks/claude-hook.sh"));
        assert!(
            command.contains("ZEROCODE_HOOK_EVENT='Stop'"),
            "the shared script cannot distinguish nested events: {command}"
        );
        assert_eq!(hooks["Stop"][0]["hooks"][0]["timeout"], 10);

        // And the script itself is there, executable, pointing at our slug.
        let script =
            std::fs::read_to_string(script_path(home.path(), AgentKind::Claude)).expect("script");
        assert!(script.contains("/hook/claude"));
        assert!(
            script.contains("DEVIN_PROJECT_DIR"),
            "the devin guard is gone"
        );
        // A backgrounded worker inherits a pane key it does not run in; the
        // guard is worthless if it runs after the post it exists to prevent.
        let job_guard = script
            .find("CLAUDE_JOB_DIR")
            .expect("the background-worker guard is gone");
        assert!(
            job_guard < script.find("curl").expect("the bridge post is gone"),
            "the background-worker guard stands after the post"
        );
    }

    /// The account-picker bug, replayed: a launched claude reads hooks only
    /// from the `CLAUDE_CONFIG_DIR` it was handed, so the hook must stand in
    /// every account file too — and the report must refuse to say
    /// "installed" while any account file lacks it. This is how a listening
    /// bridge still drew no agent rows ("실행중인 agent claude 표시가
    /// 안나와"): ~/.claude carried the hook, the launched account did not.
    #[test]
    fn claude_hooks_reach_every_account_file_and_a_gap_reads_as_partial() {
        let (home, paths) = paths();
        let one = home.path().join("accounts/a1/settings.json");
        let two = home.path().join("accounts/a2/settings.json");
        let paths = paths.with_claude_extras(vec![one.clone(), two.clone()]);

        let outcome = install_agent(&paths, AgentKind::Claude);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");
        for file in [&one, &two] {
            let config = read_json(file);
            assert_eq!(
                config["hooks"].as_object().expect("hooks").len(),
                11,
                "an account file is missing events: {file:?}"
            );
        }

        // An account added after the install has nothing yet — the report
        // says Partial, never Installed, and the next reconcile heals it.
        let three = home.path().join("accounts/a3/settings.json");
        let paths = paths.with_claude_extras(vec![one.clone(), two.clone(), three.clone()]);
        let stale = status_of(&paths, AgentKind::Claude);
        assert_eq!(stale.state, HookInstallState::Partial, "{stale:?}");
        let healed = install_agent(&paths, AgentKind::Claude);
        assert_eq!(healed.state, HookInstallState::Installed, "{healed:?}");

        // And off means out of EVERY file.
        let removed = remove_agent(&paths, AgentKind::Claude);
        assert_eq!(removed.state, HookInstallState::NotInstalled, "{removed:?}");
        for file in [&one, &two, &three] {
            let hooks = read_json(file);
            assert_eq!(
                hooks["hooks"].as_object().map(Map::len).unwrap_or(0),
                0,
                "a removed hook lingers in an account file: {file:?}"
            );
        }
    }

    /// The reason this module exists at all: a user's own hooks survive an
    /// install AND a remove, byte-for-byte in meaning.
    #[test]
    fn a_users_own_hooks_survive_install_and_remove() {
        let (_home, paths) = paths();
        let config_path = paths.config_path(AgentKind::Claude);
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &config_path,
            r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done","timeout":5}]}]}}"#,
        )
        .expect("seed");

        install_agent(&paths, AgentKind::Claude);
        let installed = read_json(&config_path);
        assert_eq!(installed["model"], "opus", "an unrelated setting was lost");
        let stop = installed["hooks"]["Stop"].as_array().expect("stop");
        assert_eq!(stop.len(), 2, "the user's Stop hook was replaced: {stop:?}");
        assert_eq!(
            stop[0]["hooks"][0]["command"], "say done",
            "the user's hook no longer fires first"
        );

        let removed = remove_agent(&paths, AgentKind::Claude);
        assert_eq!(removed.state, HookInstallState::NotInstalled);
        let after = read_json(&config_path);
        let stop = after["hooks"]["Stop"].as_array().expect("stop");
        assert_eq!(stop.len(), 1, "remove took the user's hook too: {stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
    }

    /// Another product's managed entries are not ours to clean — the needle is
    /// directory-qualified for exactly this file.
    #[test]
    fn another_products_agent_hooks_entries_are_left_alone() {
        let (_home, paths) = paths();
        let config_path = paths.config_path(AgentKind::Claude);
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &config_path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"if [ -f '/Users/x/.orca/agent-hooks/claude-hook.sh' ]; then /bin/sh '/Users/x/.orca/agent-hooks/claude-hook.sh'; fi","timeout":10}]}]}}"#,
        )
        .expect("seed");
        install_agent(&paths, AgentKind::Claude);
        remove_agent(&paths, AgentKind::Claude);
        let after = read_json(&config_path);
        let stop = after["hooks"]["Stop"].as_array().expect("stop");
        assert_eq!(
            stop.len(),
            1,
            "a foreign product's entry was swept: {stop:?}"
        );
        assert!(
            stop[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(".orca/"),
        );
    }

    /// A config that cannot be parsed is never written — the status says so
    /// and the bytes stay.
    #[test]
    fn an_unreadable_config_is_reported_not_rewritten() {
        let (_home, paths) = paths();
        let config_path = paths.config_path(AgentKind::Claude);
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("mkdir");
        std::fs::write(&config_path, "{ not json").expect("seed");
        let outcome = install_agent(&paths, AgentKind::Claude);
        assert_eq!(outcome.state, HookInstallState::Error);
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("read"),
            "{ not json",
            "an unparseable config was overwritten"
        );
    }

    #[test]
    fn cursor_writes_flat_versioned_entries() {
        let (_home, paths) = paths();
        install_agent(&paths, AgentKind::Cursor);
        let cursor = read_json(&paths.config_path(AgentKind::Cursor));
        assert_eq!(cursor["version"], 1);
        let stop = &cursor["hooks"]["stop"][0];
        assert_eq!(stop["timeout"], 10);
        assert!(stop.get("hooks").is_none(), "cursor entries are flat");
        assert!(stop["command"].as_str().unwrap().contains("cursor-hook.sh"));
    }

    #[test]
    fn reinstall_replaces_an_old_script_contract_in_place() {
        let (home, paths) = paths();
        let script = script_path(home.path(), AgentKind::Claude);
        std::fs::create_dir_all(script.parent().expect("script parent")).expect("mkdir");
        std::fs::write(&script, "#!/bin/sh\n# contract version 1\n").expect("old script");

        let outcome = install_agent(&paths, AgentKind::Claude);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");
        let installed = std::fs::read_to_string(script).expect("new script");
        assert!(installed.contains(&format!("version={}", crate::HOOK_CONTRACT_VERSION)));
        assert!(!installed.contains("contract version 1"));
    }

    #[test]
    fn antigravity_reports_work_and_completion_without_permission_hooks() {
        let (_home, paths) = paths();
        let outcome = install_agent(&paths, AgentKind::Antigravity);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");

        let config = read_json(&paths.config_path(AgentKind::Antigravity));
        let config = config.as_object().expect("config object");
        let bundle = config[ANTIGRAVITY_BUNDLE].as_object().expect("bundle");
        assert!(!bundle.contains_key("PreToolUse"));
        assert_eq!(bundle.len(), ANTIGRAVITY_EVENTS.len());
        assert_eq!(
            antigravity_present(config, AgentKind::Antigravity),
            ANTIGRAVITY_EVENTS.len()
        );
        for (event, expected) in [
            ("PreInvocation", zerocode_core::hook::HookState::Working),
            ("PostToolUse", zerocode_core::hook::HookState::Working),
            ("PostInvocation", zerocode_core::hook::HookState::Done),
            ("Stop", zerocode_core::hook::HookState::Done),
        ] {
            assert!(bundle.contains_key(event), "missing {event}");
            assert_eq!(zerocode_core::hook::hook_state(event, "{}"), Some(expected));
        }
        let post_tool_use = &bundle["PostToolUse"][0];
        assert_eq!(post_tool_use["matcher"], "*");
        assert_eq!(post_tool_use["hooks"][0]["type"], "command");
        assert!(post_tool_use.get("command").is_none());
    }

    #[test]
    fn antigravity_reinstall_removes_legacy_permission_hooks_and_keeps_user_entries() {
        for host in [ScriptHost::Posix, ScriptHost::Cmd] {
            let home = tempfile::tempdir().expect("tempdir");
            let paths = InstallPaths::new(home.path()).with_host(host);
            let agent = AgentKind::Antigravity;
            install_agent(&paths, agent);
            let config_path = paths.config_path(agent);
            let mut config = read_json(&config_path);
            let command = wrapper(
                host,
                &script_path_for(home.path(), agent, host),
                &[(crate::env_var::ANTIGRAVITY_EVENT, "PreToolUse")],
            );
            let legacy = json!([nested_definition(&command, Some("*"), TIMEOUT_SECONDS)]);
            config[ANTIGRAVITY_BUNDLE]["PreToolUse"] = legacy.clone();
            std::fs::write(&config_path, config.to_string()).expect("legacy config");
            assert_eq!(
                install_agent(&paths, agent).state,
                HookInstallState::Installed
            );
            assert!(
                read_json(&config_path)[ANTIGRAVITY_BUNDLE]
                    .get("PreToolUse")
                    .is_none()
            );
            let user_tool = nested_definition("user-tool-hook", Some("*"), TIMEOUT_SECONDS);
            let user_stop = json!({"type": "command", "command": "user-stop-hook"});
            let user_bundle = json!({"PreToolUse": [user_tool.clone()]});
            config["user-bundle"] = user_bundle.clone();
            let bundle = config[ANTIGRAVITY_BUNDLE].as_object_mut().expect("bundle");
            bundle.insert("PreToolUse".to_string(), legacy);
            bundle["PreToolUse"]
                .as_array_mut()
                .unwrap()
                .push(user_tool.clone());
            bundle["PostToolUse"]
                .as_array_mut()
                .unwrap()
                .push(user_tool.clone());
            bundle["Stop"]
                .as_array_mut()
                .unwrap()
                .push(user_stop.clone());
            std::fs::write(&config_path, config.to_string()).expect("legacy config");

            let outcome = install_agent(&paths, agent);
            assert_eq!(outcome.state, HookInstallState::Installed, "{host:?}");
            let after = read_json(&config_path);
            assert_eq!(after["user-bundle"], user_bundle);
            assert_eq!(
                after[ANTIGRAVITY_BUNDLE]["PreToolUse"],
                json!([user_tool.clone()])
            );
            assert_eq!(after[ANTIGRAVITY_BUNDLE]["PostToolUse"][0], user_tool);
            assert_eq!(after[ANTIGRAVITY_BUNDLE]["Stop"][0], user_stop);
            install_agent(&paths, agent);
            assert_eq!(read_json(&config_path), after, "reinstall stacked hooks");

            let mut partial = after.clone();
            partial[ANTIGRAVITY_BUNDLE]
                .as_object_mut()
                .unwrap()
                .remove("PostToolUse");
            std::fs::write(&config_path, partial.to_string()).expect("partial config");
            assert_eq!(status_of(&paths, agent).state, HookInstallState::Partial);

            assert_eq!(
                remove_agent(&paths, agent).state,
                HookInstallState::NotInstalled
            );
            let removed = read_json(&config_path);
            assert_eq!(removed["user-bundle"], user_bundle);
            assert_eq!(
                removed[ANTIGRAVITY_BUNDLE]["PreToolUse"],
                json!([user_tool])
            );
            assert_eq!(removed[ANTIGRAVITY_BUNDLE]["Stop"], json!([user_stop]));
        }
    }

    #[test]
    fn copilot_names_each_event_in_the_command_and_antigravity_keeps_to_its_bundle() {
        let (_home, paths) = paths();
        install_agent(&paths, AgentKind::Copilot);
        let copilot = read_json(&paths.config_path(AgentKind::Copilot));
        let stop = &copilot["hooks"]["Stop"][0];
        assert_eq!(stop["timeoutSec"], 5);
        let bash = stop["bash"].as_str().expect("bash command");
        assert!(
            bash.contains(&format!("{}='Stop'", crate::env_var::COPILOT_EVENT)),
            "{bash}"
        );

        // Antigravity: a seeded foreign key survives, ours is one bundle.
        let anti_path = paths.config_path(AgentKind::Antigravity);
        std::fs::create_dir_all(anti_path.parent().unwrap()).expect("mkdir");
        std::fs::write(&anti_path, r#"{"someone-else":{"Stop":[]}}"#).expect("seed");
        let outcome = install_agent(&paths, AgentKind::Antigravity);
        assert_eq!(outcome.state, HookInstallState::Installed);
        let anti = read_json(&anti_path);
        assert!(
            anti.get("someone-else").is_some(),
            "a foreign bundle was dropped"
        );
        assert_eq!(anti["zerocode-status"]["Stop"][0]["type"], "command");
        assert_eq!(anti["zerocode-status"]["PostToolUse"][0]["matcher"], "*");
        remove_agent(&paths, AgentKind::Antigravity);
        let after = read_json(&anti_path);
        assert!(after.get("zerocode-status").is_none());
        assert!(after.get("someone-else").is_some());
    }

    /// Kimi's TOML is edited as text between markers; everything above them —
    /// including TOML this module cannot parse — is carried, not interpreted.
    #[test]
    fn kimi_keeps_the_users_toml_and_replaces_only_its_own_block() {
        let (_home, paths) = paths();
        let config_path = paths.config_path(AgentKind::Kimi);
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &config_path,
            "model = \"k2\"\n\n[[hooks]]\nevent = \"Stop\"\ncommand = \"mine\"\n",
        )
        .expect("seed");

        let outcome = install_agent(&paths, AgentKind::Kimi);
        assert_eq!(outcome.state, HookInstallState::Installed);
        let text = std::fs::read_to_string(&config_path).expect("read");
        assert!(text.starts_with("model = \"k2\""));
        assert!(
            text.contains("command = \"mine\""),
            "the user's hook was lost"
        );
        assert_eq!(text.matches(KIMI_BLOCK_START).count(), 1);
        assert_eq!(text.matches("[[hooks]]").count(), 1 + KIMI_EVENTS.len());

        // A second install replaces the block rather than stacking one.
        install_agent(&paths, AgentKind::Kimi);
        let text = std::fs::read_to_string(&config_path).expect("read");
        assert_eq!(text.matches(KIMI_BLOCK_START).count(), 1);

        remove_agent(&paths, AgentKind::Kimi);
        let text = std::fs::read_to_string(&config_path).expect("read");
        assert!(!text.contains(KIMI_BLOCK_START));
        assert!(text.contains("command = \"mine\""));
        assert!(
            config_path.with_extension("toml.bak").exists(),
            "no backup was kept"
        );
    }

    /// Half-present entries answer `partial`, so the settings screen can say
    /// which agent needs a reinstall rather than lying in either direction.
    #[test]
    fn a_hand_pruned_config_reads_as_partial() {
        let (_home, paths) = paths();
        install_agent(&paths, AgentKind::Droid);
        let config_path = paths.config_path(AgentKind::Droid);
        let mut config: Value = read_json(&config_path);
        config["hooks"]
            .as_object_mut()
            .unwrap()
            .remove("Notification");
        std::fs::write(&config_path, config.to_string()).expect("prune");
        assert_eq!(
            status_of(&paths, AgentKind::Droid).state,
            HookInstallState::Partial
        );
    }
}
