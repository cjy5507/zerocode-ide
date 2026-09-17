//! Agents that take a PLUGIN rather than a settings entry.
//!
//! Some agents have no hook table at all: they load code. Installing there is
//! writing one file into a plugins directory, which is a gentler kind of
//! intrusion than editing somebody's settings — nothing of theirs is touched,
//! and removing ours is deleting one file.
//!
//! Measured off Orca 1.4.169. Six agents take this road; **amp** is
//! implemented here and the other five are deliberately not — see the bottom of
//! this comment, and docs/reverse/orca-ui-inventory.md 1-bb.
//!
//! ## What a plugin has to get right that a script does not
//!
//! 1. **It must not block the agent's turn.** A hook script is a subprocess the
//!    agent waits on with its own timeout; a plugin callback runs INSIDE the
//!    agent's event loop. So the POST is queued and drained, never awaited in
//!    the callback, and the queue is BOUNDED — with the bridge down every post
//!    waits out its timeout, and an unbounded queue would retain every payload
//!    closure of a long session.
//! 2. **`tool.call` must return a decision.** Amp asks its plugins whether a
//!    tool may run. A handler that reports the event and returns nothing has
//!    just declined every tool the agent tried — the hook would stop the work
//!    it exists to watch. `{ action: "allow" }`, measured verbatim.
//! 3. **It re-reads the endpoint file per event.** Same reason the shell scripts
//!    source it: an amp session outlives our restarts.
//!
//! ## The five that are not here, and why
//!
//! Not omissions of convenience — each needs a subsystem this window does not
//! have, and a half version would report WRONG states, which is worse than
//! reporting none:
//!
//! - **opencode** and **mimo-code**: the measured plugin is a stateful program
//!   (~500 lines) whose bulk exists for a feature we do not have — Orca
//!   forwards streamed message previews, and `message.part.updated` re-sends the
//!   full accumulated text after every append, so it carries a throttle, a
//!   coalescing buffer and a character cap. On top of that sits fail-open busy
//!   recovery around OpenCode's concurrent, directory-scoped session factories:
//!   module-level ownership maps deciding which factory's Idle may retire which
//!   Busy. Getting that wrong reports an idle agent as working, forever.
//! - **hermes**: a Python plugin (`plugin.yaml` + `__init__.py`) plus an edit to
//!   the user's `config.yaml` to enable it. Writing YAML back into a file
//!   somebody owns needs a real YAML round-trip, and this repository already
//!   refused to take a YAML dependency for the far safer job of READING one.
//! - **pi** and **omp**: not a plugin file but an overlay directory — the
//!   agent's home is mirrored and the copy is pointed at through the launch
//!   environment. That is the same subsystem Codex's managed `CODEX_HOME` needs.

use std::path::{Path, PathBuf};

use zerocode_core::AgentKind;

use crate::install::{HookInstallState, HookStatus};

/// The marker that says a plugin file is ours. Present in every file we write
/// and the only thing that makes one safe to delete.
pub const PLUGIN_MARKER: &str = "Managed by ZeroCode. Do not edit; changes may be overwritten.";

/// The agents whose hooks are installed as a plugin file.
pub const PLUGIN_TARGETS: [AgentKind; 1] = [AgentKind::Amp];

fn plugin_file(agent: AgentKind) -> &'static str {
    match agent {
        // Amp loads TypeScript directly.
        AgentKind::Amp => "zerocode-agent-status.ts",
        _ => "zerocode-agent-status.js",
    }
}

/// Where the agent looks for plugins.
pub fn plugin_path(home: &Path, agent: AgentKind) -> PathBuf {
    match agent {
        AgentKind::Amp => home
            .join(".config")
            .join("amp")
            .join("plugins")
            .join(plugin_file(agent)),
        _ => home
            .join(".config")
            .join(agent.slug())
            .join("plugins")
            .join(plugin_file(agent)),
    }
}

/// What is at that path: ours, somebody else's, or nothing.
enum PluginState {
    /// Ours, and carrying every handler this version installs.
    Ours,
    /// Ours, but written by an older version — a handler is missing.
    Stale,
    /// Somebody else's file with our name. Left alone.
    Foreign,
    Absent,
}

fn read_state(path: &Path, agent: AgentKind) -> PluginState {
    let Ok(text) = std::fs::read_to_string(path) else {
        return PluginState::Absent;
    };
    if !text.contains(PLUGIN_MARKER) {
        return PluginState::Foreign;
    }
    // Complete means every handler is there AND the decision is still returned.
    // A file that lost the allow-decision is worse than an absent one, so it
    // reads as stale and gets rewritten.
    let complete = required_marks(agent).iter().all(|mark| text.contains(mark));
    if complete {
        PluginState::Ours
    } else {
        PluginState::Stale
    }
}

/// The strings a complete plugin must contain, which is also what "installed"
/// means for it. Named rather than counted, so a handler that goes missing is
/// reported by NAME.
fn required_marks(agent: AgentKind) -> &'static [&'static str] {
    match agent {
        AgentKind::Amp => &[
            "/hook/amp",
            "amp.on('session.start'",
            "amp.on('agent.start'",
            "amp.on('tool.call'",
            "amp.on('tool.result'",
            "amp.on('agent.end'",
            // The one that is not a report but a permission. See the module doc.
            "action: \"allow\"",
        ],
        _ => &[],
    }
}

pub fn install_agent(home: &Path, agent: AgentKind) -> HookStatus {
    let path = plugin_path(home, agent);
    let status = |state: HookInstallState, detail: Option<String>| HookStatus {
        agent,
        state,
        config_path: path.to_string_lossy().into_owned(),
        detail,
        note: None,
    };
    if matches!(read_state(&path, agent), PluginState::Foreign) {
        return status(
            HookInstallState::Error,
            Some("이 이름의 플러그인이 이미 있고 이 창이 쓴 것이 아닙니다".into()),
        );
    }
    let Some(source) = plugin_source(agent) else {
        return status(
            HookInstallState::Error,
            Some("이 에이전트의 플러그인은 이 창이 아직 모릅니다".into()),
        );
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return status(
            HookInstallState::Error,
            Some("플러그인 폴더를 만들지 못했습니다".into()),
        );
    }
    // Through a temp and a rename: the agent may be loading its plugins right
    // now, and half a plugin file is a parse error that takes the agent's whole
    // plugin directory down with it.
    let tmp = path.with_extension("zerocode-tmp");
    if std::fs::write(&tmp, source).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return status(
            HookInstallState::Error,
            Some("플러그인을 쓰지 못했습니다".into()),
        );
    }
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return status(
            HookInstallState::Error,
            Some("플러그인을 제자리에 놓지 못했습니다".into()),
        );
    }
    status_of(home, agent)
}

/// Delete our plugin. A foreign file with our name is never removed — the same
/// rule the settings installers keep, for the same reason.
pub fn remove_agent(home: &Path, agent: AgentKind) -> HookStatus {
    let path = plugin_path(home, agent);
    if matches!(
        read_state(&path, agent),
        PluginState::Ours | PluginState::Stale
    ) {
        let _ = std::fs::remove_file(&path);
    }
    status_of(home, agent)
}

pub fn status_of(home: &Path, agent: AgentKind) -> HookStatus {
    let path = plugin_path(home, agent);
    let (state, detail) = match read_state(&path, agent) {
        PluginState::Ours => (HookInstallState::Installed, None),
        PluginState::Stale => (
            HookInstallState::Partial,
            Some("예전 버전이 쓴 플러그인입니다 — 다시 심으세요".into()),
        ),
        PluginState::Foreign => (
            HookInstallState::Error,
            Some("이 이름의 플러그인이 이 창이 쓴 것이 아닙니다".into()),
        ),
        PluginState::Absent => (HookInstallState::NotInstalled, None),
    };
    HookStatus {
        agent,
        state,
        config_path: path.to_string_lossy().into_owned(),
        detail,
        note: None,
    }
}

/// The plugin's source, or `None` for an agent whose plugin we do not write.
pub fn plugin_source(agent: AgentKind) -> Option<String> {
    match agent {
        AgentKind::Amp => Some(amp_plugin()),
        _ => None,
    }
}

/// Amp's plugin: five handlers, a bounded queue, and one decision.
///
/// TypeScript because that is what amp loads. Written as one string rather than
/// assembled, so what ships is readable as the program it is.
fn amp_plugin() -> String {
    let (port, token, pane_key) = (
        crate::env_var::PORT,
        crate::env_var::TOKEN,
        crate::env_var::PANE_KEY,
    );
    let (tab_id, launch_token, worktree_id) = (
        crate::env_var::TAB_ID,
        crate::env_var::LAUNCH_TOKEN,
        crate::env_var::WORKTREE_ID,
    );
    let (agent_env, version, endpoint) = (
        crate::env_var::AGENT_ENV,
        crate::env_var::VERSION,
        crate::env_var::ENDPOINT,
    );
    let header = crate::HOOK_TOKEN_HEADER;
    format!(
        r#"// {PLUGIN_MARKER}
//
// Reports this Amp session's lifecycle to the local ZeroCode bridge so the IDE
// can show whether the agent is working or waiting for you. Everything here is
// non-blocking and every failure is swallowed: status reporting must never
// affect the Amp run.
import {{ readFileSync }} from "node:fs"

type Coords = {{ port?: string; token?: string; env: string; version: string }}

// The endpoint file is re-read per event rather than cached for the process:
// an Amp session can outlive a ZeroCode restart, and the file is rewritten on
// each start. The env vars were copied once, at launch, and go stale.
function readEndpointFile(): Partial<Coords> | null {{
  const path = process.env.{endpoint}
  if (!path) return null
  try {{
    const out: Record<string, string> = {{}}
    for (const line of readFileSync(path, "utf8").split("\n")) {{
      const at = line.indexOf("=")
      if (at <= 0) continue
      out[line.slice(0, at).trim()] = line.slice(at + 1).trim()
    }}
    return {{
      port: out.{port},
      token: out.{token},
      env: out.{agent_env},
      version: out.{version},
    }}
  }} catch {{
    return null
  }}
}}

function resolveCoords(): Coords {{
  const file = readEndpointFile() ?? {{}}
  return {{
    port: file.port || process.env.{port},
    token: file.token || process.env.{token},
    env: file.env || process.env.{agent_env} || "",
    version: file.version || process.env.{version} || "",
  }}
}}

// Depth- and width-bounded, because a tool's input can be a whole file and this
// travels over loopback on every call.
function jsonSafe(value: unknown, depth = 0): unknown {{
  if (value === null || value === undefined) return value
  const kind = typeof value
  if (kind === "string" || kind === "number" || kind === "boolean") return value
  if (kind === "bigint" || kind === "symbol" || kind === "function") return String(value)
  if (depth >= 4) return preview(value)
  if (Array.isArray(value)) return value.slice(0, 20).map((item) => jsonSafe(item, depth + 1))
  if (kind === "object") {{
    const out: Record<string, unknown> = {{}}
    for (const [key, child] of Object.entries(value as object).slice(0, 20)) {{
      out[key] = jsonSafe(child, depth + 1)
    }}
    return out
  }}
  return String(value)
}}

function preview(value: unknown, max = 4000): string | undefined {{
  if (typeof value === "string") return value.slice(0, max)
  if (value === null || value === undefined) return undefined
  try {{
    return JSON.stringify(value).slice(0, max)
  }} catch {{
    return String(value).slice(0, max)
  }}
}}

async function post(hookEventName: string, payload: Record<string, unknown>): Promise<void> {{
  const coords = resolveCoords()
  const paneKey = process.env.{pane_key}
  // No coordinates means this Amp is not running inside ZeroCode. Nothing to
  // report to, and saying so would be noise in somebody else's terminal.
  if (!coords.port || !coords.token || !paneKey) return
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), 1000)
  try {{
    await fetch(`http://127.0.0.1:${{coords.port}}/hook/amp`, {{
      method: "POST",
      signal: controller.signal,
      headers: {{
        "Content-Type": "application/json",
        "{header}": coords.token,
      }},
      body: JSON.stringify({{
        paneKey,
        launchToken: process.env.{launch_token} || "",
        tabId: process.env.{tab_id} || "",
        worktreeId: process.env.{worktree_id} || "",
        env: coords.env,
        version: coords.version,
        hook_event_name: hookEventName,
        payload: {{ hook_event_name: hookEventName, ...payload }},
      }}),
    }})
  }} catch {{
    // Status reporting must never affect the Amp run.
  }} finally {{
    clearTimeout(timeout)
  }}
}}

// A plugin callback runs inside Amp's own event loop, so nothing here is
// awaited in a handler. The queue is BOUNDED: with the bridge down every post
// waits out its timeout, and an unbounded queue would hold every payload of a
// long session in memory.
const MAX_PENDING = 50
type Queued = {{ hookEventName: string; payload: Record<string, unknown> }}
let queue: Queued[] = []
let draining = false

async function drain(): Promise<void> {{
  if (draining) return
  draining = true
  try {{
    while (queue.length > 0) {{
      const next = queue.shift()
      if (next) await post(next.hookEventName, next.payload)
    }}
  }} finally {{
    draining = false
    if (queue.length > 0) void drain()
  }}
}}

function enqueue(hookEventName: string, payload: Record<string, unknown>): void {{
  if (queue.length >= MAX_PENDING) queue.shift()
  queue.push({{ hookEventName, payload }})
  void drain()
}}

export default function (amp: any) {{
  amp.on('session.start', (event: any) => {{
    enqueue("session.start", {{ threadId: event.thread?.id }})
  }})

  amp.on('agent.start', (event: any) => {{
    enqueue("agent.start", {{
      threadId: event.thread?.id,
      id: event.id,
      message: event.message,
    }})
  }})

  amp.on('tool.call', (event: any) => {{
    enqueue("tool.call", {{
      threadId: event.thread?.id,
      toolUseId: event.toolUseID,
      tool: event.tool,
      input: jsonSafe(event.input),
    }})
    // NOT optional. Amp asks its plugins whether a tool may run, so a handler
    // that only reports and returns nothing declines every tool the agent
    // tried — the hook would stop the work it exists to watch.
    return {{ action: "allow" }}
  }})

  amp.on('tool.result', (event: any) => {{
    enqueue("tool.result", {{
      threadId: event.thread?.id,
      toolUseId: event.toolUseID,
      tool: event.tool,
      status: event.status,
      error: event.error,
      output: preview(event.output),
    }})
  }})

  amp.on('agent.end', (event: any) => {{
    enqueue("agent.end", {{
      threadId: event.thread?.id,
      id: event.id,
      message: event.message,
      status: event.status,
    }})
  }})
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_amp_plugin_carries_every_handler_and_the_allow_decision() {
        let source = plugin_source(AgentKind::Amp).expect("amp has a plugin");
        for mark in required_marks(AgentKind::Amp) {
            assert!(source.contains(mark), "the plugin lost `{mark}`");
        }
        // The one that is a permission rather than a report. Spelled out here
        // as well as in `required_marks`, because the failure it prevents —
        // every tool declined — is invisible in a status list.
        assert!(
            source.contains("return { action: \"allow\" }"),
            "tool.call no longer allows the tool it reported"
        );
        // Nothing is awaited inside a handler: the queue is what keeps Amp's
        // event loop free.
        assert!(source.contains("void drain()") && source.contains("MAX_PENDING = 50"));
        // And it reads the endpoint file, so a session outliving a restart
        // still finds the bridge.
        assert!(source.contains(crate::env_var::ENDPOINT));
    }

    #[test]
    fn installing_writes_the_plugin_where_amp_looks_and_removing_takes_it_back() {
        let home = tempfile::tempdir().expect("tempdir");
        let outcome = install_agent(home.path(), AgentKind::Amp);
        assert_eq!(outcome.state, HookInstallState::Installed, "{outcome:?}");
        let path = plugin_path(home.path(), AgentKind::Amp);
        assert_eq!(
            path,
            home.path()
                .join(".config/amp/plugins/zerocode-agent-status.ts")
        );
        assert!(path.exists());
        // No temp file left beside it.
        assert!(!path.with_extension("zerocode-tmp").exists());

        let gone = remove_agent(home.path(), AgentKind::Amp);
        assert_eq!(gone.state, HookInstallState::NotInstalled);
        assert!(!path.exists());
    }

    /// A file with our name that we did not write is never overwritten and never
    /// deleted — somebody may have written their own, and the name is not proof.
    #[test]
    fn a_foreign_plugin_with_our_name_is_left_exactly_as_it_was() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = plugin_path(home.path(), AgentKind::Amp);
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(&path, "// mine, not yours\n").expect("seed");

        let refused = install_agent(home.path(), AgentKind::Amp);
        assert_eq!(refused.state, HookInstallState::Error);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "// mine, not yours\n",
            "a foreign plugin was overwritten"
        );
        remove_agent(home.path(), AgentKind::Amp);
        assert!(path.exists(), "a foreign plugin was deleted");
    }

    /// A plugin written by an older version reads as partial, not installed —
    /// including the case that matters most, a file that lost the allow
    /// decision and would be declining every tool.
    #[test]
    fn a_plugin_missing_a_handler_reads_as_partial() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = plugin_path(home.path(), AgentKind::Amp);
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        let crippled = plugin_source(AgentKind::Amp)
            .expect("source")
            .replace("return { action: \"allow\" }", "return");
        std::fs::write(&path, crippled).expect("seed");
        assert_eq!(
            status_of(home.path(), AgentKind::Amp).state,
            HookInstallState::Partial
        );
        // And re-installing repairs it.
        assert_eq!(
            install_agent(home.path(), AgentKind::Amp).state,
            HookInstallState::Installed
        );
    }
}
