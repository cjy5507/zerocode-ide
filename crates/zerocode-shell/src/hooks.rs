//! The window's half of the agent hook bridge.
//!
//! `zerocode-hookd` owns the server, the scripts and the installers; what
//! lives here is policy and plumbing: when the bridge starts, where its
//! coordinates are written, which launches carry them, and what happens to an
//! envelope once it arrives.
//!
//! The bridge is bound ONCE, synchronously, before the window exists — a
//! bind on loopback port 0 costs nothing, and doing it late would give the
//! first agent launch a coin-flip on whether its environment carries a port.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use serde::Serialize;
use zerocode_core::AgentKind;
use zerocode_hookd::install::{HookStatus, InstallPaths, MANAGED_TARGETS};
use zerocode_hookd::plugin::PLUGIN_TARGETS;

use crate::accounts;

/// Which app instance hooks belong to. A dev build and an installed one run
/// side by side during development; each believes only its own reports.
const HOOK_ENV_NAME: &str = "production";
pub(crate) const ENDPOINT_DIR_NAME: &str = "agent-hooks";
pub(crate) const SETTINGS_FILE_NAME: &str = "agent-hooks.json";
const DELIVERY_FAILURE_DIR_NAME: &str = "delivery-failures";
/// Below the endpoint directory: one `term-<n>.env` per pane the window has
/// granted the launched-agent capabilities to after the fact — see
/// [`grant_pane_capabilities`].
const PANE_GRANTS_DIR_NAME: &str = "panes";

/// The running bridge's coordinates.
pub struct Bridge {
    pub port: u16,
    pub token: String,
    /// The browser capability's own secret — see `pty_env`'s launch branch.
    pub browser_token: String,
    /// Desktop control is independent of page reading and gets a separate
    /// launched-agent-only capability.
    pub computer_token: String,
    /// Private files handed to shims by path. A missing file is a deliberate
    /// degraded mode: the corresponding capability falls back to its old
    /// environment value so the feature does not silently disappear.
    pub browser_token_file: Option<PathBuf>,
    pub computer_token_file: Option<PathBuf>,
    /// Where the endpoint file landed, when it could be written. `None` means
    /// launches carry the coordinates only in their environment — degraded
    /// across an app restart, not broken. The two capability paths below are
    /// independent so a partial filesystem failure does not lose both doors.
    pub endpoint: Option<PathBuf>,
    /// This window's private lane for failed-delivery markers. The final
    /// component is random per bridge lifetime, so evidence left by a crashed
    /// window cannot accuse a later pane that reused the same terminal id.
    delivery_failure_dir: PathBuf,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

pub fn bridge() -> Option<&'static Bridge> {
    BRIDGE.get()
}

/// Panes granted the launched-agent capabilities after the fact, so a hook
/// arriving every few seconds does not touch the filesystem each time.
static GRANTED_PANES: LazyLock<Mutex<HashSet<u32>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Grant a pane the launched-agent capabilities after the fact.
///
/// The capability paths ride a launch token: a pane the window opened for an
/// agent has them in its environment from birth, a plain shell does not, and
/// an agent somebody types into that shell inherits nothing — its own shim
/// then refuses it ("this pane has no Computer Use capability": a zo started
/// by hand in a pane, 2026-09-02). It is the same agent, and its first hook
/// says so. This writes the paths into a per-pane file the shims source when
/// their environment carries none, so the grant follows the AGENT the window
/// recognised rather than the shell it was typed into; a pane ending takes it
/// back ([`revoke_pane_capabilities`]). `true` when the pane holds a grant
/// after the call.
pub fn grant_pane_capabilities(term: u32) -> bool {
    let Some(bridge) = bridge() else {
        return false;
    };
    if GRANTED_PANES.lock().is_ok_and(|held| held.contains(&term)) {
        return true;
    }
    // No endpoint file means no directory the shims could find a grant in.
    let Some(dir) = bridge.endpoint.as_deref().and_then(Path::parent) else {
        return false;
    };
    let files: Vec<(&str, &Path)> = [
        (
            zerocode_hookd::env_var::COMPUTER_TOKEN_FILE,
            bridge.computer_token_file.as_deref(),
        ),
        (
            zerocode_hookd::env_var::BROWSER_TOKEN_FILE,
            bridge.browser_token_file.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(name, path)| path.map(|path| (name, path)))
    .collect();
    match write_pane_grant(dir, term, &files) {
        Ok(_) => {
            if let Ok(mut held) = GRANTED_PANES.lock() {
                held.insert(term);
            }
            true
        }
        Err(error) => {
            eprintln!("[zerocode] pane {term}: could not write its capability grant: {error}");
            false
        }
    }
}

/// Take a pane's after-the-fact grant back — its terminal ended.
pub fn revoke_pane_capabilities(term: u32) {
    let had = GRANTED_PANES
        .lock()
        .map(|mut held| held.remove(&term))
        .unwrap_or(false);
    if !had {
        return;
    }
    if let Some(dir) = bridge()
        .and_then(|bridge| bridge.endpoint.as_deref())
        .and_then(Path::parent)
    {
        let _ = std::fs::remove_file(pane_grant_path(dir, term));
    }
}

/// `<endpoint dir>/panes/term-<n>.env` — the spelling the shims' loader
/// rebuilds from `ZEROCODE_HOOK_ENDPOINT` and `ZEROCODE_PANE_KEY`.
fn pane_grant_path(dir: &Path, term: u32) -> PathBuf {
    dir.join(PANE_GRANTS_DIR_NAME)
        .join(format!("{}.env", pane_key_of(term)))
}

/// Write a pane's grant: one `NAME='path'` line per capability file, single
/// quoted because the paths under Application Support carry spaces, as a 0600
/// file in a 0700 directory, replaced atomically.
pub(crate) fn write_pane_grant(
    dir: &Path,
    term: u32,
    files: &[(&str, &Path)],
) -> std::io::Result<PathBuf> {
    let grants = dir.join(PANE_GRANTS_DIR_NAME);
    std::fs::create_dir_all(&grants)?;
    set_private_mode(&grants, 0o700)?;
    let mut body = String::new();
    for (name, path) in files {
        let quoted = path.to_string_lossy().replace('\'', "'\\''");
        body.push_str(&format!("{name}='{quoted}'\n"));
    }
    let final_path = pane_grant_path(dir, term);
    let tmp_path = grants.join(format!(".{}-{}.tmp", pane_key_of(term), std::process::id()));
    std::fs::write(&tmp_path, body)?;
    set_private_mode(&tmp_path, 0o600)?;
    if let Err(error) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    Ok(final_path)
}

fn set_private_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// A token nobody can guess, from the operating system's CSPRNG.
///
/// The bridge accepts a POST from anything that can reach loopback, which on a
/// multi-user machine is every local process — the token is the entire
/// authentication, so it cannot come from a clock. `SysRng` selects the native
/// source on every supported target. A machine where that source fails gets no
/// bridge rather than a guessable token.
pub(crate) fn random_token() -> Option<String> {
    use rand::TryRng as _;

    let mut bytes = [0u8; 24];
    rand::rngs::SysRng.try_fill_bytes(&mut bytes).ok()?;

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(HEX[usize::from(byte >> 4)] as char);
        token.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    Some(token)
}

/// Where the endpoint file lives below the injected local-data root.
fn endpoint_dir(local_data_root: &Path) -> PathBuf {
    local_data_root.join(ENDPOINT_DIR_NAME)
}

/// Bind the bridge, write the endpoint file, and hand back the envelope
/// stream. Called once from `main` before the window goes up; the caller
/// drives the stream.
///
/// Every failure path returns `None` and the window opens without a bridge —
/// hooks are a reporting channel, and a window that refuses to start because
/// a port could not bind has its priorities backwards.
pub type HookBridgeReceivers = (
    tokio::sync::mpsc::UnboundedReceiver<zerocode_core::HookEnvelope>,
    tokio::sync::mpsc::Receiver<zerocode_hookd::TeamRequest>,
    tokio::sync::mpsc::UnboundedReceiver<zerocode_hookd::BrowserRequest>,
    tokio::sync::mpsc::UnboundedReceiver<zerocode_hookd::ComputerRequest>,
    tokio::sync::mpsc::UnboundedReceiver<zerocode_hookd::FederationRequest>,
);

pub fn start(local_data_root: &Path) -> Option<HookBridgeReceivers> {
    let token = random_token()?;
    let delivery_epoch = random_token()?;
    // The browser capability's OWN secret: reporting lifecycle events must not
    // imply reading pages. Only a launched-agent shim receives its private
    // capability path (1-g4 리뷰 발견 1).
    let browser_token = random_token()?;
    let computer_token = random_token()?;
    let (state, events, teams, browser, computer, federation) =
        zerocode_hookd::BridgeState::new_with_computer(
            token.clone(),
            browser_token.clone(),
            computer_token.clone(),
        );
    // Every prompt a Claude or Codex pane sends is answered with the second
    // brain's pages for it, from the vault this window has saved — the same
    // effect zo gets from its own prompt section, for the agents that read
    // hooks instead. No vault, no block; the source says nothing.
    let state = state.with_prompt_knowledge(std::sync::Arc::new(
        crate::cmd::second_brain::VaultKnowledge::default(),
    ));
    /* Where a fixed ledger pointer waits for a provider's own hook.
     *
     * Installed unconditionally and for every pane, which is the whole point:
     * a new person with default settings gets internal agent coordination
     * without being told to turn anything on, and a pane that opens, is
     * adopted, or is resumed an hour from now is covered by the same shelf
     * with no launch bookkeeping to get out of step. Which providers are
     * actually offered a route is the catalog's measured answer, asked at the
     * moment of the knock. */
    let state = state.with_artifacts(std::sync::Arc::new(crate::artifact_runtime::ArtifactDoor));
    let state = state.with_pointer_mailbox(crate::orchestration_pointer_mailbox::mailbox());
    let computer_sender = state.computer_requests();
    let addr = tauri::async_runtime::block_on(async {
        zerocode_hookd::serve(state, 0)
            .await
            .ok()
            .map(|(addr, _)| addr)
    })?;
    let endpoint = zerocode_hookd::endpoint::write_endpoint_file(
        &endpoint_dir(local_data_root),
        &zerocode_hookd::endpoint::EndpointFields {
            port: addr.port(),
            token: token.clone(),
            env: HOOK_ENV_NAME.to_string(),
            version: zerocode_hookd::HOOK_CONTRACT_VERSION.to_string(),
        },
    )
    .ok();
    // The endpoint already has the bridge token. Keep the two capability
    // values in separate 0600 files so a pane that receives only a path does
    // not expose either secret through its inherited environment.
    let browser_token_file = zerocode_hookd::endpoint::write_private_token_file(
        &endpoint_dir(local_data_root),
        zerocode_hookd::endpoint::BROWSER_TOKEN_FILE,
        &browser_token,
    )
    .ok();
    let computer_token_file = zerocode_hookd::endpoint::write_private_token_file(
        &endpoint_dir(local_data_root),
        zerocode_hookd::endpoint::COMPUTER_TOKEN_FILE,
        &computer_token,
    )
    .ok();
    // Grants are per pane id, and pane ids restart with the window: a grant a
    // previous window wrote must not answer for whoever gets that id now.
    let _ = std::fs::remove_dir_all(endpoint_dir(local_data_root).join(PANE_GRANTS_DIR_NAME));
    BRIDGE
        .set(Bridge {
            port: addr.port(),
            token,
            browser_token,
            computer_token,
            browser_token_file,
            computer_token_file,
            endpoint,
            delivery_failure_dir: endpoint_dir(local_data_root)
                .join(DELIVERY_FAILURE_DIR_NAME)
                .join(delivery_epoch),
        })
        .ok()?;
    crate::flow_console::install_sender(computer_sender);
    Some((events, teams, browser, computer, federation))
}

/// The variable that tells powerlevel10k not to open its configuration wizard.
///
/// Named here rather than inline because it is read twice — asked about, then
/// set — and a second spelling would seed a variable nobody checks.
const P10K_WIZARD_OFF: &str = "POWERLEVEL9K_DISABLE_CONFIGURATION_WIZARD";

/// This machine's second-brain vault, as the settings document last said.
///
/// `pty_env` builds a pane's environment from a pane key and a worktree; it
/// has no window state to ask and no config root to read, and giving it one
/// would mean seven call sites each answering "where is the vault" for
/// themselves. So the settings layer PUBLISHES the answer here — once at
/// startup and again on every settings write ([`crate::mutate_settings`]) —
/// and the pane environment reads it. Empty means no vault is configured, and
/// then no pane carries the variable at all.
///
/// The same shape `shell_path::hydrated` already has, and for the same
/// reason: a value a PTY child needs that only the window can know.
static SECOND_BRAIN_VAULT: Mutex<String> = Mutex::new(String::new());

/// Tell every pane opened from now on where the second brain is. An empty or
/// blank path means there is none, and the variable stops being carried.
///
/// A RELATIVE path is treated as none. Every pane has its own working
/// directory, so a relative vault would name a different folder in each of
/// them — which is the one thing this variable exists to stop — and the
/// readers on the other side refuse it anyway (zo's `runtime::second_brain`
/// requires an absolute path). Nothing this window writes is relative: the
/// setup command canonicalizes before saving. A hand-edited settings file is
/// what this guard is for.
pub fn publish_second_brain_vault(path: &str) {
    let path = path.trim();
    let absolute = if Path::new(path).is_absolute() {
        path
    } else {
        ""
    };
    if let Ok(mut held) = SECOND_BRAIN_VAULT.lock() {
        absolute.clone_into(&mut held);
    }
}

/// What [`publish_second_brain_vault`] last said, or nothing.
#[must_use]
pub fn second_brain_vault() -> Option<String> {
    SECOND_BRAIN_VAULT
        .lock()
        .ok()
        .map(|held| held.clone())
        .filter(|path| !path.is_empty())
}

/// The hook coordinates a PTY child gets — every terminal, not only agent
/// launches, because an agent started by hand inside a plain shell should
/// report exactly like a launched one. `launch_token` is the launched-agent
/// extra: the nonce that lets a stale envelope from a previous occupant of
/// this pane be told from a live one.
/// `base` is the `PATH` the caller's environment already carries, and asking
/// for it is the whole of what stops this function throwing one away.
///
/// It used to work it out for itself — its own vec first, then the process's —
/// and neither of those is the caller's. So every road that did
/// `env.extend(pty_env(..))` had whatever `PATH` it had built overwritten at
/// spawn time, silently: a summoned worker lost the shims that let it reach the
/// ledger at all, and a person who wrote a `PATH` into an agent's launch
/// environment had it dropped with nothing said. Two of those roads happened to
/// put their own directories back afterwards, which is why this looked like it
/// worked — masked, not safe.
///
/// A parameter rather than a smarter search, because the question is the
/// caller's to answer and only the caller can: the day somebody adds a ninth
/// road, the compiler asks them what this pane's `PATH` should be built on
/// instead of leaving it to be measured by eye, road by road.
pub fn pty_env(
    pane_key: &str,
    launch_token: Option<&str>,
    worktree: &Path,
    base: Option<&str>,
) -> Vec<(String, String)> {
    let Some(bridge) = bridge() else {
        return Vec::new();
    };
    let mut env = vec![
        (
            zerocode_hookd::env_var::PORT.into(),
            bridge.port.to_string(),
        ),
        (
            zerocode_hookd::env_var::TOKEN.into(),
            // An endpoint file is the hook's durable source. The empty pair
            // asks PtyLane to remove an inherited value before the child is
            // created; hook_script sources the file in the child that needs it.
            if bridge.endpoint.is_some() {
                String::new()
            } else {
                bridge.token.clone()
            },
        ),
        (
            zerocode_hookd::env_var::PANE_KEY.into(),
            pane_key.to_string(),
        ),
        (
            zerocode_hookd::env_var::WORKTREE_ID.into(),
            worktree.to_string_lossy().into_owned(),
        ),
        (
            zerocode_hookd::env_var::AGENT_ENV.into(),
            HOOK_ENV_NAME.into(),
        ),
        (
            zerocode_hookd::env_var::VERSION.into(),
            zerocode_hookd::HOOK_CONTRACT_VERSION.into(),
        ),
        // A pane may have been started from another ZeroCode/Claude pane. Do
        // not carry its team capability or Claude's matching IPC credential
        // into this new process. A team launch adds its own path-only file
        // after this boundary.
        (
            zerocode_core::agent_teams::TEAM_TOKEN_VAR.into(),
            String::new(),
        ),
        (
            zerocode_hookd::env_var::CLAUDE_MESSAGING_TOKEN.into(),
            String::new(),
        ),
    ];
    if let Some(endpoint) = &bridge.endpoint {
        env.push((
            zerocode_hookd::env_var::ENDPOINT.into(),
            endpoint.to_string_lossy().into_owned(),
        ));
    }
    // A marker path, not a writable payload path: the generated script only
    // creates or removes this empty directory. Restrict it to pane keys minted
    // by this module so an unexpected caller cannot turn a key into traversal.
    if term_of_pane_key(pane_key).is_some() {
        env.push((
            zerocode_hookd::env_var::DELIVERY_FAILURE_MARKER.into(),
            bridge
                .delivery_failure_dir
                .join(pane_key)
                .to_string_lossy()
                .into_owned(),
        ));
    }
    if let Some(token) = launch_token {
        env.push((
            zerocode_hookd::env_var::LAUNCH_TOKEN.into(),
            token.to_string(),
        ));
        // The browser capability travels ONLY with a launch token — a
        // launched agent's pane, never a plain shell. A build script in a
        // terminal must not be able to read pages with the hook token
        // (1-g4 리뷰 발견 1).
        // Capability values are read by the corresponding shim, never by the
        // agent CLI. Pass only paths when the private files were written.
        // Empty fallback entries clear a secret inherited from the process
        // that opened this window.
        env.push((zerocode_hookd::env_var::BROWSER_TOKEN.into(), String::new()));
        env.push((
            zerocode_hookd::env_var::BROWSER_TOKEN_FILE.into(),
            String::new(),
        ));
        if let Some(path) = &bridge.browser_token_file {
            env.push((
                zerocode_hookd::env_var::BROWSER_TOKEN_FILE.into(),
                path.to_string_lossy().into_owned(),
            ));
        } else {
            env.push((
                zerocode_hookd::env_var::BROWSER_TOKEN.into(),
                bridge.browser_token.clone(),
            ));
        }
        env.push((
            zerocode_hookd::env_var::COMPUTER_TOKEN.into(),
            String::new(),
        ));
        env.push((
            zerocode_hookd::env_var::COMPUTER_TOKEN_FILE.into(),
            String::new(),
        ));
        if let Some(path) = &bridge.computer_token_file {
            env.push((
                zerocode_hookd::env_var::COMPUTER_TOKEN_FILE.into(),
                path.to_string_lossy().into_owned(),
            ));
        } else {
            env.push((
                zerocode_hookd::env_var::COMPUTER_TOKEN.into(),
                bridge.computer_token.clone(),
            ));
        }
        // And Git stops asking questions this pane cannot answer. The launch
        // token is what says "unattended" here — it is minted for a launched
        // agent and for nothing else — which is the same line Orca draws:
        // "unattended agents must fail instead of looping on OS credential
        // prompts; user terminals keep normal Git behavior"
        // (`main/ipc/pty.ts:1783`). A person's own shell is left alone, so
        // typing `git push` in a terminal still gets the prompt it should.
        env.extend(zerocode_core::git_prompt::guard_env(&|name: &str| {
            std::env::var(name).ok()
        }));
    } else {
        // A hand-started agent still receives hook coordinates, but no
        // launched-agent capabilities. Explicitly clear values inherited from
        // a parent agent so a plain shell cannot accidentally become one.
        env.extend([
            (zerocode_hookd::env_var::BROWSER_TOKEN.into(), String::new()),
            (
                zerocode_hookd::env_var::COMPUTER_TOKEN.into(),
                String::new(),
            ),
            (
                zerocode_hookd::env_var::BROWSER_TOKEN_FILE.into(),
                String::new(),
            ),
            (
                zerocode_hookd::env_var::COMPUTER_TOKEN_FILE.into(),
                String::new(),
            ),
            (
                zerocode_hookd::env_var::TEAM_TOKEN_FILE.into(),
                String::new(),
            ),
            (zerocode_hookd::env_var::LAUNCH_TOKEN.into(), String::new()),
        ]);
    }
    // p10k's first-run wizard blocks shell startup, which means it blocks the
    // launch command queued behind it — a pane that looks hung on its first
    // use. Seeded for every pane, agent or not, and only when nothing has said
    // otherwise: somebody who wants the wizard keeps it, and `p10k configure`
    // still runs by hand (Orca seeds the same variable the same way,
    // `main/pty/powerlevel10k-wizard-env.ts:1`).
    if std::env::var_os(P10K_WIZARD_OFF).is_none() {
        env.push((P10K_WIZARD_OFF.to_string(), "true".to_string()));
    }
    // Where the second brain is, for every pane and every agent in it. The
    // vault used to be findable only from inside the vault's own project —
    // its `AGENTS.md` was the only thing that named it — so an agent working
    // anywhere else had never heard of it ("어디서 일하든 세컨드 브레인을
    // 바라보게", 2026-09-03). A pane carries it only while a vault is
    // configured: clearing the setting stops the next pane from being told.
    if let Some(vault) = second_brain_vault() {
        env.push((zerocode_hookd::env_var::SECOND_BRAIN.into(), vault));
    }
    // What this pane's PATH is built on, in the order of who knows best.
    //
    // The CALLER first, because a caller that already put a PATH in its
    // environment did so on purpose — a person's launch override, a leader's
    // shim directories — and this road used to overwrite both without a word.
    //
    // Then the user's real PATH, when the shell has answered. That is what lets
    // a Finder-launched window START an agent at all: the program name is
    // resolved against the child's PATH, and launchd's has no homebrew, no npm,
    // no nvm on it. A plain shell rebuilds its own PATH anyway; an agent binary
    // cannot. Absent until hydration lands — a launch must not wait five
    // seconds on a stuck rc file, and inheriting the process PATH is exactly
    // what it did before.
    let standing = base
        .map(str::to_string)
        .or_else(crate::shell_path::hydrated)
        .or_else(|| std::env::var("PATH").ok());
    // The mirror shims go at the HEAD of it. An agent run as a command inside
    // these terminals (`codex exec` under a Bash tool) writes to a pipe its
    // parent owns — the shim tees those bytes into a file this window can pour
    // into a real pane (1-fm). Interactive runs are untouched: the shim execs
    // the real binary the moment it sees a tty.
    //
    // ONE `PATH` pair leaves here, whatever happens. Two was the shape that
    // hid the overwrite: the second was computed from the first rather than
    // from the caller, and at spawn time the later pair simply won.
    match (mirror_shims(), standing) {
        (Some((shims, mirrors)), standing) => {
            env.push((
                "ZEROCODE_MIRROR_DIR".into(),
                mirrors.to_string_lossy().into_owned(),
            ));
            env.push((
                "PATH".into(),
                zerocode_core::agent_teams::shim_path(
                    &[&shims.to_string_lossy()],
                    standing.as_deref().unwrap_or_default(),
                ),
            ));
        }
        (None, Some(standing)) => env.push(("PATH".into(), standing)),
        (None, None) => {}
    }
    env
}

/// Add the runtime readers used by generated shims.
///
/// The endpoint source supplies the hook token from the already-private
/// endpoint file. Each extra pair supplies a capability token from its own
/// 0600 file. The generated script keeps the value only in the short-lived
/// shim process and its intentional curl child; a normal child of the agent
/// sees paths, not secrets.
///
/// A pane whose environment names no capability file — an agent typed into a
/// plain shell — reads the window's after-the-fact grant for its pane instead
/// (`grant_pane_capabilities`), from the endpoint's own directory and only
/// for a pane key of the window's spelling. The environment still wins when
/// it carries a path.
pub(crate) fn shim_script_with_private_tokens(
    script: String,
    private_tokens: &[(&str, &str)],
) -> String {
    let endpoint = zerocode_hookd::env_var::ENDPOINT;
    let pane_key = zerocode_hookd::env_var::PANE_KEY;
    // A Windows window spells the endpoint path with backslashes; Git Bash
    // reads such a path but `${endpoint%/*}` below cannot cut a directory
    // off it. One spelling before anything looks at it.
    let mut loader = format!(
        r#"case "${{{endpoint}:-}}" in
  *\\*) {endpoint}=$(printf '%s' "${{{endpoint}}}" | tr '\\' '/') ;;
esac
if [ -n "${{{endpoint}:-}}" ] && [ -r "${{{endpoint}}}" ]; then
  . "${{{endpoint}}}" 2>/dev/null || :
fi
"#
    );
    let missing = private_tokens
        .iter()
        .map(|(_, file_var)| format!(r#"[ -z "${{{file_var}:-}}" ]"#))
        .collect::<Vec<_>>()
        .join(" || ");
    if !missing.is_empty() {
        loader.push_str(&format!(
            r#"if {{ {missing}; }} && [ -n "${{{endpoint}:-}}" ] && [ -n "${{{pane_key}:-}}" ]; then
  case "${{{pane_key}}}" in
    term-[0-9]*)
      grant="${{{endpoint}%/*}}/{PANE_GRANTS_DIR_NAME}/${{{pane_key}}}.env"
      if [ -r "$grant" ]; then
        . "$grant" 2>/dev/null || :
      fi
      ;;
  esac
fi
"#
        ));
    }
    for (token_var, file_var) in private_tokens {
        loader.push_str(&format!(
            r#"if [ -n "${{{file_var}:-}}" ] && [ -r "${{{file_var}}}" ]; then
  {token_var}=$({{ command -p cat "${{{file_var}}}" 2>/dev/null || cat "${{{file_var}}}"; }}) || :
  export {token_var}
fi
"#
        ));
    }
    let Some((_, rest)) = script.split_once('\n') else {
        return script;
    };
    format!(
        "{}\n{}{}",
        script.lines().next().unwrap_or_default(),
        loader,
        rest
    )
}

/// The agent mirror wrapper's conditional loader.
///
/// The wrapper must load credentials only on the mirror path. If the mirror is
/// stale it falls through to the real agent, and handing that fallback a bridge
/// token would recreate the inheritance bug this boundary closes.
pub(crate) fn mirror_shim_script(mirror: &Path, real: &Path) -> String {
    let endpoint = zerocode_hookd::env_var::ENDPOINT;
    let hook_token = zerocode_hookd::env_var::TOKEN;
    let team_token = zerocode_core::agent_teams::TEAM_TOKEN_VAR;
    let team_token_file = zerocode_hookd::env_var::TEAM_TOKEN_FILE;
    format!(
        r#"#!/bin/sh
if [ -x "{mirror}" ]; then
  if [ -n "${{{endpoint}:-}}" ] && [ -r "${{{endpoint}}}" ]; then
    . "${{{endpoint}}}" 2>/dev/null || :
    export {hook_token}
  fi
  if [ -n "${{{team_token_file}:-}}" ] && [ -r "${{{team_token_file}}}" ]; then
    {team_token}=$({{ command -p cat "${{{team_token_file}}}" 2>/dev/null || cat "${{{team_token_file}}}"; }}) || :
    export {team_token}
  fi
  exec "{mirror}" "{real}" "$@"
fi
exec "{real}" "$@"
"#,
        mirror = mirror.display(),
        real = real.display(),
    )
}

/// The same loader for the PowerShell companion of the orchestration shim.
///
/// The endpoint is written in the POSIX assignment form even on the current
/// Windows-compatible path, so only the one hook-token line is read here. The
/// capability files contain a bare token and are read without invoking a shell.
#[cfg(any(windows, test))]
pub(crate) fn powershell_shim_script_with_private_tokens(
    script: String,
    private_tokens: &[(&str, &str)],
) -> String {
    let endpoint = zerocode_hookd::env_var::ENDPOINT;
    let hook_token = zerocode_hookd::env_var::TOKEN;
    let mut loader = format!(
        r#"$endpoint = $env:{endpoint}
if ($endpoint -and (Test-Path -LiteralPath $endpoint -PathType Leaf)) {{
  foreach ($line in (Get-Content -LiteralPath $endpoint -ErrorAction SilentlyContinue)) {{
    if ($line.StartsWith('{hook_token}=')) {{
      $env:{hook_token} = $line.Substring('{hook_token}='.Length)
    }}
  }}
}}
"#
    );
    for (token_var, file_var) in private_tokens {
        loader.push_str(&format!(
            r#"$tokenFile = $env:{file_var}
if ($tokenFile -and (Test-Path -LiteralPath $tokenFile -PathType Leaf)) {{
  try {{ $env:{token_var} = [System.IO.File]::ReadAllText($tokenFile).Trim() }} catch {{}}
}}
"#
        ));
    }
    format!("{loader}{script}")
}

/// Tell a launched agent's hook that its first prompt already carries the
/// orchestration selection contract. Kept beside `pty_env`, because this is a
/// hook transport bit — not an agent-specific launch argument — and both the
/// ordinary launcher and the worker host must spell it identically.
pub fn mark_selection_seeded(env: &mut Vec<(String, String)>, prompt: &str) {
    let key = zerocode_hookd::env_var::SELECTION_SEEDED;
    env.retain(|(name, _)| name != key);
    if zerocode_core::delegation::has_agent_selection_contract(prompt) {
        env.push((key.to_string(), "1".to_string()));
    }
}

/// The shim and mirror directories, built once for this window's lifetime —
/// or `None` when the mirror binary is not beside this executable, which is
/// the graceful absence every launch keeps working through.
pub fn mirror_shims() -> Option<(PathBuf, PathBuf)> {
    use std::sync::OnceLock;
    static HELD: OnceLock<Option<(PathBuf, PathBuf)>> = OnceLock::new();
    HELD.get_or_init(build_mirror_shims).clone()
}

/// Where the mirror binary lives: beside this executable, always.
///
/// Named in one place because two questions ask it — whether shims can be
/// written at all ([`build_mirror_shims`]) and whether the binary they name is
/// still there ([`mirror_ready`]) — and a second spelling of this path would
/// let those two answers drift apart.
fn mirror_bin() -> Option<PathBuf> {
    // With the platform's executable suffix: on Windows the file beside the
    // window is `zerocode-mirror.exe`, and asking for the bare name found
    // nothing — which built no shim directory at all, and with it none of
    // the four agent doors on any pane's PATH (t-2628 inventory, 2026-09-05).
    Some(
        std::env::current_exe()
            .ok()?
            .parent()?
            .join(format!("zerocode-mirror{}", std::env::consts::EXE_SUFFIX)),
    )
}

/// Whether a nested run can be mirrored RIGHT NOW.
///
/// [`mirror_shims`] answers once for the window's lifetime, because the shims
/// are written once. This is the other question, and it has a different
/// answer: the absolute path those shims hold can go stale under a running
/// window — a release build replacing its own output is the measured case —
/// and from that moment every nested run has no live output at all, because
/// the shim falls through to the real agent by design.
///
/// That fallback is documented above as *a reported outage*. Nothing reported
/// it: a person saw a page with a command on it and nothing else, and had no
/// way to tell a quiet run from broken plumbing ("아무것도 안보임"). This is
/// what reports it.
#[must_use]
pub fn mirror_ready() -> bool {
    mirror_shims().is_some() && mirror_bin().is_some_and(|bin| bin.is_file())
}

fn build_mirror_shims() -> Option<(PathBuf, PathBuf)> {
    let mirror_bin = mirror_bin()?;
    if !mirror_bin.is_file() {
        return None;
    }
    let stamp = std::process::id();
    let shims = std::env::temp_dir().join(format!("zerocode-shims-{stamp}"));
    let mirrors = std::env::temp_dir().join(format!("zerocode-mirror-{stamp}"));
    write_shim_dir(&shims, &mirror_bin, resolvable_agents().as_slice()).ok()?;
    std::fs::create_dir_all(&mirrors).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&mirrors, std::fs::Permissions::from_mode(0o700));
    }
    Some((shims, mirrors))
}

/// Every lane binary this machine can actually resolve, by the user's shell
/// PATH—the same path used when a launch is handed to a child.
fn resolvable_agents() -> Vec<(String, PathBuf)> {
    let path = crate::shell_path::hydrate(false)
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    resolvable_agents_in(path.as_deref())
}

/// Resolve the lane agents against one PATH. Keeping the path walk here as a
/// pure input makes the one-time shim decision testable without mutating the
/// process environment; the executable check itself is shared with agent
/// presence reporting in `zerocode-core`.
fn resolvable_agents_in(path: Option<&std::ffi::OsStr>) -> Vec<(String, PathBuf)> {
    zerocode_core::agent::ALL_AGENTS
        .into_iter()
        // Zo publishes its pane-owned event channel and is adopted by the
        // shell. Putting it behind a mirror hook would create a second,
        // weaker lifecycle source and race the structured turn frames.
        .filter(|kind| *kind != zerocode_core::agent::AgentKind::Zo)
        .filter_map(|kind| {
            let name = kind.command();
            zerocode_core::agent::resolve_on_path(path, name).map(|held| (name.to_string(), held))
        })
        .collect()
}

/// One shim per resolved agent: an `exec` through the mirror when the mirror
/// is there, and straight to the real binary when it is not.
///
/// The fallback is not caution, it is a reported outage. These shims sit at
/// the HEAD of every pane's PATH and they name the mirror by absolute path
/// inside the running bundle. Replace or delete that bundle while the window
/// is still up — which is what a release build does to its own output — and
/// the path goes stale: the running window keeps working, because unix keeps
/// a deleted binary alive for the process that opened it, but every shim on
/// it now points at nothing. `/bin/sh` answers a missing exec target with
/// **126**, so every new agent terminal opened after that moment appeared,
/// died in five milliseconds, and left a dead pane with no explanation
/// (measured: `term 19 spawned claude` / `term 19 ended code Some(126)`).
///
/// Mirroring is a convenience — it tees a piped agent's bytes into a file a
/// pane can pour out. Losing it must cost the mirror, not the terminal.
fn write_shim_dir(
    dir: &Path,
    mirror_bin: &Path,
    agents: &[(String, PathBuf)],
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    for (name, real) in agents {
        let body = mirror_shim_script(mirror_bin, real);
        let held = dir.join(name);
        std::fs::write(&held, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    // The agents' browser door rides the same directory: it is already at the
    // head of every pane's PATH. The hook token comes from the private endpoint
    // file and the browser capability from its own private file; neither value
    // needs to be present in every pane's environment.
    let browser = dir.join("zerocode-browser");
    std::fs::write(
        &browser,
        shim_script_with_private_tokens(
            zerocode_core::agent_browser::shim_script(
                zerocode_hookd::env_var::PORT,
                zerocode_hookd::env_var::BROWSER_TOKEN,
                zerocode_hookd::env_var::TOKEN,
            ),
            &[(
                zerocode_hookd::env_var::BROWSER_TOKEN,
                zerocode_hookd::env_var::BROWSER_TOKEN_FILE,
            )],
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755))?;
    }
    let artifact = dir.join(zerocode_core::artifact_publish::SHIM);
    std::fs::write(
        &artifact,
        shim_script_with_private_tokens(
            zerocode_core::artifact_publish::shim_script(
                zerocode_hookd::env_var::PORT,
                zerocode_hookd::env_var::TOKEN,
                false,
            ),
            &[],
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755))?;
    }
    let computer = dir.join(zerocode_core::computer_use::COMPUTER_CLI);
    std::fs::write(
        &computer,
        shim_script_with_private_tokens(
            zerocode_core::computer_use::shim_script(
                zerocode_hookd::env_var::PORT,
                zerocode_hookd::env_var::COMPUTER_TOKEN,
                zerocode_hookd::env_var::TOKEN,
            ),
            &[(
                zerocode_hookd::env_var::COMPUTER_TOKEN,
                zerocode_hookd::env_var::COMPUTER_TOKEN_FILE,
            )],
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&computer, std::fs::Permissions::from_mode(0o755))?;
    }
    // Mobile work never falls through the desktop accessibility provider.
    // This sibling command uses the same launched-agent capability and bridge,
    // but prefixes its envelope so the window dispatches it to the emulator
    // pane's own iOS/Android backend.
    let emulator = dir.join("zerocode-emulator");
    std::fs::write(
        &emulator,
        shim_script_with_private_tokens(
            zerocode_core::computer_use::emulator_shim_script(
                zerocode_hookd::env_var::PORT,
                zerocode_hookd::env_var::COMPUTER_TOKEN,
                zerocode_hookd::env_var::TOKEN,
            ),
            &[(
                zerocode_hookd::env_var::COMPUTER_TOKEN,
                zerocode_hookd::env_var::COMPUTER_TOKEN_FILE,
            )],
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&emulator, std::fs::Permissions::from_mode(0o755))?;
    }
    // And remote work never falls through to clicking this window either. The
    // machines ZeroCode can already reach — saved SSH hosts, remote
    // workspaces, remote servers — and its own local shells answer here, in
    // panes the person can watch and stop (live report 2026-08-25: 컴퓨터
    // 유즈로 "zerocode안에 모든 브라우저 및 기술을 ssh등 사용할수있어야해").
    let ssh = dir.join("zerocode-ssh");
    std::fs::write(
        &ssh,
        shim_script_with_private_tokens(
            zerocode_core::computer_use::ssh_shim_script(
                zerocode_hookd::env_var::PORT,
                zerocode_hookd::env_var::COMPUTER_TOKEN,
                zerocode_hookd::env_var::TOKEN,
            ),
            &[(
                zerocode_hookd::env_var::COMPUTER_TOKEN,
                zerocode_hookd::env_var::COMPUTER_TOKEN_FILE,
            )],
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755))?;
    }
    // Windows hosts: cmd resolves a `.cmd` through PATHEXT and PowerShell
    // resolves a bare name to a `.cmd` too — as long as no `<door>.ps1`
    // stands beside it, which PowerShell would prefer and its default
    // execution policy would then refuse. Both companions sit beside the
    // POSIX file Git Bash keeps reading — the same four doors, the same
    // private token files, the same routes.
    #[cfg(windows)]
    {
        for (name, body) in windows_door_scripts() {
            std::fs::write(dir.join(name), body)?;
        }
        // The agent mirrors too: `claude` typed in a Windows pane resolves to
        // `claude.cmd` here, at the head of PATH, before the real one.
        for (name, body) in windows_agent_mirror_scripts(mirror_bin, agents) {
            std::fs::write(dir.join(name), body)?;
        }
    }
    Ok(())
}

/// The name PowerShell will NOT resolve a bare door name to: command
/// discovery tries `<name>.ps1` (before PATHEXT) and `<name>.cmd`, never
/// `<name>.impl.ps1`, so every host — cmd, PowerShell 5.1 under the default
/// `Restricted` policy, PowerShell 7 — reaches the `.cmd`, which runs the
/// implementation with a per-process `-ExecutionPolicy Bypass`. The machine
/// policy is never touched.
#[cfg(any(windows, test))]
fn door_implementation_name(door: &str) -> String {
    format!("{door}.impl.ps1")
}

/// Windows PowerShell 5.1 reads a `.ps1` without a byte-order mark in the
/// ANSI code page; the manuals carry non-ASCII (an em dash), so the file
/// says what it is.
#[cfg(any(windows, test))]
const UTF8_BOM: &str = "\u{FEFF}";

/// The eight files a Windows shim directory carries beside the four POSIX
/// doors: a `<door>.impl.ps1` that posts in-process and a `<door>.cmd` that
/// reaches it.
///
/// Pure, and compiled for tests everywhere, so the shape is pinned on every
/// platform that builds this crate rather than only on the one that writes
/// the files.
#[cfg(any(windows, test))]
fn windows_door_scripts() -> Vec<(String, String)> {
    use zerocode_hookd::env_var::{
        BROWSER_TOKEN, BROWSER_TOKEN_FILE, COMPUTER_TOKEN, COMPUTER_TOKEN_FILE, PORT, TOKEN,
    };
    let doors: [(&str, String, (&str, &str)); 5] = [
        (
            zerocode_core::artifact_publish::SHIM,
            zerocode_core::artifact_publish::shim_script(PORT, TOKEN, true),
            (TOKEN, ""),
        ),
        (
            "zerocode-browser",
            zerocode_core::agent_browser::shim_script_powershell(PORT, BROWSER_TOKEN, TOKEN),
            (BROWSER_TOKEN, BROWSER_TOKEN_FILE),
        ),
        (
            zerocode_core::computer_use::COMPUTER_CLI,
            zerocode_core::computer_use::shim_script_powershell(PORT, COMPUTER_TOKEN, TOKEN),
            (COMPUTER_TOKEN, COMPUTER_TOKEN_FILE),
        ),
        (
            "zerocode-emulator",
            zerocode_core::computer_use::emulator_shim_script_powershell(
                PORT,
                COMPUTER_TOKEN,
                TOKEN,
            ),
            (COMPUTER_TOKEN, COMPUTER_TOKEN_FILE),
        ),
        (
            "zerocode-ssh",
            zerocode_core::computer_use::ssh_shim_script_powershell(PORT, COMPUTER_TOKEN, TOKEN),
            (COMPUTER_TOKEN, COMPUTER_TOKEN_FILE),
        ),
    ];
    doors
        .into_iter()
        .flat_map(|(name, script, (token_var, file_var))| {
            let powershell = door_implementation_name(name);
            let body = format!(
                "{UTF8_BOM}{}",
                powershell_shim_script_with_private_tokens(
                    script,
                    &[(token_var, file_var)]
                        .into_iter()
                        .filter(|(_, path)| !path.is_empty())
                        .collect::<Vec<_>>()
                )
            );
            let wrapper = zerocode_core::agent_teams::shim_cmd(&powershell);
            [(powershell, body), (format!("{name}.cmd"), wrapper)]
        })
        .collect()
}

/// The PowerShell body of an agent's mirror shim: the mirror binary with the
/// real path and the pane's arguments when the mirror is there, the real
/// binary alone when it is not — each wearing the child's exit code, as the
/// POSIX [`mirror_shim_script`] does with `exec`. Paths ride as PowerShell
/// literal strings ([`powershell_literal`]).
#[cfg(any(windows, test))]
fn mirror_shim_script_powershell(mirror: &Path, real: &Path) -> String {
    let mirror = powershell_literal(&mirror.to_string_lossy());
    let real = powershell_literal(&real.to_string_lossy());
    format!(
        "if (Test-Path -LiteralPath {mirror} -PathType Leaf) {{\r\n  & {mirror} {real} @args\r\n  exit $LASTEXITCODE\r\n}}\r\n& {real} @args\r\nexit $LASTEXITCODE\r\n"
    )
}

/// The characters that open and close a PowerShell single-quoted string:
/// the ASCII apostrophe and the typographic U+2018, U+2019, U+201A and
/// U+201B, which PowerShell's tokenizer treats exactly alike.
#[cfg(any(windows, test))]
const POWERSHELL_SINGLE_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// `text` as a PowerShell single-quoted (verbatim) string. Inside one, any
/// of [`POWERSHELL_SINGLE_QUOTES`] ends it and a doubled one stands for
/// itself, so each is doubled; nothing else is special there. Escaping only
/// the ASCII `'` let a profile folder like `O’Neil` close the literal and
/// hand the rest of the path to the parser as script.
#[cfg(any(windows, test))]
fn powershell_literal(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for character in text.chars() {
        if POWERSHELL_SINGLE_QUOTES.contains(&character) {
            quoted.push(character);
        }
        quoted.push(character);
    }
    quoted.push('\'');
    quoted
}

/// The Windows companions of the agent mirror shims, in the doors' shape:
/// for every resolved agent an `<agent>.impl.ps1` that loads the endpoint
/// and the team token the way the POSIX shim does, then runs the mirror, and
/// an `<agent>.cmd` that reaches it through PATHEXT — or, when a Group
/// Policy refuses the script, the real agent itself
/// ([`zerocode_core::agent_teams::agent_shim_cmd`]). Never an `<agent>.ps1`.
///
/// Pure, and compiled for tests everywhere, so the shape is pinned on every
/// platform that builds this crate.
#[cfg(any(windows, test))]
fn windows_agent_mirror_scripts(
    mirror_bin: &Path,
    agents: &[(String, PathBuf)],
) -> Vec<(String, String)> {
    agents
        .iter()
        .flat_map(|(name, real)| {
            let powershell = door_implementation_name(name);
            let body = format!(
                "{UTF8_BOM}{}",
                powershell_shim_script_with_private_tokens(
                    mirror_shim_script_powershell(mirror_bin, real),
                    &[(
                        zerocode_core::agent_teams::TEAM_TOKEN_VAR,
                        zerocode_hookd::env_var::TEAM_TOKEN_FILE,
                    )],
                )
            );
            // The agent's own door: when a Group Policy refuses the
            // script, the real agent still starts (without the mirror).
            let wrapper =
                zerocode_core::agent_teams::agent_shim_cmd(&powershell, &real.to_string_lossy());
            [(powershell, body), (format!("{name}.cmd"), wrapper)]
        })
        .collect()
}

/// The pane key a terminal id resolves to, and back. One spelling, owned here,
/// because the envelope's `pane_key` and the launch env must agree forever.
pub fn pane_key_of(term: u32) -> String {
    format!("term-{term}")
}

pub fn term_of_pane_key(pane_key: &str) -> Option<u32> {
    pane_key.strip_prefix("term-")?.parse().ok()
}

/// Delivery failures the script left for this bridge lifetime.
///
/// This is called from the readiness pass on the window's existing one-second
/// beat. The directory does not exist until something fails, so the healthy
/// path is one failed `read_dir`, not a stat per pane. Marker mtimes preserve
/// the first failed attempt because the script uses atomic `mkdir` and leaves
/// an existing marker untouched.
fn delivery_failures_in(dir: &Path, now_ms: i64) -> std::collections::HashMap<u32, i64> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return std::collections::HashMap::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name();
            let term = term_of_pane_key(name.to_str()?)?;
            // Declined, not dated NOW. `read_dir` names an entry and
            // `metadata` is a second syscall against it — and between the two
            // a successful delivery may have taken the marker away. Answering
            // `now` there would invent the failure it could not read: the
            // actor writes that fabricated moment into the ledger, and the
            // board draws a pane whose payload just landed as unreachable.
            // Nothing is lost by declining, because the scan is
            // level-triggered — a marker that really stands is named again on
            // the next beat, with its own time.
            let marked_ms = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|marked| marked.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
                .map(|marked| marked.min(now_ms))?;
            Some((term, marked_ms))
        })
        .collect()
}

fn delivery_failure_cache() -> &'static std::sync::Mutex<std::collections::HashMap<u32, i64>> {
    static FAILURES: OnceLock<std::sync::Mutex<std::collections::HashMap<u32, i64>>> =
        OnceLock::new();
    FAILURES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Refresh the window's memory from the script markers, on the beat that
/// already performs readiness judgement, and return the actor facts.
pub(crate) fn sweep_delivery_failures(now_ms: i64) -> Vec<(u32, i64)> {
    let found = bridge()
        .map(|bridge| delivery_failures_in(&bridge.delivery_failure_dir, now_ms))
        .unwrap_or_default();
    let answer = found.iter().map(|(&term, &at)| (term, at)).collect();
    *delivery_failure_cache()
        .lock()
        .unwrap_or_else(|held| held.into_inner()) = found;
    answer
}

/// The last beat's direct evidence for all panes. Board paint clones it once;
/// it never touches the filesystem or takes one lock per card.
pub(crate) fn delivery_failures_cached() -> std::collections::HashMap<u32, i64> {
    delivery_failure_cache()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone()
}

/// A payload reached the window, so the marker standing for this pane is
/// evidence about a delivery that is over.
///
/// The script's `rmdir` is its LAST act, and a script does not always get to
/// finish: an agent killed at the end of its turn, or one whose answer was
/// lost after the body was already ours, leaves the marker behind — and
/// nothing but that pane's next successful POST would ever take it back, so
/// the beat would read the same stale word every second until then. The
/// window holds the payload; the window is what erases it, on disk where the
/// next beat looks and not only in the cache this paint reads.
///
/// Gated on the cache so a healthy pane costs no syscall at all: the beat has
/// already looked, and a marker it did not see is not one this envelope can
/// speak for. A marker that appears between beats is a failure of its own and
/// is left for the beat to report.
pub(crate) fn delivery_arrived(term: u32) {
    let stood = delivery_failure_cache()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term)
        .is_some();
    if stood && let Some(bridge) = bridge() {
        forget_delivery_failure_in(&bridge.delivery_failure_dir, term);
    }
}

/// Take one pane's marker back. `remove_dir` rather than `remove_dir_all`:
/// the marker is an empty directory by construction, and one that has grown
/// contents is not this window's to delete.
fn forget_delivery_failure_in(dir: &Path, term: u32) {
    let _ = std::fs::remove_dir(dir.join(pane_key_of(term)));
}

// ------------------------------------------------------------------ settings

/// Whether managed hooks are wanted, default ON — Orca's
/// `agentStatusHooksEnabled !== false`, stored the same way: only an explicit
/// false turns it off.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct HookSettings {
    enabled: bool,
}

fn settings_file(config_root: &Path) -> PathBuf {
    config_root.join(SETTINGS_FILE_NAME)
}

pub fn hooks_enabled(config_root: &Path) -> bool {
    std::fs::read_to_string(settings_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str::<HookSettings>(&text).ok())
        .map(|settings| settings.enabled)
        .unwrap_or(true)
}

pub fn set_hooks_enabled(config_root: &Path, enabled: bool) -> Result<(), String> {
    let file = settings_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(&HookSettings { enabled })
        .map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

// ----------------------------------------------------------------- reconcile

/// Make the agents' config files agree with the setting.
///
/// Orca's `applyAgentStatusHooksEnabled`, with the same three gates: on means
/// install — but only for agents whose CLI is actually on this machine, so a
/// tool the user never installed does not grow a config directory it never
/// made; off means remove for every managed agent, presence or not, because
/// entries left behind are the thing "off" was asked to end.
pub fn reconcile(
    config_root: &Path,
    local_data_root: &Path,
    installed_slugs: &[String],
) -> Vec<HookStatus> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let paths = InstallPaths::from_environment(home.clone())
        .with_claude_extras(claude_account_settings(config_root));
    let enabled = hooks_enabled(config_root);
    let settings = MANAGED_TARGETS.iter().map(|&agent| {
        if !enabled {
            zerocode_hookd::install::remove_agent(&paths, agent)
        } else if installed_slugs.iter().any(|slug| slug == agent.slug()) {
            zerocode_hookd::install::install_agent(&paths, agent)
        } else {
            zerocode_hookd::install::status_of(&paths, agent)
        }
    });
    // The agents that take a plugin FILE rather than a settings entry, on the
    // same three gates. A separate module because the mechanism differs at every
    // step — nothing of the user's is edited, and "installed" means a file
    // carrying every handler — not because the policy does.
    let plugins = PLUGIN_TARGETS.iter().map(|&agent| {
        if !enabled {
            zerocode_hookd::plugin::remove_agent(&home, agent)
        } else if installed_slugs.iter().any(|slug| slug == agent.slug()) {
            zerocode_hookd::plugin::install_agent(&home, agent)
        } else {
            zerocode_hookd::plugin::status_of(&home, agent)
        }
    });
    // And Codex, whose mechanism differs at every step: the hook has to be
    // trusted before it will run, the trust comes from Codex itself, and a
    // refusal falls to a home this window owns. Its own lane for the same reason
    // the plugin agents have theirs.
    let codex = if !enabled {
        Some(zerocode_hookd::codex_install::remove_agent(&codex_lane(
            &home,
            local_data_root,
        )))
    } else if installed_slugs
        .iter()
        .any(|slug| slug == AgentKind::Codex.slug())
    {
        Some(zerocode_hookd::codex_install::install_agent(
            &codex_lane(&home, local_data_root),
            |entries| grant_codex_trust(&home, entries),
        ))
    } else {
        Some(zerocode_hookd::codex_install::status_of(&codex_lane(
            &home,
            local_data_root,
        )))
    };

    settings.chain(plugins).chain(codex).collect()
}

/// Where the two Codex lanes read and write.
fn codex_lane(home: &Path, local_data_root: &Path) -> zerocode_hookd::codex_install::Lane {
    zerocode_hookd::codex_install::Lane {
        home: home.to_path_buf(),
        user_data: local_data_root.to_path_buf(),
        state_dir: local_data_root.to_path_buf(),
    }
}

/// Ask the Codex on this machine to trust the entries we just wrote.
///
/// Against Codex's OWN default home, which is why `use_default_home` is true: the
/// grant has to land in the config the user's `codex` reads, and a `CODEX_HOME`
/// inherited from this process would put it somewhere else.
fn grant_codex_trust(
    home: &Path,
    entries: &[zerocode_hookd::codex_trust::TrustEntry],
) -> Result<zerocode_hookd::codex_grant::Grant, zerocode_hookd::codex_grant::GrantError> {
    let system = zerocode_hookd::codex_install::system_home(home);
    let plan = zerocode_hookd::codex_grant::GrantPlan {
        invocation: zerocode_hookd::codex_grant::Invocation::native(
            AgentKind::Codex.command(),
            &system,
            true,
        ),
        hooks_list_cwd: home.to_path_buf(),
        expected_keys: entries
            .iter()
            .map(|entry| {
                zerocode_hookd::codex_grant::normalize_listing_key(
                    &zerocode_hookd::codex_trust::trust_key(entry),
                )
            })
            .collect(),
        managed_command: entries
            .first()
            .map(|entry| entry.command.clone())
            .unwrap_or_default(),
    };
    zerocode_hookd::codex_grant::grant(&plan)
}

/// What a Codex launch has to carry beyond the bridge coordinates.
///
/// Empty unless the mirror lane is the live one, and the answer comes from
/// reading the mirror rather than from remembering the install: a home whose
/// hook was removed by hand must stop being used, not keep being pointed at.
pub fn agent_launch_env(local_data_root: &Path, agent: &str) -> Vec<(String, String)> {
    agent_launch_env_with_lock(local_data_root, agent).0
}

/// Prepare a named agent's environment and, for Codex's mirror lane, return
/// the lease that must live through the worker's PTY spawn.
///
/// Most launch roads only need the environment and keep the old helper above.
/// A teammate is different: its caller is one of several blocking team tasks,
/// so it carries this guard until `PtyLane::spawn` has created the child.
pub fn agent_launch_env_with_lock(
    local_data_root: &Path,
    agent: &str,
) -> (
    Vec<(String, String)>,
    Option<zerocode_hookd::codex_runtime_auth::LaunchLock>,
) {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let (mut env, lock) = agent_launch_env_with_lock_inner(local_data_root, agent, home.as_deref());
    env.extend(router_env_for(
        agent,
        crate::api_routers::router_launch_env_from_disk,
    ));
    (env, lock)
}

/// The router keys the window holds (settings → API 라우터) ride a zo launch
/// only: zo is the one agent that reads the `providers[]` table those keys are
/// named in, and a key handed to an agent that never reads it is exposure with
/// no reader. Every launch road — pane, board, automation, worker, file —
/// comes through [`agent_launch_env_with_lock`], so this is the one place.
fn router_env_for(
    agent: &str,
    read: impl FnOnce() -> Vec<(String, String)>,
) -> Vec<(String, String)> {
    if agent == zerocode_core::agent::AgentKind::Zo.slug() {
        read()
    } else {
        Vec::new()
    }
}

/// [`agent_launch_env_with_lock`] with the machine's home named rather than
/// read, so a test can hand it one that is not the developer's.
fn agent_launch_env_with_lock_inner(
    local_data_root: &Path,
    agent: &str,
    system_home: Option<&Path>,
) -> (
    Vec<(String, String)>,
    Option<zerocode_hookd::codex_runtime_auth::LaunchLock>,
) {
    if !zerocode_core::account::providers_for(agent)
        .contains(&zerocode_core::account::Provider::OpenAi)
    {
        return (Vec::new(), None);
    }
    let Some(home) = system_home else {
        return (Vec::new(), None);
    };
    let runtime = crate::codex_accounts::runtime_home(local_data_root);
    let mut record = crate::codex_accounts::read_runtime_record(&runtime);
    if record.account.is_none() {
        let store = crate::codex_accounts::read_store(local_data_root);
        if zerocode_core::codex_account::active_account(&store.accounts, &store.selection).is_some()
        {
            let _ = crate::codex_accounts::materialize(local_data_root, local_data_root);
            record = crate::codex_accounts::read_runtime_record(&runtime);
        }
    }
    if record.account.is_some() {
        // A managed account is active. Its auth was already materialized into
        // the runtime home, so we do NOT sync with ~/.codex — doing so would
        // clobber ~/.codex with another login or clobber the mirror with the
        // system default. We only acquire the mirror launch lock and point
        // CODEX_HOME at the runtime mirror.
        let mirror = zerocode_hookd::codex_mirror::Home::under(local_data_root);
        let lock = zerocode_hookd::codex_runtime_auth::acquire_launch_lock(&mirror).ok();
        return (
            vec![(
                "CODEX_HOME".to_string(),
                mirror.path().to_string_lossy().into_owned(),
            )],
            lock,
        );
    }
    zerocode_hookd::codex_install::launch_env_with_lock(&codex_lane(home, local_data_root))
}

/// A pane ended, so return a rotated Codex credential while its provenance is
/// still available. Other agents have no mirror credential to synchronize.
///
/// Best effort belongs to the caller, matching launch preparation: a failed
/// auth sync must never keep a terminal from closing.
pub fn sync_agent_auth(
    local_data_root: &Path,
    agent: &str,
) -> std::io::Result<Option<zerocode_hookd::codex_runtime_auth::AuthSync>> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    sync_agent_auth_inner(local_data_root, agent, home.as_deref())
}

fn sync_agent_auth_inner(
    local_data_root: &Path,
    agent: &str,
    system_home: Option<&Path>,
) -> std::io::Result<Option<zerocode_hookd::codex_runtime_auth::AuthSync>> {
    use zerocode_core::account::{Provider, providers_for};

    if !providers_for(agent).contains(&Provider::OpenAi) {
        return Ok(None);
    }
    let Some(home) = system_home else {
        return Ok(None);
    };
    let runtime = crate::codex_accounts::runtime_home(local_data_root);
    let record = crate::codex_accounts::read_runtime_record(&runtime);
    let store = crate::codex_accounts::read_store(local_data_root);
    let active_id = record.account.as_deref().or_else(|| {
        zerocode_core::codex_account::active_account(&store.accounts, &store.selection)
            .map(|a| a.id.as_str())
    });
    if let Some(account_id) = active_id {
        // A managed account is active: any refreshed token inside the runtime
        // home belongs to THAT account's managed storage, not ~/.codex.
        // Sync back to the account's home instead of touching ~/.codex.
        let runtime_auth = runtime.join(crate::codex_accounts::AUTH_FILE);
        if let Ok(curr) = std::fs::read_to_string(&runtime_auth)
            && record.written.as_deref() != Some(&curr)
            && crate::codex_accounts::signed_in_content(&curr)
            && let Some(managed_home) =
                crate::codex_accounts::account_home(local_data_root, account_id)
        {
            // The same direction rule the launch road keeps: only a provably
            // fresher stored copy stops this write. A rotating refresh token
            // has one live branch, and saving the older one over it is a
            // logout with a delay on it (t-5777).
            let account_auth = managed_home.join(crate::codex_accounts::AUTH_FILE);
            let held = std::fs::read_to_string(&account_auth).ok();
            if !held
                .as_deref()
                .is_some_and(|held| crate::codex_accounts::runtime_copy_wins(held, &curr))
            {
                let _ = std::fs::write(&account_auth, &curr);
            }
        }
        return Ok(Some(
            zerocode_hookd::codex_runtime_auth::AuthSync::Unchanged,
        ));
    }
    zerocode_hookd::codex_install::sync_auth(&codex_lane(home, local_data_root)).map(Some)
}

/// Every account directory's settings.json, alongside `~/.claude`'s own.
///
/// A launched claude reads hooks from the `CLAUDE_CONFIG_DIR` the account
/// picker handed it, so the installer has to reach every account. The
/// directories come from the STORE — each account records its `config_dir`
/// absolutely — not from globbing a well-known parent: the store is the same
/// authority the launcher reads, and the parent differs by platform (the
/// dirs are created under local data, the store under config; on macOS the
/// two roots merely happen to coincide). Recomputed at each call rather than
/// cached, so an account added a minute ago is covered by the very next
/// reconcile. A missing settings.json is created by the write.
fn claude_account_settings(config_root: &Path) -> Vec<PathBuf> {
    // The runtime home is where every Zerocode-launched Claude now reads its
    // settings. Keep the old account files in the reconcile set only so a
    // login or pre-migration directory is repaired consistently; they are no
    // longer launch homes.
    let mut files = vec![accounts::runtime_home(config_root).join("settings.json")];
    files.extend(
        accounts::read_store(config_root)
            .accounts
            .iter()
            .map(|account| PathBuf::from(&account.config_dir).join("settings.json")),
    );
    files.sort();
    files.dedup();
    files
}

/// What the settings screen shows, read without writing anything.
pub fn statuses(config_root: &Path, local_data_root: &Path) -> Vec<HookStatus> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let paths = InstallPaths::from_environment(home.clone())
        .with_claude_extras(claude_account_settings(config_root));
    MANAGED_TARGETS
        .iter()
        .map(|&agent| zerocode_hookd::install::status_of(&paths, agent))
        .chain(
            PLUGIN_TARGETS
                .iter()
                .map(|&agent| zerocode_hookd::plugin::status_of(&home, agent)),
        )
        .chain(std::iter::once(zerocode_hookd::codex_install::status_of(
            &codex_lane(&home, local_data_root),
        )))
        .collect()
}

// -------------------------------------------------------------- the envelope

/// What one hook event tells the window, already reduced to what it can draw.
#[derive(Debug, Clone, Serialize)]
pub struct PaneHookReport {
    pub term: u32,
    pub agent: AgentKind,
    /// `working`, `needs-attention` or `done` — the three facts a tab can
    /// wear. See `zerocode_core::hook_state`.
    pub state: zerocode_core::hook::HookState,
    pub event: String,
    /// A session boundary wearing `done`: claude's `SessionStart` on an
    /// idle/resumed pane. The row lands so a reopened session does not wear
    /// a phantom spinner, and this flag keeps every completion-reactive
    /// consumer (the ring, the roster's done gate) out of it — Orca's
    /// `sessionBoundary`, exactly.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub session_boundary: bool,
    /// A `done` a PERSON ended rather than the agent — Orca's `interrupted`.
    ///
    /// Sits beside `session_boundary` because the two are one idea wearing two
    /// names: both say a `done` is not a completion, and every consumer that
    /// reads one has a reason to read the other. Orca declares them as adjacent
    /// siblings (`agent-status-types.ts:138-139`) and reads them in one
    /// expression where it matters (`agent-finished-timestamp.ts:27`).
    ///
    /// They differ in ONE way, and it is why this is a second field rather than
    /// a shared one: a boundary is a fact about a single event, while an
    /// interrupt is carried across a turn — see
    /// [`zerocode_core::hook::interrupt_declared`]'s `carried`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// The permission mode the CLI reported with this event (Claude Code's
    /// hook input carries `permission_mode` on every event), for the
    /// composer's mode chip. Absent when the agent's hooks do not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// This event is a HELPER's, not the lead's — Orca's `eventAgentId`
    /// truthiness ([`zerocode_core::hook::helper_attributed`]).
    ///
    /// Belongs here and not with `interrupted` next door, even though both feed
    /// the same road: this is a fact about THIS ENVELOPE and nothing else,
    /// which is exactly this function's contract, while an interrupt needs the
    /// pane's memory to answer. The distinction is why one is read in
    /// [`report_of`] and the other in the window's event loop.
    ///
    /// What reads it: a helper's WAIT displaces the lead's own word, and the
    /// displaced word has to be kept before the row is rebuilt over it
    /// ([`zerocode_core::hook::wait_stash`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub child_attributed: bool,
    /// How much of this stop one keystroke could finish — the tool gate and
    /// the prompt's shape as one word
    /// ([`zerocode_core::ask::SubmitShape`]).
    ///
    /// Read on the envelope like the ask beside it, and for the same reason it
    /// is read HERE rather than at the keyboard: the payload is here. Orca
    /// re-parses its bounded JSON on every candidate keystroke; a word costs a
    /// comparison.
    #[serde(
        default,
        skip_serializing_if = "zerocode_core::ask::SubmitShape::is_closed"
    )]
    pub submit_shape: zerocode_core::ask::SubmitShape,
    /// The agent's OWN id for this conversation, when its payload carried one.
    ///
    /// What turns a pane from "a terminal that had an agent in it" into "this
    /// conversation": the id belongs to the vendor and outlives our process, so
    /// a tab that closed can be reopened onto the same session. Absent for the
    /// agents that publish none — see `zerocode_core::provider_session`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<zerocode_core::ProviderSession>,
    /// Whether that session can actually be reopened.
    ///
    /// Decided HERE rather than in the window, because the answer is a table of
    /// vendor flags and this side owns the table. Two agents report a session and
    /// offer no way back to it; a window left to assume would draw a "reopen" row
    /// that fails.
    pub resumable: bool,
    /// What the person asked, when this event is the asking — the
    /// `UserPromptSubmit` payload's own field, clamped for a card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// What the agent answered, when this event ends a turn — the payload's
    /// explicit field, or the tail of the transcript it names.
    ///
    /// `None` on a turn-ending event is a CLEAR, not an unknown (Orca's
    /// `clearLastAssistantMessage`, out/main/index.js:9857): a card showing
    /// the previous turn's answer under this turn's prompt is a card lying
    /// about which question was answered. The state field says which reading
    /// applies — `Done` with no words is the clear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// What the agent stopped to ask, when this event is the stopping — the
    /// question, the approval, or the notification's message.
    ///
    /// Valid for its one event only, which is Orca's rule verbatim ("don't
    /// inherit … carrying it forward leaves a stale live card",
    /// out/main/index.js:8840): the next working or done event means the
    /// question was answered, and a card still wearing it is asking for an
    /// answer nobody owes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
    /// The ask's full shape, when the stopping is an `AskUserQuestion` — the
    /// questions, their options, their multi-select flags. What lets a card
    /// offer the choices themselves rather than only their summary; parsed
    /// under the same one-event validity as `ask`, and by the same shape
    /// rules as Orca (`parseQuestionsShape`, index-ftls8Hg_.js:69040).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_prompt: Option<zerocode_core::ask::AskPrompt>,
    /// The tool waiting on permission, when the stopping is a
    /// `PermissionRequest` — that one event and no other, which is Orca's
    /// own condition (`deriveInteractivePrompt`, out/main/index.js:9035).
    /// What lets a card offer Allow and Deny rather than only the summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<zerocode_core::ask::ApprovalPrompt>,
    /// The model this event says the agent is on — the payload's own `model`
    /// field, per event and raw. Orca keeps a sticky copy in its lead state
    /// (index.js:10385); ours is kept by the WINDOW, which already keeps the
    /// last state per pane and cleans both at the same term death — one
    /// sticky map is one to clean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Turn an envelope into a report, or into nothing.
///
/// `expected_launch_token` is what this window handed the pane at launch, when
/// it launched one: an envelope carrying a DIFFERENT token is a straggler from
/// a previous occupant of the pane and is dropped. One carrying none is an
/// agent somebody started by hand inside a plain shell — real, and welcome.
/// One helper running inside a pane's agent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubagentRow {
    /// Which helper, as the vendor identifies it — used to find the row again
    /// when it stops.
    pub id: String,
    /// What to call it on screen.
    pub name: String,
    /// Still at work, or finished. A finished helper KEEPS its row: the
    /// person who ran five in parallel watched them vanish one by one as
    /// each finished — and with each, the page its transcript opens from
    /// ("subagent가 돌 때는 저 화면이 맞는데 나는 병렬로 … 지금"). The row
    /// stays, greyed, until the session turns or the pane leaves.
    pub state: SubagentState,
    /// Whether the `background_tasks` roll call has ever named this id.
    ///
    /// Provenance, not display — kept off the wire. A row the roll call
    /// claimed may be retired by a later COMPLETE roll call that omits it;
    /// a row born only from an explicit `SubagentStart` may not, because the
    /// list Claude sends simply does not carry foreground helpers, and
    /// deleting one for being unlisted would clear a row that is still real.
    #[serde(skip_serializing)]
    pub born_listed: bool,
    /// How many tools this helper has picked up so far.
    ///
    /// The one number Claude Code puts on a running helper's line ("12 tool
    /// uses"), and it travels as ONE field whoever counted it: zo counts its
    /// own helpers and says the number in its `subagents` frame, while a
    /// vendor that only fires hook events is counted here, from the tool
    /// calls filed under the helper's card. A window that had to know which
    /// vendor a row came from to read its count would be a second pipeline.
    ///
    /// Zero is "nothing yet", which is why it is never drawn: a helper that
    /// has not picked up a tool has no number to say.
    pub tool_calls: u64,
    /// The helper's own transcript on disk, when the vendor said where it is.
    ///
    /// zo names each running helper's session file in its `subagents` frame
    /// (`<store>/<id>.session.jsonl`); Claude Code names none, and its helpers
    /// are found under the pane's own transcript instead (`subagent_log`).
    /// Provenance for the helper's page, not display — kept off the wire.
    #[serde(skip_serializing)]
    pub transcript: Option<std::path::PathBuf>,
    /// Which ROSTER this row was born from, when the vendor names one — zo's
    /// session-owned registry (docs/design/zo-session-agent-registry.md
    /// §3), carried on its `subagents` frame as `registry`. A frame from one
    /// registry may not retire rows another registry listed: two zo sessions
    /// sharing a pane after a `/resume`, or a store that moved under the
    /// session, used to read as every helper finishing at once (Opus review
    /// §2-B, "false Done"). `None` is a vendor that names no registry, and
    /// keeps the one-roster behaviour it always had. Provenance, off the wire.
    #[serde(skip_serializing)]
    pub registry: Option<String>,
}

/// Whether a helper is still at work or has finished. Serialized in the
/// window's own words so a row is drawn from its state rather than from
/// whether it is still in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentState {
    #[default]
    Running,
    Done,
}

/// How many finished helpers one pane keeps. Past this the oldest finished
/// row goes — a person who ran a hundred helpers in one session is not
/// reading the first of them from the sidebar, and its transcript is still
/// on disk.
pub const MAX_DONE_HELPERS: usize = 32;

/// Mark a helper finished, keeping its name and transcript. Answers whether
/// that changed anything, so a repeated stop is not a repaint.
pub fn retire_helper(row: &mut SubagentRow) -> bool {
    if row.state == SubagentState::Done {
        return false;
    }
    row.state = SubagentState::Done;
    true
}

/// What a `SubagentStop` does to the row its `SubagentStart` opened.
///
/// For a vendor whose stop closes the SPAWN
/// (`AgentKind::subagent_stop_closes_the_spawn`) the row goes without a
/// trace and nothing is finished: the helper it announced either runs on as
/// the roster row the same stop's roll call named, or was already retired by
/// the road that tracks it by its own id. For every other vendor the stop
/// is the helper finishing — the row stays, greyed, with its page
/// ([`retire_helper`]), under the bound on finished rows. Answers whether
/// anything changed. A helper whose PANE ended takes the finishing road
/// too, whoever the vendor is: the pane was the helper.
pub fn close_helper_row(rows: &mut Vec<SubagentRow>, id: &str, closes_the_spawn: bool) -> bool {
    if closes_the_spawn {
        let before = rows.len();
        rows.retain(|row| row.id != id);
        return rows.len() != before;
    }
    let mut changed = rows
        .iter_mut()
        .find(|row| row.id == id)
        .is_some_and(retire_helper);
    changed |= bound_done_helpers(rows);
    changed
}

/// Drop finished rows past [`MAX_DONE_HELPERS`], oldest first — rows stand
/// in the order they arrived.
pub fn bound_done_helpers(rows: &mut Vec<SubagentRow>) -> bool {
    let mut done = rows
        .iter()
        .filter(|row| row.state == SubagentState::Done)
        .count();
    let before = rows.len();
    rows.retain(|row| {
        if row.state == SubagentState::Done && done > MAX_DONE_HELPERS {
            done -= 1;
            return false;
        }
        true
    });
    rows.len() != before
}

/// Is any helper in this roster still at work?
#[must_use]
pub fn any_helper_running(rows: &[SubagentRow]) -> bool {
    rows.iter().any(|row| row.state == SubagentState::Running)
}

/// Fold a vendor's WHOLE roster — zo's `subagents` frame names every helper
/// that is running, and only those — into the rows held for the pane. A
/// listed id is running (a new row, or the old one with its name and
/// transcript refreshed); a roster-born row the frame no longer lists has
/// finished and is kept as finished, transcript and all; a row born from a
/// lifecycle hook is not the frame's to retire. Answers whether anything
/// changed.
///
/// A frame that names no registry: every roster-born row is the frame's.
/// See [`fold_helper_roster_from`] for the frame that names one — the
/// production road, which reads the registry off the frame; this is the
/// tests' shorter spelling of the same fold.
#[cfg(test)]
pub fn fold_helper_roster(held: &mut Vec<SubagentRow>, fresh: Vec<SubagentRow>) -> bool {
    fold_helper_roster_from(held, fresh, None)
}

/// `fold_helper_roster`, from a frame that says WHICH roster it is the
/// whole of.
///
/// The rule the session-registry design asks of the window (§3, "거짓 Done
/// 차단"): a complete frame retires only the running rows born from the SAME
/// registry, and rows another registry listed are left exactly as they were
/// — a second zo session sharing the pane, or a store that moved under the
/// first, is not every helper finishing at once. A row that named no
/// registry (born before the vendor said one, or from a vendor that never
/// does) belongs to whichever frame comes, which is the behaviour it always
/// had. The rows the frame lists are stamped with its registry, so the next
/// frame knows them as its own.
pub fn fold_helper_roster_from(
    held: &mut Vec<SubagentRow>,
    fresh: Vec<SubagentRow>,
    registry: Option<&str>,
) -> bool {
    let mut changed = false;
    let listed: std::collections::HashSet<String> =
        fresh.iter().map(|row| row.id.clone()).collect();
    for row in held.iter_mut() {
        let this_frames = match (&row.registry, registry) {
            (Some(born), Some(frame)) => born == frame,
            _ => true,
        };
        if this_frames
            && row.born_listed
            && row.state == SubagentState::Running
            && !listed.contains(&row.id)
        {
            row.state = SubagentState::Done;
            changed = true;
        }
    }
    for mut row in fresh {
        row.state = SubagentState::Running;
        row.born_listed = true;
        if row.registry.is_none() {
            row.registry = registry.map(str::to_string);
        }
        if let Some(seat) = held.iter_mut().find(|one| one.id == row.id) {
            let refreshed = seat.state != SubagentState::Running
                || seat.name != row.name
                || (row.transcript.is_some() && seat.transcript != row.transcript)
                || row.tool_calls > seat.tool_calls;
            if refreshed {
                seat.state = SubagentState::Running;
                seat.name = row.name;
                seat.born_listed = true;
                if row.transcript.is_some() {
                    seat.transcript = row.transcript;
                }
                if row.registry.is_some() {
                    seat.registry = row.registry;
                }
                // The GREATER of the two, because a count only ever goes up
                // while a helper runs: a vendor that does not count yet says
                // zero on every frame, and a zero taken at its word would
                // erase what the tool road already counted.
                seat.tool_calls = seat.tool_calls.max(row.tool_calls);
                changed = true;
            }
        } else {
            held.push(row);
            changed = true;
        }
    }
    changed |= bound_done_helpers(held);
    changed
}

/// A subagent starting or stopping inside a pane, or `None` for every other
/// event.
///
/// The twin of [`report_of`], and separate for the reason the classifier is:
/// this event says nothing about what the PANE is doing, so folding it into
/// the pane report would either repaint the pane wrongly or be dropped —
/// which is exactly what used to happen to it.
pub fn subagent_of(
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) -> Option<(u32, zerocode_core::hook::SubagentStep, SubagentRow)> {
    let term = term_of_pane_key(&envelope.pane_key)?;
    // The same gate the pane report walks through: a report carrying somebody
    // else's launch token is a previous occupant of the pane.
    if let Some(expected) = expected_launch_token
        && !envelope.launch_token.is_empty()
        && envelope.launch_token != expected
    {
        return None;
    }
    let payload = zerocode_core::payload::HookPayload::of(&envelope.payload);
    let event = zerocode_core::hook::envelope_event_name_parsed(envelope, &payload)?;
    let step = zerocode_core::hook::subagent_step(&event)?;
    if ["subagent_type", "agent_type"].iter().any(|key| {
        payload
            .tree_or_null()
            .get(key)
            .and_then(serde_json::Value::as_str)
            == Some("classifier")
    }) {
        return None;
    }
    let (name, id) = zerocode_core::hook::subagent_in_parsed(&payload);
    // A helper with no name at all is still a helper that is running, and the
    // count is the useful part — but it needs SOME id or a stop can never
    // find it. Without either, there is nothing to draw and nothing to clear.
    let id = id?;
    Some((
        term,
        step,
        SubagentRow {
            name: name.unwrap_or_else(|| id.clone()),
            id,
            state: SubagentState::Running,
            born_listed: false,
            transcript: None,
            tool_calls: 0,
            registry: None,
        },
    ))
}

/// The `background_tasks` roll call one envelope carries, when it carries
/// one — behind the same pane-key and launch-token gates every other road
/// walks, because a stale occupant's roll call is as much a straggler as its
/// lifecycle events are.
pub fn background_tasks_of(
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) -> Option<(u32, zerocode_core::hook::BackgroundTasksReading)> {
    let term = term_of_pane_key(&envelope.pane_key)?;
    if let Some(expected) = expected_launch_token
        && !envelope.launch_token.is_empty()
        && envelope.launch_token != expected
    {
        return None;
    }
    let reading = zerocode_core::hook::background_agent_tasks_parsed(
        &zerocode_core::payload::HookPayload::of(&envelope.payload),
    )?;
    Some((term, reading))
}

/// Fold one roll call into a pane's helper roster, and say whether it moved.
///
/// The rules are Orca's (`foldClaudeBackgroundTasksIntoRoster`), carried in
/// three sentences: a running subagent-typed entry is a row (created, or
/// trusted onto the id-exact match it names); one reported ended takes its
/// row away whoever made it; and a tracked roll-call row the list does not
/// name goes only when the reading is NOT truncated, because a capped or
/// mangled list cannot prove absence. Teammate-typed entries are skipped —
/// their ids report running forever and never match a stop.
pub fn fold_background_tasks(
    running: &mut Vec<SubagentRow>,
    reading: &zerocode_core::hook::BackgroundTasksReading,
) -> bool {
    let mut changed = false;
    for task in &reading.tasks {
        if task.teammate {
            continue;
        }
        if task.running {
            let name = task
                .agent_type
                .clone()
                .or_else(|| task.description.clone())
                .unwrap_or_else(|| task.id.clone());
            if let Some(seat) = running.iter_mut().find(|one| one.id == task.id) {
                if seat.name != name {
                    seat.name = name;
                    changed = true;
                }
                if seat.state != SubagentState::Running {
                    seat.state = SubagentState::Running;
                    changed = true;
                }
                // The roll call has proven it tracks this id — a later
                // complete list may retire it by omission now.
                seat.born_listed = true;
            } else {
                running.push(SubagentRow {
                    id: task.id.clone(),
                    name,
                    state: SubagentState::Running,
                    born_listed: true,
                    transcript: None,
                    // A roll call names who is running, never what they have
                    // done — a fresh row starts at nothing and the tool road
                    // is what moves it.
                    tool_calls: 0,
                    registry: None,
                });
                changed = true;
            }
        } else if let Some(seat) = running.iter_mut().find(|one| one.id == task.id) {
            changed |= retire_helper(seat);
        }
    }
    if !reading.truncated {
        let named: std::collections::HashSet<&str> = reading
            .tasks
            .iter()
            .filter(|task| !task.teammate)
            .map(|task| task.id.as_str())
            .collect();
        for row in running.iter_mut() {
            if row.born_listed && !named.contains(row.id.as_str()) {
                changed |= retire_helper(row);
            }
        }
    }
    changed |= bound_done_helpers(running);
    changed
}

/// The name a card answers to, which is what an activity is filed under.
///
/// The window's own spelling for its board cards (`term:${pane.term}` and
/// `sub:${pane.term}:${sub.id}`, ui/shell.js), owned here for the reason
/// [`pane_key_of`] is owned here: two spellings of one identity is how a
/// stream ends up filed under a name no surface looks for. It is deliberately
/// NOT the bridge's `term-3` — that one names a PANE to the agent scripts,
/// and a helper running inside a pane has no pane of its own to name.
pub fn activity_pane(term: u32) -> String {
    format!("term:{term}")
}

pub fn activity_subagent(term: u32, id: &str) -> String {
    format!("sub:{term}:{id}")
}

/// And back: the helper a card name belongs to, or nothing when the name is a
/// pane's own.
///
/// The reverse of [`activity_subagent`] and spelled beside it for the reason
/// that one is spelled here — the road that FILES a helper's work under
/// `sub:<term>:<id>` and the road that reads the name back have to agree, and
/// two spellings of one identity is how a count lands on a row nobody is
/// looking at.
#[must_use]
pub fn helper_in_card(card: &str) -> Option<(u32, &str)> {
    let (term, id) = card.strip_prefix("sub:")?.split_once(':')?;
    Some((term.parse().ok()?, id))
}

/// Everything filed under one pane's helpers, for the road that forgets them.
pub fn activity_subagent_prefix(term: u32) -> String {
    format!("sub:{term}:")
}

/// What the pane's agent just DID, and which card it belongs under.
///
/// The third road off one envelope, beside [`report_of`] and [`subagent_of`].
/// The detail was arriving all along and being dropped where the state was
/// classified: `hook_state` reduces a plain `PreToolUse` to the word
/// `working` (a blocked question is the one exception it reads deeper for),
/// which is true of five agents at once and says nothing about any of them.
///
/// The card is chosen by the payload and not by the event: a helper running
/// inside an agent reports through its parent's pane key — it has no pane of
/// its own, which is the whole reason it is drawn as a row — so a tool call
/// that names a helper is filed under that helper's card and everything else
/// under the pane's. `subagent_in_payload` is the same reader the roster uses,
/// so the two roads cannot disagree about which helper an event belongs to.
pub fn activity_of(
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) -> Option<(String, zerocode_core::hook::Activity)> {
    let term = term_of_pane_key(&envelope.pane_key)?;
    // The same gate both other roads walk through: a report carrying somebody
    // else's launch token is a previous occupant of the pane.
    if let Some(expected) = expected_launch_token
        && !envelope.launch_token.is_empty()
        && envelope.launch_token != expected
    {
        return None;
    }
    let payload = zerocode_core::payload::HookPayload::of(&envelope.payload);
    let event = zerocode_core::hook::envelope_event_name_parsed(envelope, &payload)?;
    let activity = zerocode_core::hook::activity_of_parsed(&event, &payload)?;
    let (_, id) = zerocode_core::hook::subagent_in_parsed(&payload);
    let pane = id.map_or_else(|| activity_pane(term), |id| activity_subagent(term, &id));
    Some((pane, activity))
}

/// The pane whose agent says its session ended, or nothing — the fourth road
/// off one envelope, beside [`report_of`], [`subagent_of`] and
/// [`activity_of`], behind the same launch-token gate: a report carrying
/// somebody else's token is a previous occupant of the pane.
pub fn session_end_of(
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) -> Option<u32> {
    let term = term_of_pane_key(&envelope.pane_key)?;
    if let Some(expected) = expected_launch_token
        && !envelope.launch_token.is_empty()
        && envelope.launch_token != expected
    {
        return None;
    }
    let payload = zerocode_core::payload::HookPayload::of(&envelope.payload);
    let event = zerocode_core::hook::envelope_event_name_parsed(envelope, &payload)?;
    zerocode_core::hook::ends_the_session(&event).then_some(term)
}

/// Remove C0 controls from text bound for cards while retaining the two
/// layout characters the UI deliberately supports.
fn sanitize_card_text(text: String) -> String {
    text.chars()
        .filter(|character| {
            matches!(*character, '\n' | '\t') || u32::from(*character) > u32::from('\u{1f}')
        })
        .collect()
}

pub fn report_of(
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) -> Option<PaneHookReport> {
    let term = term_of_pane_key(&envelope.pane_key)?;
    if let Some(expected) = expected_launch_token
        && !envelope.launch_token.is_empty()
        && envelope.launch_token != expected
    {
        return None;
    }
    let payload = zerocode_core::payload::HookPayload::of(&envelope.payload);
    let event = zerocode_core::hook::envelope_event_name_parsed(envelope, &payload)?;
    // The boundary is asked FIRST because the state table cannot see it:
    // `SessionStart` says nothing in the table (every other vendor's is
    // dropped), and claude's — gated on source and attribution in core —
    // lands as an idle `done` row so a resumed pane does not spin.
    let session_boundary =
        zerocode_core::hook::session_boundary_parsed(envelope.agent, &event, &payload);
    let state = if session_boundary {
        zerocode_core::hook::HookState::Done
    } else {
        zerocode_core::hook::hook_state_parsed(&event, &payload)?
    };
    /* A turn a hook just CONTINUED did not end.
     *
     * The bridge answered this very `Stop` with the pane's waiting ledger
     * pointer, which tells the agent to go and read its mail rather than
     * stop. Reported as `Done`, the same event would light a finished card,
     * ring a completion, and hand the pointer road a pane it thinks is idle —
     * three claims about a turn that is still running. The mark is taken
     * rather than read: one continuation answers for one event.
     *
     * Only the state is corrected. What the agent said, the session, the
     * whole rest of the report stay exactly as the payload gave them. */
    let state = if state == zerocode_core::hook::HookState::Done
        && crate::orchestration_pointer_mailbox::turn_was_continued(
            term,
            crate::orchestration_pointer_mailbox::CONTINUATION_RECOGNITION,
        ) {
        zerocode_core::hook::HookState::Working
    } else {
        state
    };
    // Read here, from the same envelope, rather than in a second pass: the
    // session id is in the payload the event arrived in, and going back for it
    // later would mean keeping payloads around to go back to.
    let session = zerocode_core::provider_session::session_in_parsed(envelope.agent, &payload);
    // The card lines, by what kind of event this is. The prompt only rides
    // prompt events and the answer only rides turn-ending ones — asking every
    // payload for both would read `message` fields that mean other things.
    let asking = {
        let word: String = event
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        word == "userpromptsubmit" || word == "beforesubmitprompt"
    };
    let prompt = asking
        .then(|| zerocode_core::transcript::prompt_in_parsed(&payload))
        .flatten()
        .map(sanitize_card_text);
    // The answer line: a turn-ending event carries the whole answer, and a
    // tool landing mid-turn carries what it just did — Orca updates
    // `lastAssistantMessage` on both (`extractClaudeToolFields`,
    // out/main/index.js:9541), so a card is not silent for a whole turn.
    let said = if state == zerocode_core::hook::HookState::Done {
        zerocode_core::transcript::said_in_parsed(&payload)
    } else {
        let word: String = event
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        match word.as_str() {
            "posttooluse" => zerocode_core::transcript::tool_said_in_parsed(&payload),
            "posttoolusefailure" => zerocode_core::transcript::tool_failure_in_parsed(&payload),
            _ => None,
        }
    }
    .map(sanitize_card_text);
    let ask = (state == zerocode_core::hook::HookState::NeedsAttention)
        .then(|| zerocode_core::transcript::ask_in_parsed(&payload))
        .flatten();
    let ask_prompt = (state == zerocode_core::hook::HookState::NeedsAttention)
        .then(|| zerocode_core::ask::prompt_in_parsed(&payload))
        .flatten();
    // Same one-event life, same gate: a pane that is not stopped has nothing a
    // keystroke could finish, and the refusing word is the default.
    let submit_shape = if state == zerocode_core::hook::HookState::NeedsAttention {
        zerocode_core::ask::submit_shape_in_parsed(&payload)
    } else {
        zerocode_core::ask::SubmitShape::NotAQuestion
    };
    // Narrower than the ask: `Notification` also lands in NeedsAttention, and
    // a notification has nothing to allow — Orca builds the approval off the
    // `PermissionRequest` event alone (out/main/index.js:9035).
    let approving = {
        let word: String = event
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        word == "permissionrequest"
    };
    let approval = approving
        .then(|| zerocode_core::ask::approval_in_parsed(&payload))
        .flatten()
        // The plan a permission is asking approval FOR, filled here because
        // this is the line that knows WHICH agent asked: the reader is given
        // a payload, and which tool carries a plan is the catalog's fact
        // (`AgentVoice::plan_tool`). An agent the catalog does not voice
        // names no plan tool, so its permissions stay ordinary approvals.
        .map(|mut approval| {
            approval.plan = zerocode_core::ask::plan_in(
                payload.tree().and_then(|tree| tree.get("tool_input")),
                &approval.tool,
                zerocode_core::agent::agent_voice(envelope.agent.slug()).plan_tool,
            );
            approval
        });
    Some(PaneHookReport {
        permission_mode: payload
            .tree()
            .and_then(|tree| tree.get("permission_mode"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        term,
        agent: envelope.agent,
        state,
        session_boundary,
        // Declared elsewhere, and deliberately: an interrupt is CARRIED across
        // a turn, and the flag it is carried in lives with the pane's memory —
        // which this function does not have and should not grow, since its
        // whole contract is "what this envelope alone says". Orca draws the
        // same line: it computes `interrupted` inside the listener that holds
        // `claudeLeadStateByPaneKey`, not in the payload readers. Ours is
        // `declare_pane_interrupt` in the window's event loop.
        interrupted: false,
        // And this one IS the envelope's alone, so it is read right here.
        child_attributed: zerocode_core::hook::helper_attributed_parsed(&payload),
        submit_shape,
        resumable: session
            .as_ref()
            .is_some_and(|one| zerocode_core::resume_argv(envelope.agent, one).is_some()),
        session,
        event,
        prompt,
        said,
        ask,
        ask_prompt,
        approval,
        model: zerocode_core::hook::model_in_parsed(&payload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::HookEnvelope;

    /// Router keys reach zo and only zo: another agent's launch never even
    /// reads the keychain for them.
    #[test]
    fn router_keys_ride_a_zo_launch_and_no_other() {
        let key = || {
            vec![(
                "ZEROCODE_ROUTER_OPENROUTER_KEY".to_string(),
                "k".to_string(),
            )]
        };
        assert_eq!(router_env_for("zo", key), key());
        for agent in ["claude", "codex", "gemini"] {
            let read = std::cell::Cell::new(false);
            let env = router_env_for(agent, || {
                read.set(true);
                key()
            });
            assert!(
                env.is_empty() && !read.get(),
                "{agent} was handed a router key"
            );
        }
    }

    /// A zo pane is an OpenAI consumer, so it gets the same mirror `CODEX_HOME`
    /// (and the same launch lock) a Codex pane gets. Before the provider table
    /// this door answered `agent == "codex"` and handed zo nothing, which is
    /// how a zo pane ended up speaking as whatever `~/.codex` held.
    #[test]
    fn a_zo_launch_is_handed_the_selected_codex_account_mirror() {
        let root = tempfile::tempdir().expect("local data root");
        let system_home = tempfile::tempdir().expect("system home");
        let managed = crate::codex_accounts::account_home(root.path(), "account-a")
            .expect("managed account home");
        std::fs::create_dir_all(&managed).expect("managed home");
        std::fs::write(
            managed.join(crate::codex_accounts::AUTH_FILE),
            r#"{"OPENAI_API_KEY":"selected"}"#,
        )
        .expect("managed auth");
        std::fs::write(
            root.path().join(crate::codex_accounts::ACCOUNT_STORE_FILE),
            serde_json::json!({
                "accounts": [{ "id": "account-a", "home_dir": managed, "added_at": 0 }],
                "active": "account-a"
            })
            .to_string(),
        )
        .expect("account store");

        let (zo, zo_lock) =
            agent_launch_env_with_lock_inner(root.path(), "zo", Some(system_home.path()));
        assert!(
            zo_lock.is_some(),
            "a zo launch took the mirror without its launch lock"
        );
        // Released before the second call: that lock is the mirror's, one
        // holder at a time, and a launch that kept it would sit on the next
        // one forever.
        drop(zo_lock);
        let (codex, codex_lock) =
            agent_launch_env_with_lock_inner(root.path(), "codex", Some(system_home.path()));
        drop(codex_lock);

        let home_of = |env: &[(String, String)]| {
            env.iter()
                .find(|(key, _)| key == "CODEX_HOME")
                .map(|(_, value)| value.clone())
        };
        let mirror = home_of(&zo).expect("zo received no CODEX_HOME");
        assert_eq!(
            Some(mirror),
            home_of(&codex),
            "a zo pane and a codex pane were pointed at different accounts"
        );
    }

    /// Agents this window holds no OpenAI credentials for are left alone —
    /// naming one would be a control that lies, and it is the same table that
    /// says so.
    #[test]
    fn an_agent_with_no_openai_lane_is_handed_nothing() {
        let root = tempfile::tempdir().expect("local data root");
        let system_home = tempfile::tempdir().expect("system home");

        for agent in ["claude", "opencode", ""] {
            let (env, lock) =
                agent_launch_env_with_lock_inner(root.path(), agent, Some(system_home.path()));
            assert!(env.is_empty(), "{agent} was handed {env:?}");
            assert!(lock.is_none(), "{agent} took the mirror launch lock");
        }
    }

    /// No `HOME` is not a reason to guess one. The lane the fallback would
    /// build is rooted there, and a lane rooted at nothing writes into the
    /// wrong place.
    #[test]
    fn a_launch_without_a_home_is_handed_nothing() {
        let root = tempfile::tempdir().expect("local data root");

        let (env, lock) = agent_launch_env_with_lock_inner(root.path(), "zo", None);

        assert!(env.is_empty(), "{env:?}");
        assert!(lock.is_none());
    }

    /// The exit door refuses the same agents the launch door refuses — one
    /// table, read from both ends, so a pane can never be launched under a
    /// managed account whose rotation nobody carries home.
    #[test]
    fn an_agent_with_no_openai_lane_syncs_nothing_on_exit() {
        let root = tempfile::tempdir().expect("local data root");
        let system_home = tempfile::tempdir().expect("system home");

        for agent in ["claude", "opencode"] {
            let synced = sync_agent_auth_inner(root.path(), agent, Some(system_home.path()))
                .expect("best-effort sync");
            assert!(synced.is_none(), "{agent} reached the codex auth sync");
        }
    }

    /// …and refuses the write when the account's own home holds the newer
    /// login. The pane's copy is not automatically the live branch: a
    /// `codex login` in another home rotates the refresh token too, and the
    /// older file landing last is the logout the person reports as "it expires
    /// again" (t-5777).
    #[test]
    fn a_zo_exit_never_overwrites_a_fresher_account_login() {
        let root = tempfile::tempdir().expect("local data root");
        let system_home = tempfile::tempdir().expect("system home");
        let managed = crate::codex_accounts::account_home(root.path(), "account-a")
            .expect("managed account home");
        std::fs::create_dir_all(&managed).expect("managed home");
        let newer = crate::codex_accounts::auth_fixture(
            "00000000-0000-0000-0000-00000000000a",
            "2026-09-21T06:32:33Z",
            "at-new",
        );
        let older = crate::codex_accounts::auth_fixture(
            "00000000-0000-0000-0000-00000000000a",
            "2026-09-17T02:48:26Z",
            "at-old",
        );
        std::fs::write(managed.join(crate::codex_accounts::AUTH_FILE), &newer)
            .expect("the account refreshed elsewhere");
        std::fs::write(
            root.path().join(crate::codex_accounts::ACCOUNT_STORE_FILE),
            serde_json::json!({
                "accounts": [{ "id": "account-a", "home_dir": managed, "added_at": 0 }],
                "active": "account-a"
            })
            .to_string(),
        )
        .expect("account store");
        let runtime = crate::codex_accounts::runtime_home(root.path());
        std::fs::create_dir_all(&runtime).expect("runtime home");
        crate::codex_accounts::write_runtime_record(
            &runtime,
            &crate::codex_accounts::CodexRuntimeAuth {
                version: 1,
                account: Some("account-a".to_string()),
                written: Some("{}".to_string()),
            },
        );
        std::fs::write(runtime.join(crate::codex_accounts::AUTH_FILE), &older)
            .expect("a stale runtime copy");

        sync_agent_auth_inner(root.path(), "zo", Some(system_home.path()))
            .expect("best-effort sync");

        assert_eq!(
            std::fs::read_to_string(managed.join(crate::codex_accounts::AUTH_FILE))
                .expect("the account's login"),
            newer,
            "the exit road wrote an older copy over the account's newer login"
        );
    }

    #[test]
    fn a_zo_exit_returns_a_rotated_codex_token_to_its_managed_account() {
        let root = tempfile::tempdir().expect("local data root");
        let system_home = tempfile::tempdir().expect("system home");
        let managed = crate::codex_accounts::account_home(root.path(), "account-a")
            .expect("managed account home");
        std::fs::create_dir_all(&managed).expect("managed home");
        std::fs::write(
            managed.join(crate::codex_accounts::AUTH_FILE),
            r#"{"OPENAI_API_KEY":"before"}"#,
        )
        .expect("managed auth");
        std::fs::write(
            root.path().join(crate::codex_accounts::ACCOUNT_STORE_FILE),
            serde_json::json!({
                "accounts": [{
                    "id": "account-a",
                    "home_dir": managed,
                    "added_at": 0
                }],
                "active": "account-a"
            })
            .to_string(),
        )
        .expect("account store");
        let runtime = crate::codex_accounts::runtime_home(root.path());
        std::fs::create_dir_all(&runtime).expect("runtime home");
        crate::codex_accounts::write_runtime_record(
            &runtime,
            &crate::codex_accounts::CodexRuntimeAuth {
                version: 1,
                account: Some("account-a".to_string()),
                written: Some(r#"{"OPENAI_API_KEY":"before"}"#.to_string()),
            },
        );
        std::fs::write(
            runtime.join(crate::codex_accounts::AUTH_FILE),
            r#"{"OPENAI_API_KEY":"after"}"#,
        )
        .expect("rotated runtime auth");

        let synced = sync_agent_auth_inner(root.path(), "zo", Some(system_home.path()))
            .expect("best-effort sync");

        assert!(synced.is_some(), "zo was not treated as an OpenAI consumer");
        assert_eq!(
            std::fs::read_to_string(managed.join(crate::codex_accounts::AUTH_FILE))
                .expect("synced managed auth"),
            r#"{"OPENAI_API_KEY":"after"}"#
        );
    }

    /// Which lane agents have no installed hook transport, named rather than
    /// assumed.
    ///
    /// Zo deliberately stays on this list: its pane event channel reports the
    /// same turn lifecycle without installing a mirror hook. OpenCode has no
    /// measured lifecycle transport and remains genuinely unobserved.
    ///
    /// Pinned as a list, not asserted to be empty: adding a hook event a
    /// vendor never calls buys a quietly dead hook and nothing else, so
    /// widening the roads is a measurement about that vendor, not a tidy-up.
    /// What this test refuses is silent growth — a new agent landing in
    /// `ALL_AGENTS` without an installed hook or an explicit alternative
    /// transport and nobody noticing.
    #[test]
    fn every_lane_agent_without_an_installed_hook_is_named() {
        let reports = |agent: zerocode_core::AgentKind| {
            MANAGED_TARGETS.contains(&agent)
                || PLUGIN_TARGETS.contains(&agent)
                // Codex keeps its own installer beside the managed table.
                || agent == zerocode_core::AgentKind::Codex
        };
        let mute: Vec<&str> = zerocode_core::agent::ALL_AGENTS
            .into_iter()
            .filter(|&agent| !reports(agent))
            .map(zerocode_core::AgentKind::slug)
            .collect();
        assert_eq!(
            mute,
            ["zo", "opencode"],
            "the set of agents that can never be measured at rest moved"
        );
    }

    #[test]
    fn a_seeded_selection_marker_is_unique_and_only_follows_the_contract() {
        let key = zerocode_hookd::env_var::SELECTION_SEEDED;
        let mut env = vec![(key.to_string(), "stale".to_string())];
        mark_selection_seeded(&mut env, "ordinary prompt");
        assert!(!env.iter().any(|(name, _)| name == key));

        mark_selection_seeded(&mut env, zerocode_core::delegation::AGENT_SELECTION_CONTEXT);
        mark_selection_seeded(&mut env, zerocode_core::delegation::AGENT_SELECTION_CONTEXT);
        assert_eq!(
            env.iter().filter(|(name, _)| name == key).count(),
            1,
            "the launch exported two conflicting markers"
        );
    }

    #[test]
    fn tokens_are_twenty_four_os_bytes_in_lowercase_hex() {
        let token = random_token().expect("the operating system has no random source");

        assert_eq!(token.len(), 48);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "token was not fixed lowercase hex"
        );
    }

    #[test]
    fn independently_minted_tokens_do_not_repeat() {
        let first = random_token().expect("the operating system has no random source");
        let second = random_token().expect("the operating system has no random source");

        assert_ne!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn a_generated_shim_reads_private_tokens_only_when_it_runs() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("shim dir");
        let endpoint = zerocode_hookd::endpoint::write_endpoint_file(
            dir.path(),
            &zerocode_hookd::endpoint::EndpointFields {
                port: 1,
                token: "hook-secret".to_string(),
                env: "production".to_string(),
                version: zerocode_hookd::HOOK_CONTRACT_VERSION.to_string(),
            },
        )
        .expect("endpoint");
        let browser = zerocode_hookd::endpoint::write_private_token_file(
            dir.path(),
            zerocode_hookd::endpoint::BROWSER_TOKEN_FILE,
            "browser-secret",
        )
        .expect("browser token");
        let script = dir.path().join("loader.sh");
        let body = shim_script_with_private_tokens(
            "#!/bin/sh\nprintf '%s/%s' \"$ZEROCODE_HOOK_TOKEN\" \"$ZEROCODE_BROWSER_TOKEN\"\n"
                .to_string(),
            &[(
                zerocode_hookd::env_var::BROWSER_TOKEN,
                zerocode_hookd::env_var::BROWSER_TOKEN_FILE,
            )],
        );
        std::fs::write(&script, body).expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("script mode");
        let output = crate::proc::quiet_command("/bin/sh")
            .arg(&script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(zerocode_hookd::env_var::ENDPOINT, &endpoint)
            .env(zerocode_hookd::env_var::BROWSER_TOKEN_FILE, browser)
            .output()
            .expect("run loader");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"hook-secret/browser-secret");
    }

    #[test]
    fn settings_and_endpoint_use_their_injected_roots() {
        let config = tempfile::tempdir().expect("no config dir");
        let local_data = tempfile::tempdir().expect("no local data dir");

        assert_eq!(
            settings_file(config.path()),
            config.path().join(SETTINGS_FILE_NAME)
        );
        assert_eq!(
            endpoint_dir(local_data.path()),
            local_data.path().join(ENDPOINT_DIR_NAME)
        );
    }

    #[test]
    fn delivery_failure_scan_names_only_marker_directories_for_terminal_panes() {
        let dir = tempfile::tempdir().expect("failure dir");
        std::fs::create_dir(dir.path().join("term-7")).expect("term marker");
        std::fs::create_dir(dir.path().join("somebody-else")).expect("foreign marker");
        std::fs::write(dir.path().join("term-8"), b"not a marker directory").expect("plain file");

        let found = delivery_failures_in(dir.path(), i64::MAX);

        assert_eq!(found.keys().copied().collect::<Vec<_>>(), [7]);
    }

    /// A marker the window cannot read is not a pane that failed NOW.
    ///
    /// The naming and the dating are two syscalls, and a successful delivery
    /// can erase the marker between them. Dating it `now` hands the actor a
    /// failure that never happened, in the one moment the pane is provably
    /// healthy — and the ledger keeps that stamp until the pane's next turn
    /// boundary. The same refusal covers a whole directory the window has
    /// lost access to, which would otherwise read as every pane in it failing
    /// at once.
    #[cfg(unix)]
    #[test]
    fn a_marker_whose_time_cannot_be_read_is_not_a_failure_now() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("failure dir");
        std::fs::create_dir(dir.path().join(pane_key_of(7))).expect("marker");
        // Readable but not searchable: `read_dir` still names the entry and
        // every `stat` beneath it is refused — a vanished marker's shape,
        // deterministically.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o400))
            .expect("close the door");
        let refused = std::fs::metadata(dir.path().join(pane_key_of(7))).is_err();

        let found = delivery_failures_in(dir.path(), 5_000);

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("open it again so the tempdir can be swept");
        // A machine that stats anyway (root) has nothing to answer here.
        if refused {
            assert!(
                found.is_empty(),
                "an unreadable marker was dated NOW and became a failure nobody had: {found:?}"
            );
        }
    }

    /// The script's unlink is its last act and it does not always run — an
    /// agent killed at the end of a turn leaves the marker standing over a
    /// payload the window is holding. Erasing only the cache would let the
    /// next beat read the same stale word off disk, every second, until that
    /// pane happened to POST successfully again.
    #[test]
    fn a_delivered_payload_takes_the_marker_back_off_disk_too() {
        let dir = tempfile::tempdir().expect("failure dir");
        std::fs::create_dir(dir.path().join(pane_key_of(7))).expect("standing marker");
        std::fs::create_dir(dir.path().join(pane_key_of(8))).expect("another pane's marker");

        forget_delivery_failure_in(dir.path(), 7);

        assert_eq!(
            delivery_failures_in(dir.path(), i64::MAX)
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [8],
            "the beat would have read the erased pane as unreachable again"
        );
    }

    /// Re-askable cost of the one filesystem read added to the existing beat.
    /// No wall-clock assertion: a shared runner's speed is not correctness.
    ///
    /// `cargo test -p zerocode-shell --bin zerocode-shell -- --ignored \
    /// --nocapture measure_delivery_failure_scan`
    #[test]
    #[ignore = "measurement, not a rule"]
    fn measure_delivery_failure_scan() {
        let absent_parent = tempfile::tempdir().expect("absent failure parent");
        let absent = absent_parent.path().join("not-created");
        let empty = tempfile::tempdir().expect("empty failure dir");
        let marked = tempfile::tempdir().expect("marked failure dir");
        for term in 1..=64 {
            std::fs::create_dir(marked.path().join(pane_key_of(term))).expect("marker");
        }
        let rounds = 2_000u32;
        let measure = |dir: &Path| {
            let started = std::time::Instant::now();
            let mut rows = 0usize;
            for _ in 0..rounds {
                rows += std::hint::black_box(delivery_failures_in(dir, i64::MAX)).len();
            }
            (started.elapsed() / rounds, rows / rounds as usize)
        };
        let (absent_cost, absent_rows) = measure(&absent);
        let (empty_cost, empty_rows) = measure(empty.path());
        let (marked_cost, marked_rows) = measure(marked.path());
        println!(
            "delivery failure scan: absent={absent_cost:?} ({absent_rows} rows), \
             empty={empty_cost:?} ({empty_rows} rows), \
             64-markers={marked_cost:?} ({marked_rows} rows)"
        );
    }

    /// The one bridge these tests share.
    ///
    /// `start` binds ONCE per process — `BRIDGE` is a `OnceLock` — so a second
    /// test that starts its own is not a second bridge, it is a test that
    /// fails whenever it loses the race. Both callers come through here
    /// instead, and the first one through pays for it.
    fn bridge_for_tests() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
        ONCE.call_once(|| {
            let root = ROOT.get_or_init(|| tempfile::tempdir().expect("no local data dir"));
            start(root.path()).expect("the test bridge starts");
        });
        assert!(bridge().is_some(), "the test bridge is not up");
    }

    /// The vault reaches every pane, agent or not — and stops when it is
    /// unset.
    ///
    /// A pane is where an agent lives, and until this landed the only thing
    /// that named the vault was a file inside the vault, so a pane opened in
    /// any other project had no way to hear of it. The "not carried when
    /// cleared" half is the other promise: a person who takes the setting
    /// away must not keep finding the variable in new terminals.
    #[test]
    fn every_pane_is_told_where_the_second_brain_is_and_only_while_there_is_one() {
        bridge_for_tests();
        let vault = "/Users/fixture/Knowledge";
        let carried = |env: &[(String, String)]| {
            env.iter()
                .find(|(name, _)| name == zerocode_hookd::env_var::SECOND_BRAIN)
                .map(|(_, value)| value.clone())
        };

        publish_second_brain_vault(vault);
        assert_eq!(
            carried(&pty_env("term-plain", None, Path::new("/workspace"), None)).as_deref(),
            Some(vault),
            "a plain shell was not told where the second brain is"
        );
        assert_eq!(
            carried(&pty_env(
                "term-agent",
                Some("launch-nonce"),
                Path::new("/workspace"),
                None,
            ))
            .as_deref(),
            Some(vault)
        );

        // A relative path is no vault at all: each pane has its own working
        // directory, so it would name a different folder in every one of them.
        publish_second_brain_vault("Knowledge");
        assert_eq!(second_brain_vault(), None);

        // Cleared: the next pane hears nothing, rather than an empty value it
        // would have to know to ignore.
        publish_second_brain_vault(vault);
        publish_second_brain_vault("   ");
        assert_eq!(
            carried(&pty_env("term-plain", None, Path::new("/workspace"), None)),
            None,
            "the vault variable outlived the setting"
        );
        assert_eq!(second_brain_vault(), None);
    }

    #[test]
    fn a_plain_shell_clears_the_inherited_hook_secret_when_endpoint_exists() {
        bridge_for_tests();
        let env = pty_env("term-plain", None, Path::new("/workspace"), None);
        let hook = env
            .iter()
            .find(|(name, _)| name == zerocode_hookd::env_var::TOKEN)
            .expect("the child must explicitly clear an inherited token");
        assert!(
            hook.1.is_empty(),
            "the bridge token was still copied into a plain shell: {:?}",
            hook.1
        );
        assert!(
            env.iter()
                .any(|(name, _)| { name == zerocode_hookd::env_var::ENDPOINT })
        );
        let measurable = pty_env("term-7", None, Path::new("/workspace"), None);
        assert!(measurable.iter().any(|(name, value)| {
            name == zerocode_hookd::env_var::DELIVERY_FAILURE_MARKER && value.ends_with("/term-7")
        }));
        for name in [
            zerocode_hookd::env_var::BROWSER_TOKEN,
            zerocode_hookd::env_var::COMPUTER_TOKEN,
            zerocode_hookd::env_var::BROWSER_TOKEN_FILE,
            zerocode_hookd::env_var::COMPUTER_TOKEN_FILE,
            zerocode_core::agent_teams::TEAM_TOKEN_VAR,
            zerocode_hookd::env_var::TEAM_TOKEN_FILE,
            zerocode_hookd::env_var::LAUNCH_TOKEN,
            zerocode_hookd::env_var::CLAUDE_MESSAGING_TOKEN,
        ] {
            assert!(
                env.iter()
                    .filter(|(entry, _)| entry == name)
                    .all(|(_, value)| value.is_empty()),
                "{name} still reaches a plain shell"
            );
        }

        let launched = pty_env(
            "term-agent",
            Some("launch-nonce"),
            Path::new("/workspace"),
            None,
        );
        for name in [
            zerocode_hookd::env_var::TOKEN,
            zerocode_hookd::env_var::BROWSER_TOKEN,
            zerocode_hookd::env_var::COMPUTER_TOKEN,
            zerocode_core::agent_teams::TEAM_TOKEN_VAR,
            zerocode_hookd::env_var::CLAUDE_MESSAGING_TOKEN,
        ] {
            assert!(
                launched
                    .iter()
                    .filter(|(entry, _)| entry == name)
                    .all(|(_, value)| value.is_empty()),
                "{name} still has a secret value in the launch environment"
            );
        }
        assert!(
            launched
                .iter()
                .any(|(name, _)| { name == zerocode_hookd::env_var::BROWSER_TOKEN_FILE })
        );
        assert!(
            launched
                .iter()
                .any(|(name, _)| { name == zerocode_hookd::env_var::COMPUTER_TOKEN_FILE })
        );
    }

    #[test]
    fn hook_preference_round_trips_only_through_the_injected_root() {
        let config = tempfile::tempdir().expect("no config dir");
        assert!(hooks_enabled(config.path()));
        set_hooks_enabled(config.path(), false).expect("write preference");
        assert!(!hooks_enabled(config.path()));
        assert!(config.path().join(SETTINGS_FILE_NAME).is_file());
    }

    #[test]
    fn codex_lane_keeps_the_agent_home_external_and_app_data_injected() {
        let external_home = tempfile::tempdir().expect("no external home");
        let local_data = tempfile::tempdir().expect("no local data dir");
        let lane = codex_lane(external_home.path(), local_data.path());

        assert_eq!(lane.home, external_home.path());
        assert_eq!(lane.user_data, local_data.path());
        assert_eq!(lane.state_dir, local_data.path());
    }

    fn envelope(pane_key: &str, launch_token: &str, payload: &str) -> HookEnvelope {
        HookEnvelope {
            agent: AgentKind::Claude,
            pane_key: pane_key.into(),
            tab_id: String::new(),
            launch_token: launch_token.into(),
            worktree_id: String::new(),
            env: String::new(),
            version: "1".into(),
            hook_event_name: String::new(),
            payload: payload.into(),
        }
    }

    /// A pane's `SessionEnd` names the pane (t-6336) — behind the launch-token
    /// gate every road off an envelope takes — and nothing else does: a turn's
    /// `Stop` ends a turn, not the session that borrowed a device.
    #[test]
    fn a_session_ending_names_its_pane_and_only_its_own() {
        const TERM: u32 = 9_312;
        let key = crate::hooks::pane_key_of(TERM);
        let ended = envelope(
            &key,
            "tok-1",
            r#"{"hook_event_name":"SessionEnd","reason":"exit"}"#,
        );
        assert_eq!(session_end_of(&ended, Some("tok-1")), Some(TERM));
        assert_eq!(session_end_of(&ended, None), Some(TERM));
        assert_eq!(
            session_end_of(&ended, Some("tok-2")),
            None,
            "a previous occupant's goodbye ended this pane's session"
        );
        let stopped = envelope(&key, "tok-1", r#"{"hook_event_name":"Stop"}"#);
        assert_eq!(session_end_of(&stopped, Some("tok-1")), None);
        let nameless = envelope("tab-1/leaf-2", "", r#"{"hook_event_name":"SessionEnd"}"#);
        assert_eq!(session_end_of(&nameless, None), None);
    }

    /// A `Stop` the bridge answered with a continuation is not a turn that
    /// ended.
    ///
    /// Reported as `Done`, that same event would light a finished card, ring a
    /// completion, and hand the pointer pass a pane it believes is idle —
    /// three claims about a turn the hook has just told the model to carry on
    /// with. The mark is TAKEN, so one continuation answers for one event and
    /// the next `Stop` is a real one.
    #[test]
    fn a_stop_a_hook_continued_is_reported_as_work_and_only_once() {
        const TERM: u32 = 9_311;
        let stop = envelope(
            &crate::hooks::pane_key_of(TERM),
            "",
            r#"{"hook_event_name":"Stop","session_id":"s-1"}"#,
        );

        // Without a continuation it is exactly what it has always been.
        crate::orchestration_pointer_mailbox::forget_term(TERM);
        assert_eq!(
            report_of(&stop, None).expect("a row").state,
            zerocode_core::hook::HookState::Done
        );

        // The bridge takes this pane's pointer, which is what marks the turn
        // as continued — and it does so before the envelope ever reaches this
        // road, which is the whole reason the mark can be here to read.
        assert!(crate::orchestration_pointer_mailbox::park(
            TERM,
            "run-1",
            "run:run-1",
            "m-1",
            None,
            zerocode_hookd::session_notify::PointerNotice::new("run-1", "run:run-1", "m-1", 1)
                .expect("a pointer"),
        ));
        assert!(
            zerocode_hookd::pointer_mailbox::PointerMailbox::take(
                &*crate::orchestration_pointer_mailbox::mailbox(),
                &crate::hooks::pane_key_of(TERM),
                "",
                zerocode_hookd::pointer_mailbox::PointerMoment::TurnEnding,
            )
            .is_some()
        );
        assert_eq!(
            report_of(&stop, None).expect("a row").state,
            zerocode_core::hook::HookState::Working,
            "a turn a hook told the model to carry on with was announced as finished"
        );

        // Once. The next Stop is a real one.
        assert_eq!(
            report_of(&stop, None).expect("a row").state,
            zerocode_core::hook::HookState::Done,
            "one continuation answered for two events"
        );
        crate::orchestration_pointer_mailbox::forget_term(TERM);
    }

    /// A resumed session lands as an idle done row, flagged as the boundary
    /// it is — and only the landing shapes Orca allows land at all.
    #[test]
    fn a_session_start_lands_as_a_flagged_boundary_or_not_at_all() {
        let resumed = envelope(
            "term-7",
            "",
            r#"{"hook_event_name":"SessionStart","source":"resume"}"#,
        );
        let report = report_of(&resumed, None).expect("a boundary row");
        assert_eq!(report.state, zerocode_core::hook::HookState::Done);
        assert!(report.session_boundary);
        // A compact restart fires mid-turn and must not flip a live pane.
        let compacted = envelope(
            "term-7",
            "",
            r#"{"hook_event_name":"SessionStart","source":"compact"}"#,
        );
        assert!(report_of(&compacted, None).is_none());
        // And an ordinary Stop is a completion, not a boundary.
        let stopped = envelope("term-7", "", r#"{"hook_event_name":"Stop"}"#);
        let report = report_of(&stopped, None).expect("a stop row");
        assert_eq!(report.state, zerocode_core::hook::HookState::Done);
        assert!(!report.session_boundary);
    }

    /// 도우미 귀속은 **봉투 하나만으로** 답할 수 있는 사실이라 여기서
    /// 읽힌다 — 옆자리 `interrupted`가 판의 기억을 필요로 해서 창으로
    /// 미뤄지는 것과 대비되는 자리다.
    ///
    /// 판이 이 깃발로 무엇을 하는지는 `hook::wait_stash`가 쥔다. 이 시험이
    /// 지키는 것은 그 앞의 한 걸음, **깃발이 실제로 서는가**이다.
    #[test]
    fn a_helpers_envelope_says_so_and_the_leads_does_not() {
        let helper = r#"{"hook_event_name":"PermissionRequest","agent_id":"a-1"}"#;
        let report = report_of(&envelope("term-3", "", helper), None).expect("a report");
        assert_eq!(report.state, zerocode_core::hook::HookState::NeedsAttention);
        assert!(report.child_attributed);
        // 같은 사건, 리드의 것.
        let lead = r#"{"hook_event_name":"PermissionRequest"}"#;
        let report = report_of(&envelope("term-3", "", lead), None).expect("a report");
        assert_eq!(report.state, zerocode_core::hook::HookState::NeedsAttention);
        assert!(!report.child_attributed);
        // 그리고 인터럽트는 여기서 서지 않는다 — 봉투가 답할 수 없으므로.
        assert!(!report.interrupted);
    }

    /// 판이 멈춘 자리마다 「키 하나로 끝낼 수 있는가」가 함께 실린다 —
    /// 그리고 멈추지 않은 판에는 거절하는 낱말만 실린다.
    ///
    /// 이 낱말이 도구 이름 관문을 **품고 있다**는 것이 요점이다. 모양만
    /// 물었으면 입력을 안 실은 권한 요청이 Enter에 열렸을 것이다.
    #[test]
    fn a_stop_carries_what_one_key_could_finish_and_a_running_pane_carries_nothing() {
        let asking = concat!(
            r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","#,
            r#""tool_input":{"questions":[{"question":"Which?","#,
            r#""options":[{"label":"A"},{"label":"B"}]}]}}"#
        );
        let report = report_of(&envelope("term-3", "", asking), None).expect("a report");
        assert_eq!(report.state, zerocode_core::hook::HookState::NeedsAttention);
        assert_eq!(
            report.submit_shape,
            zerocode_core::ask::SubmitShape::SingleSelect(2)
        );
        // 권한 요청은 같은 amber 자리에 서고도 아무 키로도 끝나지 않는다.
        let permission = concat!(
            r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","#,
            r#""tool_input":{"command":"rm -rf build"}}"#
        );
        let report = report_of(&envelope("term-3", "", permission), None).expect("a report");
        assert_eq!(report.state, zerocode_core::hook::HookState::NeedsAttention);
        assert_eq!(
            report.submit_shape,
            zerocode_core::ask::SubmitShape::NotAQuestion
        );
        // 그리고 멈추지 않은 판에는 끝낼 것이 없다 — 이 낱말은 정지의 것이다.
        let stop = r#"{"hook_event_name":"Stop"}"#;
        let report = report_of(&envelope("term-3", "", stop), None).expect("a report");
        assert_eq!(
            report.submit_shape,
            zerocode_core::ask::SubmitShape::NotAQuestion
        );
    }

    /// 계획을 묻는 권한 요청은 계획을 싣고 온다 — 그 도구가 이 CLI의 계획
    /// 도구일 때에만. 도구 이름은 카탈로그의 사실이고(`AgentVoice::plan_tool`),
    /// 읽는 이는 페이로드만 받으므로 이 줄이 둘을 잇는 유일한 자리다.
    #[test]
    fn a_permission_on_the_agents_plan_tool_carries_its_plan() {
        let planning = concat!(
            r#"{"hook_event_name":"PermissionRequest","tool_name":"ExitPlanMode","#,
            r#""tool_input":{"plan":"1. read ask.rs 2. stand the card"}}"#
        );
        let report = report_of(&envelope("term-3", "", planning), None).expect("a report");
        assert_eq!(
            report
                .approval
                .as_ref()
                .and_then(|approval| approval.plan.as_deref()),
            Some("1. read ask.rs 2. stand the card")
        );
        // 다른 도구의 입력은 계획이 아니다 — 무엇을 싣고 있든.
        let ordinary = concat!(
            r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","#,
            r#""tool_input":{"command":"ls","plan":"1. read ask.rs"}}"#
        );
        let report = report_of(&envelope("term-3", "", ordinary), None).expect("a report");
        assert_eq!(report.approval.expect("an approval").plan, None);
    }

    /// The model rides the report when the payload says one, and stays off
    /// it when the payload says something else — the stickiness is the
    /// window's, not this reader's.
    #[test]
    fn the_model_rides_the_report_raw() {
        let saying = r#"{"hook_event_name":"Stop","model":"gpt-5.6-sol"}"#;
        let report = report_of(&envelope("term-3", "", saying), None).expect("a report");
        assert_eq!(report.model.as_deref(), Some("gpt-5.6-sol"));
        let silent = r#"{"hook_event_name":"Stop"}"#;
        let report = report_of(&envelope("term-3", "", silent), None).expect("a report");
        assert_eq!(report.model, None);
    }

    /// Hook text is display data. C0 editing bytes may be present in vendor
    /// transcripts, but they must not become boxes on the board or sidebar.
    #[test]
    fn hook_card_text_drops_c0_controls_except_newline_and_tab() {
        assert_eq!(
            sanitize_card_text("\u{15}You\n\tare\u{b}".into()),
            "You\n\tare"
        );
        let prompt = concat!(
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"\u0015\u0015"#,
            r#"You are\n\ta worker\u000b"}"#
        );
        let report = report_of(&envelope("term-3", "", prompt), None).expect("a report");
        assert_eq!(report.prompt.as_deref(), Some("You are a worker"));
    }

    #[test]
    fn a_stale_launch_token_is_dropped_and_a_handmade_agent_is_not() {
        let stop = r#"{"hook_event_name":"Stop"}"#;
        // The pane was relaunched; the old occupant's report arrives late.
        assert!(report_of(&envelope("term-3", "lt-old", stop), Some("lt-new")).is_none());
        // The live occupant.
        let live = report_of(&envelope("term-3", "lt-new", stop), Some("lt-new"))
            .expect("the live token was refused");
        assert_eq!(live.term, 3);
        assert_eq!(live.state, zerocode_core::hook::HookState::Done);
        // An agent started by hand carries no token and still counts.
        assert!(report_of(&envelope("term-3", "", stop), Some("lt-new")).is_some());
        // And a pane key this window never minted goes nowhere.
        assert!(report_of(&envelope("pane/9", "", stop), None).is_none());
    }

    #[test]
    fn an_event_that_says_nothing_about_state_reports_nothing() {
        let quiet = r#"{"hook_event_name":"SubagentStart"}"#;
        assert!(report_of(&envelope("term-1", "", quiet), None).is_none());
    }

    /// The roll call walks the same gates the lifecycle road does, and the
    /// fold holds its three sentences: running entries are rows, ended ones
    /// go whoever made them, and omission retires only roll-call-born rows —
    /// and only when the list was whole enough to prove absence.
    #[test]
    fn a_roll_call_folds_into_the_roster_without_clearing_what_it_cannot_see() {
        let listed = r#"{"hook_event_name":"Stop","background_tasks":[
            {"id":"bg-1","type":"subagent","agent_type":"Explore","status":"running"},
            {"id":"tm-1","type":"teammate","agent_type":"lead"}
        ]}"#;
        let (term, reading) =
            background_tasks_of(&envelope("term-6", "", listed), None).expect("present");
        assert_eq!(term, 6);
        // A stale occupant's roll call is a straggler like its events are.
        assert!(
            background_tasks_of(&envelope("term-6", "lt-old", listed), Some("lt-new")).is_none()
        );
        // And a payload without the field keeps the roster untouched.
        assert!(
            background_tasks_of(
                &envelope("term-6", "", r#"{"hook_event_name":"Stop"}"#),
                None
            )
            .is_none()
        );

        // A row the lifecycle made, and the roll call's own row beside it.
        let mut running = vec![SubagentRow {
            id: "fg-1".to_string(),
            name: "arch".to_string(),
            state: SubagentState::Running,
            born_listed: false,
            transcript: None,
            tool_calls: 4,
            registry: None,
        }];
        assert!(fold_background_tasks(&mut running, &reading));
        assert_eq!(running.len(), 2, "the teammate entry is not a helper row");
        assert_eq!(running[1].id, "bg-1");
        assert_eq!(running[1].name, "Explore");
        assert!(running[1].born_listed);
        assert!(
            !running[0].born_listed,
            "a foreground row keeps its provenance through a fold"
        );
        assert_eq!(
            (running[0].tool_calls, running[1].tool_calls),
            (4, 0),
            "a roll call names who is running, not what they have done"
        );

        // A complete list that no longer names bg-1 retires it — the row
        // stays, finished, with its page still reachable — and leaves the
        // foreground row running, because the list never carries those.
        let gone = zerocode_core::hook::background_agent_tasks(r#"{"background_tasks":[]}"#)
            .expect("present");
        assert!(fold_background_tasks(&mut running, &gone));
        assert_eq!(running.len(), 2);
        assert_eq!(running[0].id, "fg-1");
        assert_eq!(running[0].state, SubagentState::Running);
        assert_eq!(running[1].id, "bg-1");
        assert_eq!(running[1].state, SubagentState::Done);
        assert!(
            !fold_background_tasks(&mut running, &gone),
            "retiring twice changes nothing"
        );
        running.remove(1);

        // A truncated list proves nothing: the roll-call row it fails to name
        // stays. (Unnamed entry ⇒ truncated, by the core reader's contract.)
        running.push(SubagentRow {
            id: "bg-2".to_string(),
            name: "Explore".to_string(),
            state: SubagentState::Running,
            born_listed: true,
            transcript: None,
            tool_calls: 0,
            registry: None,
        });
        let mangled = zerocode_core::hook::background_agent_tasks(
            r#"{"background_tasks":[{"type":"subagent"}]}"#,
        )
        .expect("present");
        assert!(mangled.truncated);
        assert!(!fold_background_tasks(&mut running, &mangled));
        assert_eq!(running.len(), 2, "absence unproven, nothing retired");

        // An ended entry takes its row whoever made it — lifecycle-born too.
        let ended = zerocode_core::hook::background_agent_tasks(
            r#"{"background_tasks":[{"id":"fg-1","type":"subagent","status":"completed"}]}"#,
        )
        .expect("present");
        assert!(fold_background_tasks(&mut running, &ended));
        let fg = running
            .iter()
            .find(|one| one.id == "fg-1")
            .expect("the row stays");
        assert_eq!(fg.state, SubagentState::Done, "ended is finished, not gone");
    }

    /// zo's `SubagentStop` closes the SPAWN, not the helper: it fires when
    /// the spawning tool call returns, which for a background `Agent` is
    /// milliseconds after its start while the child runs on in a pane of its
    /// own, and the child's real life travels by its agent id — the roll
    /// call the same stop carries. Retiring the spawn's row there drew a
    /// finished helper the moment one was summoned (「완료 1개」 beside a
    /// running pane, 2026-09-07 23:20, t-3098): two rows for one summons,
    /// one of them a lie. The row goes and nothing is finished. A vendor
    /// whose stop IS the helper finishing keeps today's greyed row and page.
    #[test]
    fn a_stop_that_closes_the_spawn_takes_its_row_and_finishes_nothing() {
        let spawn = |id: &str| SubagentRow {
            id: id.to_string(),
            name: "stage-art · redesign the stage".to_string(),
            state: SubagentState::Running,
            born_listed: false,
            transcript: None,
            tool_calls: 0,
            registry: None,
        };
        let mut running = vec![spawn("call-1")];
        // The roll call the stop carries names the child by ITS id, running.
        let reading = zerocode_core::hook::background_agent_tasks(
            r#"{"background_tasks":[{"id":"agent-1","type":"subagent",
                "agent_type":"stage-art","status":"running","pane":"%2"}]}"#,
        )
        .expect("present");
        assert!(fold_background_tasks(&mut running, &reading));
        assert!(close_helper_row(
            &mut running,
            "call-1",
            AgentKind::Zo.subagent_stop_closes_the_spawn(),
        ));
        assert_eq!(
            running
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["agent-1"],
            "the spawn's row stood on as a finished helper, or the child went with it"
        );
        assert_eq!(running[0].state, SubagentState::Running);
        assert!(
            !close_helper_row(&mut running, "call-1", true),
            "a row already gone changes nothing"
        );
        // Claude's pair brackets the helper itself: its stop is a finish, and
        // the finished row stays with its page.
        let mut foreground = vec![spawn("a-1")];
        assert!(close_helper_row(
            &mut foreground,
            "a-1",
            AgentKind::Claude.subagent_stop_closes_the_spawn(),
        ));
        assert_eq!(foreground.len(), 1);
        assert_eq!(foreground[0].state, SubagentState::Done);
        assert!(
            !close_helper_row(&mut foreground, "a-1", false),
            "finishing twice changes nothing"
        );
        // And the finishing road keeps the bound on finished rows.
        let mut crowded: Vec<SubagentRow> = (0..=MAX_DONE_HELPERS)
            .map(|at| {
                let mut row = spawn(&format!("old-{at}"));
                row.state = SubagentState::Done;
                row
            })
            .collect();
        crowded.push(spawn("last"));
        assert!(close_helper_row(&mut crowded, "last", false));
        assert_eq!(crowded.len(), MAX_DONE_HELPERS);
        assert_eq!(crowded.last().map(|row| row.id.as_str()), Some("last"));
    }

    /// A frame is the whole of ITS registry and nobody else's. Two zo sessions
    /// sharing a pane after a `/resume`, or a store that moved under the
    /// first: the frame that lists only its own helpers used to fold the other
    /// session's running rows to Done at once — the false completion the
    /// registry design's Opus review named (§2-B). Rows born before any
    /// registry was named belong to whichever frame comes, as before; and a
    /// stale frame is the caller's to drop, not this fold's (see
    /// `shell_runtime::zo_frame_is_stale`).
    #[test]
    fn a_frame_from_one_registry_never_retires_another_registrys_rows() {
        let row = |id: &str, registry: Option<&str>| SubagentRow {
            id: id.to_string(),
            name: id.to_uppercase(),
            state: SubagentState::Running,
            born_listed: true,
            transcript: None,
            tool_calls: 0,
            registry: registry.map(str::to_string),
        };
        let mut held = Vec::new();
        // Session A lists a1 and a2; session B lists b1 — three rows, all up.
        assert!(fold_helper_roster_from(
            &mut held,
            vec![row("a1", None), row("a2", None)],
            Some("A")
        ));
        assert!(fold_helper_roster_from(
            &mut held,
            vec![row("b1", None)],
            Some("B")
        ));
        assert_eq!(held.len(), 3);
        assert!(held.iter().all(|one| one.state == SubagentState::Running));
        assert_eq!(
            held.iter()
                .find(|one| one.id == "a1")
                .unwrap()
                .registry
                .as_deref(),
            Some("A")
        );
        assert_eq!(
            held.iter()
                .find(|one| one.id == "b1")
                .unwrap()
                .registry
                .as_deref(),
            Some("B")
        );

        // A finishes a1: only a1 folds. b1 is B's, and A's frame is not the
        // whole of B.
        assert!(fold_helper_roster_from(
            &mut held,
            vec![row("a2", None)],
            Some("A")
        ));
        let state =
            |held: &[SubagentRow], id: &str| held.iter().find(|one| one.id == id).unwrap().state;
        assert_eq!(state(&held, "a1"), SubagentState::Done);
        assert_eq!(state(&held, "a2"), SubagentState::Running);
        assert_eq!(
            state(&held, "b1"),
            SubagentState::Running,
            "A's frame retired B's helper"
        );

        // B's empty frame: b1 finishes, a2 still runs.
        assert!(fold_helper_roster_from(&mut held, Vec::new(), Some("B")));
        assert_eq!(state(&held, "b1"), SubagentState::Done);
        assert_eq!(state(&held, "a2"), SubagentState::Running);

        // A row born before any registry was named belongs to the next frame
        // that comes, whichever registry it names — the old behaviour.
        let mut legacy = vec![row("old", None)];
        assert!(fold_helper_roster_from(&mut legacy, Vec::new(), Some("A")));
        assert_eq!(state(&legacy, "old"), SubagentState::Done);
        // And a frame naming no registry keeps its one-roster meaning.
        let mut mixed = vec![row("x", Some("A")), row("y", Some("B"))];
        assert!(fold_helper_roster(&mut mixed, Vec::new()));
        assert!(mixed.iter().all(|one| one.state == SubagentState::Done));
    }

    /// zo's frame names what is running and only that. Folding it keeps a
    /// helper it stopped naming as a finished row — name and transcript
    /// intact, so its page still opens — brings a re-listed id back to
    /// running, leaves a hook-born row alone, and bounds the finished ones.
    #[test]
    fn a_vendor_roster_retires_what_it_stops_naming_and_keeps_its_page() {
        let row = |id: &str, born_listed: bool, transcript: Option<&str>| SubagentRow {
            id: id.to_string(),
            name: id.to_uppercase(),
            state: SubagentState::Running,
            born_listed,
            transcript: transcript.map(std::path::PathBuf::from),
            tool_calls: 0,
            registry: None,
        };
        let mut held = vec![row("hook-born", false, None)];
        assert!(fold_helper_roster(
            &mut held,
            vec![
                row("a", true, Some("/store/a.jsonl")),
                row("b", true, Some("/store/b.jsonl"))
            ]
        ));
        assert_eq!(held.len(), 3);

        // `a` finished: the frame names `b` alone.
        assert!(fold_helper_roster(
            &mut held,
            vec![row("b", true, Some("/store/b.jsonl"))]
        ));
        let a = held.iter().find(|one| one.id == "a").expect("a stays");
        assert_eq!(a.state, SubagentState::Done);
        assert_eq!(
            a.transcript.as_deref(),
            Some(std::path::Path::new("/store/a.jsonl"))
        );
        assert_eq!(
            held.iter().find(|one| one.id == "b").expect("b").state,
            SubagentState::Running
        );
        assert_eq!(
            held.iter()
                .find(|one| one.id == "hook-born")
                .expect("hook row")
                .state,
            SubagentState::Running,
            "a row the frame never carried is not the frame's to retire"
        );
        assert!(
            !fold_helper_roster(&mut held, vec![row("b", true, Some("/store/b.jsonl"))]),
            "the same frame again changes nothing"
        );
        assert!(any_helper_running(&held));

        // The count the frame carries lands on the row, and a later frame
        // that counts nothing does not take it away — the number only ever
        // goes up while a helper runs, and a vendor that has not learned to
        // count yet says zero on every frame it sends.
        let mut counting = row("b", true, None);
        counting.tool_calls = 12;
        assert!(fold_helper_roster(&mut held, vec![counting]));
        let count =
            |held: &[SubagentRow]| held.iter().find(|one| one.id == "b").expect("b").tool_calls;
        assert_eq!(count(&held), 12);
        assert!(!fold_helper_roster(&mut held, vec![row("b", true, None)]));
        assert_eq!(count(&held), 12, "a zero erased what was counted");

        // Everything finished: the empty frame.
        assert!(fold_helper_roster(&mut held, Vec::new()));
        assert_eq!(
            held.iter()
                .filter(|one| one.state == SubagentState::Done)
                .count(),
            2
        );
        assert!(any_helper_running(&held), "the hook-born row still runs");

        // A finished id listed again is running again.
        assert!(fold_helper_roster(&mut held, vec![row("a", true, None)]));
        let a = held.iter().find(|one| one.id == "a").expect("a");
        assert_eq!(a.state, SubagentState::Running);
        assert_eq!(
            a.transcript.as_deref(),
            Some(std::path::Path::new("/store/a.jsonl")),
            "a frame without the path keeps the one known"
        );

        // The finished rows are bounded, oldest first.
        let mut many: Vec<SubagentRow> = (0..MAX_DONE_HELPERS + 5)
            .map(|at| {
                let mut one = row(&format!("h-{at}"), true, None);
                one.state = SubagentState::Done;
                one
            })
            .collect();
        many.push(row("live", true, None));
        assert!(bound_done_helpers(&mut many));
        assert_eq!(many.len(), MAX_DONE_HELPERS + 1);
        assert_eq!(many[0].id, "h-5", "the oldest finished rows went");
        assert_eq!(many.last().expect("live").id, "live");
    }

    /// A card name and the helper it belongs to are one identity, spelled
    /// once each way: what files a tool call and what reads the count back
    /// have to agree, and a pane's own card is never a helper's.
    #[test]
    fn a_helper_card_name_reads_back_as_the_helper_that_owns_it() {
        assert_eq!(
            helper_in_card(&activity_subagent(7, "a-1")),
            Some((7, "a-1"))
        );
        // An id with the separator in it keeps all of itself: the term is the
        // first field, and the rest is the id whatever it holds.
        assert_eq!(
            helper_in_card(&activity_subagent(7, "run:codex:2")),
            Some((7, "run:codex:2"))
        );
        assert_eq!(helper_in_card(&activity_pane(7)), None);
        assert_eq!(helper_in_card("sub:not-a-term:a-1"), None);
        assert_eq!(helper_in_card("sub:7"), None);
    }

    #[test]
    fn classification_hooks_do_not_create_user_facing_workers() {
        for key in ["subagent_type", "agent_type"] {
            let payload = serde_json::json!({"hook_event_name":"SubagentStart", "subagent_id":"internal", key:"classifier"}).to_string();
            assert!(subagent_of(&envelope("term-4", "", &payload), None).is_none());
        }
    }

    #[test]
    fn a_helper_starting_and_stopping_is_read_off_the_event_the_pane_report_drops() {
        // The exact pair the pane report answers `None` for, above. Both roads
        // read the SAME envelope — that is the whole design: the pane's state
        // and who is running underneath it are two questions, and folding them
        // into one is what made this event reach nobody.
        let start =
            r#"{"hook_event_name":"SubagentStart","subagent_type":"arch","subagent_id":"a-1"}"#;
        let (term, step, row) =
            subagent_of(&envelope("term-4", "", start), None).expect("a start was refused");
        assert_eq!(term, 4);
        assert_eq!(step, zerocode_core::hook::SubagentStep::Start);
        assert_eq!(row.id, "a-1");
        assert_eq!(row.name, "arch");
        let stop = r#"{"hook_event_name":"SubagentStop","subagent_id":"a-1"}"#;
        let (_, step, row) =
            subagent_of(&envelope("term-4", "", stop), None).expect("a stop was refused");
        assert_eq!(step, zerocode_core::hook::SubagentStep::Stop);
        // The id is what a stop is matched on, so a stop that names only the id
        // must still carry it — and names itself after it when it has nothing
        // better, rather than drawing a blank row.
        assert_eq!(row.id, "a-1");
        assert_eq!(row.name, "a-1");
        // Everything else is not this road's business, including the events
        // that DO move the pane.
        assert!(
            subagent_of(
                &envelope("term-4", "", r#"{"hook_event_name":"Stop"}"#),
                None
            )
            .is_none()
        );
        // And the same stale-token gate the pane report walks through: a helper
        // of the pane's previous occupant is not this pane's helper.
        assert!(subagent_of(&envelope("term-4", "lt-old", start), Some("lt-new")).is_none());
    }

    /// The tool stream is read off the same envelope, and filed under the card
    /// the work is being done in.
    #[test]
    fn a_tool_call_is_filed_under_the_pane_or_under_the_helper_that_made_it() {
        let call = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_input":{"command":"cargo test --workspace"}}"#;
        let (pane, activity) =
            activity_of(&envelope("term-4", "", call), None).expect("a tool call was refused");
        assert_eq!(pane, "term:4");
        assert_eq!(activity.verb, zerocode_core::hook::Tool::Bash);
        assert_eq!(activity.target.as_deref(), Some("cargo test --workspace"));

        // A helper's tool call arrives on its PARENT's pane key — it has no
        // pane of its own — so the payload is what says which card it belongs
        // under, read by the same function the roster reads.
        let helper = r#"{"hook_event_name":"PostToolUse","subagent_id":"a-1","tool_name":"Read",
            "tool_input":{"file_path":"/repo/src/main.rs"}}"#;
        let (pane, activity) = activity_of(&envelope("term-4", "", helper), None)
            .expect("a helper's call was refused");
        assert_eq!(pane, "sub:4:a-1");
        assert_eq!(activity.phase, zerocode_core::hook::Phase::Finished);

        // The events the other two roads own say nothing here, and neither
        // does a pane key this window never minted.
        assert!(
            activity_of(
                &envelope("term-4", "", r#"{"hook_event_name":"SubagentStart"}"#),
                None
            )
            .is_none()
        );
        assert!(activity_of(&envelope("pane/9", "", call), None).is_none());
        // And the stale-token gate, which every road off an envelope walks
        // through: a previous occupant's tool call is not this agent's.
        assert!(activity_of(&envelope("term-4", "lt-old", call), Some("lt-new")).is_none());
        assert!(activity_of(&envelope("term-4", "", call), Some("lt-new")).is_some());
    }

    #[test]
    fn pane_keys_round_trip() {
        assert_eq!(term_of_pane_key(&pane_key_of(41)), Some(41));
        assert_eq!(term_of_pane_key("term-x"), None);
        assert_eq!(term_of_pane_key("41"), None);
    }

    /// A bundle replaced under a running window takes the mirror with it, and
    /// the shim must lose the mirror rather than the agent.
    ///
    /// Run for real, both ways: these shims sit at the head of every pane's
    /// PATH, and when the mirror path went stale `/bin/sh` answered every one
    /// of them with 126 — every new agent terminal died in five milliseconds
    /// with nothing on screen to say why.
    #[cfg(unix)]
    #[test]
    fn a_missing_mirror_costs_the_mirror_and_not_the_agent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zc-shim-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mirror = dir.join("zerocode-mirror");
        let real = dir.join("real-claude");
        for (path, body) in [
            (&mirror, "#!/bin/sh\necho mirrored\n"),
            (&real, "#!/bin/sh\necho the agent itself\n"),
        ] {
            std::fs::write(path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let shims = dir.join("shims");
        write_shim_dir(&shims, &mirror, &[("claude".to_string(), real.clone())]).unwrap();

        let run = || {
            let out = crate::proc::quiet_command(shims.join("claude"))
                .output()
                .expect("the shim runs");
            (
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            )
        };
        assert_eq!(run(), (Some(0), "mirrored".to_string()));
        // The bundle goes away under the running window.
        std::fs::remove_file(&mirror).unwrap();
        assert_eq!(
            run(),
            (Some(0), "the agent itself".to_string()),
            "a stale mirror path took the terminal with it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One shim per resolved agent, each an exec through the mirror —
    /// executable, private to the user, and naming BOTH absolute paths, so a
    /// PATH that changes later cannot re-aim an already-written shim.
    #[test]
    fn a_shim_is_a_two_line_exec_wearing_both_absolute_paths() {
        let dir = std::env::temp_dir().join(format!("zc-shim-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mirror = dir.join("zerocode-mirror");
        let real = dir.join("real-codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&mirror, "").unwrap();
        std::fs::write(&real, "").unwrap();
        write_shim_dir(
            &dir.join("shims"),
            &mirror,
            &[("codex".to_string(), real.clone())],
        )
        .unwrap();
        let body = std::fs::read_to_string(dir.join("shims/codex")).unwrap();
        assert!(body.starts_with("#!/bin/sh\n"), "{body}");
        assert!(
            body.contains(&format!(
                "exec \"{}\" \"{}\" \"$@\"",
                mirror.display(),
                real.display()
            )),
            "{body}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("shims/codex"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755, "the shim is not executable");
            let dir_mode = std::fs::metadata(dir.join("shims"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(dir_mode & 0o777, 0o700, "the shim dir leaks to other users");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_mirror_wrapper_loads_tokens_only_for_the_mirror_child() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("shim dir");
        let endpoint = zerocode_hookd::endpoint::write_endpoint_file(
            dir.path(),
            &zerocode_hookd::endpoint::EndpointFields {
                port: 1,
                token: "hook-secret".to_string(),
                env: "production".to_string(),
                version: zerocode_hookd::HOOK_CONTRACT_VERSION.to_string(),
            },
        )
        .expect("endpoint");
        let team_token = zerocode_hookd::endpoint::write_private_token_file(
            dir.path(),
            zerocode_hookd::endpoint::BROWSER_TOKEN_FILE,
            "team-secret",
        )
        .expect("team token");
        assert!(
            std::fs::read_to_string(&endpoint)
                .expect("endpoint contents")
                .contains("ZEROCODE_HOOK_TOKEN=hook-secret")
        );
        let mirror = dir.path().join("mirror");
        let real = dir.path().join("real");
        for (path, body) in [
            (
                &mirror,
                "#!/bin/sh\nprintf '%s/%s' \"$ZEROCODE_HOOK_TOKEN\" \"$ZEROCODE_AGENT_TEAM_TOKEN\"\n",
            ),
            (
                &real,
                "#!/bin/sh\nprintf '%s' \"${ZEROCODE_HOOK_TOKEN:-unset}\"\n",
            ),
        ] {
            std::fs::write(path, body).expect("write child");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("child mode");
        }
        let shims = dir.path().join("shims");
        write_shim_dir(&shims, &mirror, &[("fake-agent".to_string(), real.clone())])
            .expect("write wrapper");
        let shim = shims.join("fake-agent");
        let output = crate::proc::quiet_command(&shim)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(zerocode_hookd::env_var::ENDPOINT, &endpoint)
            .env(zerocode_hookd::env_var::TEAM_TOKEN_FILE, team_token.clone())
            .output()
            .expect("mirror child");
        assert!(output.status.success());
        assert_eq!(
            output.stdout,
            b"hook-secret/team-secret",
            "wrapper did not load (stderr={}): {}",
            String::from_utf8_lossy(&output.stderr),
            std::fs::read_to_string(&shim).expect("wrapper")
        );

        std::fs::remove_file(&mirror).expect("remove mirror");
        let output = crate::proc::quiet_command(&shim)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(zerocode_hookd::env_var::ENDPOINT, &endpoint)
            .env(zerocode_hookd::env_var::TEAM_TOKEN_FILE, team_token)
            .output()
            .expect("fallback child");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"unset");
    }

    /// Every door an agent has out of its pane is laid by this one function,
    /// and a door that is merely WRITTEN somewhere else is a door nobody can
    /// open: the dir is the head of the pane's PATH and nothing else is.
    #[test]
    fn every_agent_door_is_laid_in_the_pane_path_and_is_executable() {
        let dir = std::env::temp_dir().join(format!("zc-doors-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mirror = dir.join("zerocode-mirror");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&mirror, "").unwrap();
        write_shim_dir(&dir.join("shims"), &mirror, &[]).unwrap();
        for door in [
            "zerocode-browser",
            "zerocode-computer",
            "zerocode-emulator",
            "zerocode-ssh",
        ] {
            let path = dir.join("shims").join(door);
            let body =
                std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{door} is missing"));
            assert!(body.starts_with("#!/usr/bin/env sh\n"), "{door}: {body}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&path).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o755, "{door} is not executable");
            }
        }
        // The remote door speaks its own route, or the window would dispatch
        // it to the desktop accessibility provider.
        let ssh = std::fs::read_to_string(dir.join("shims/zerocode-ssh")).unwrap();
        assert!(
            ssh.contains(&format!(
                "body=\"ssh{}\"",
                zerocode_core::agent_teams::ARGV_SEPARATOR
            )),
            "{ssh}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mirror discovery must wait for the same shell PATH that agent
    /// detection uses.  A Finder-launched window starts with launchd's
    /// minimal PATH; reading only the non-blocking cache lets a first pane
    /// resolve `agy` from its leader's shell while this one-time shim scan
    /// misses it forever.
    #[test]
    fn mirror_discovery_hydrates_the_shell_path_before_scanning() {
        let source = include_str!("hooks.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(production, _)| production);
        let resolver = production
            .split_once("fn resolvable_agents()")
            .map(|(_, resolver)| resolver)
            .expect("the mirror resolver");
        assert!(
            resolver.contains("shell_path::hydrate(false)"),
            "mirror discovery can freeze before the user's shell PATH arrives:\n{resolver}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn mirror_discovery_uses_the_catalogue_binary_name_and_shared_path_check() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp");
        let command = AgentKind::Antigravity.command();
        let binary = dir.path().join(command);
        std::fs::write(&binary, b"#!/bin/sh\n").expect("write");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let found = resolvable_agents_in(Some(dir.path().as_os_str()));
        assert_eq!(
            found
                .iter()
                .find(|(name, _)| name == command)
                .map(|(_, path)| path),
            Some(&binary),
            "the shim resolver did not find the catalogue's executable"
        );
    }

    /// An agent typed into a plain shell is recognised by its hooks and granted
    /// the capability paths by file: the grant names the files, single quoted
    /// for the spaces under Application Support, and the shim loader reads it
    /// only when its environment carries none — and only for a pane key of
    /// the window's own spelling.
    #[test]
    fn a_hand_started_agents_pane_is_granted_by_file_and_the_shim_reads_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("Application Support");
        std::fs::create_dir_all(&root).expect("root");
        let token_file = root.join("computer-token");
        std::fs::write(&token_file, "secret-1234\n").expect("token");
        let grant = write_pane_grant(
            &root,
            7,
            &[(
                zerocode_hookd::env_var::COMPUTER_TOKEN_FILE,
                token_file.as_path(),
            )],
        )
        .expect("grant");
        assert_eq!(grant, root.join("panes").join("term-7.env"));
        assert_eq!(
            std::fs::read_to_string(&grant).expect("read"),
            format!("ZEROCODE_COMPUTER_TOKEN_FILE='{}'\n", token_file.display())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&grant)
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the grant is private");
        }

        // The loader, run by /bin/sh the way a shim runs it: an environment
        // with the endpoint and the pane key but no capability path ends up
        // with the token; another pane's key, or a key of another spelling,
        // gets nothing; a path already in the environment is kept.
        let endpoint = root.join("endpoint.env");
        std::fs::write(&endpoint, "ZEROCODE_HOOK_PORT=1\nZEROCODE_HOOK_TOKEN=h\n")
            .expect("endpoint");
        let script = shim_script_with_private_tokens(
            "#!/bin/sh\nprintf '%s' \"${ZEROCODE_COMPUTER_TOKEN:-none}\"\n".to_string(),
            &[(
                zerocode_hookd::env_var::COMPUTER_TOKEN,
                zerocode_hookd::env_var::COMPUTER_TOKEN_FILE,
            )],
        );
        let run = |pane_key: &str, file_var: Option<&Path>| {
            let mut command = crate::proc::quiet_command("/bin/sh");
            command
                .arg("-c")
                .arg(&script)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("ZEROCODE_HOOK_ENDPOINT", &endpoint)
                .env("ZEROCODE_PANE_KEY", pane_key);
            if let Some(file) = file_var {
                command.env("ZEROCODE_COMPUTER_TOKEN_FILE", file);
            }
            let out = command.output().expect("sh");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        assert_eq!(run("term-7", None), "secret-1234");
        assert_eq!(
            run("term-8", None),
            "none",
            "another pane's grant is not this pane's"
        );
        assert_eq!(
            run("../term-7", None),
            "none",
            "a key that is not the window's spelling"
        );
        let own = root.join("own-token");
        std::fs::write(&own, "launched\n").expect("own token");
        assert_eq!(
            run("term-7", Some(&own)),
            "launched",
            "the launch environment wins"
        );

        // A Windows window hands the same path spelled with backslashes; the
        // loader reads the grant through it all the same.
        let backslashed = endpoint.to_string_lossy().replace('/', "\\");
        let out = crate::proc::quiet_command("/bin/sh")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("ZEROCODE_HOOK_ENDPOINT", &backslashed)
            .env("ZEROCODE_PANE_KEY", "term-7")
            .output()
            .expect("sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "secret-1234",
            "a backslash-spelled endpoint still finds the pane's grant"
        );
    }

    /// The agent mirror shims' Windows companions: for every resolved agent a
    /// `<agent>.impl.ps1` (BOM, endpoint and team-token loading, then the
    /// mirror binary with the real path and `@args`, wearing its exit code)
    /// and an `<agent>.cmd` naming it — and never an `<agent>.ps1`, which
    /// PowerShell would prefer and its default policy refuse. Until
    /// 2026-09-11 the mirror shims were `#!/bin/sh` files with no extension,
    /// which no Windows host resolves through PATH: a `claude` typed in a
    /// Windows pane bypassed the mirror and the window drew nothing of a
    /// piped nested run (docs/design/windows-parity-audit-20260910.md §3).
    #[test]
    fn the_windows_companions_of_every_agent_mirror_are_pinned_everywhere() {
        let mirror = Path::new(r"C:\Program Files\ZeroCode\zerocode-mirror.exe");
        let agents = vec![
            (
                "claude".to_string(),
                PathBuf::from(r"C:\Users\x\AppData\Roaming\npm\claude.cmd"),
            ),
            (
                "codex".to_string(),
                PathBuf::from(r"C:\Users\x\AppData\Roaming\npm\codex.cmd"),
            ),
        ];
        let scripts = windows_agent_mirror_scripts(mirror, &agents);
        assert_eq!(scripts.len(), 4);
        for (agent, real) in &agents {
            assert!(
                !scripts
                    .iter()
                    .any(|(name, _)| name == &format!("{agent}.ps1")),
                "{agent}.ps1 would shadow the .cmd under PowerShell"
            );
            let implementation = format!("{agent}.impl.ps1");
            let body = scripts
                .iter()
                .find(|(name, _)| name == &implementation)
                .map(|(_, body)| body)
                .unwrap_or_else(|| panic!("{implementation} is missing"));
            assert!(
                body.starts_with("\u{FEFF}$endpoint = $env:ZEROCODE_HOOK_ENDPOINT"),
                "{implementation} does not carry a BOM and load the endpoint first:\n{body}"
            );
            assert!(
                body.contains(&format!(
                    "$tokenFile = $env:{}",
                    zerocode_hookd::env_var::TEAM_TOKEN_FILE
                )),
                "the team token file is not loaded:\n{body}"
            );
            let mirror_text = mirror.to_string_lossy();
            let real_text = real.to_string_lossy();
            assert!(
                body.contains(&format!("& '{mirror_text}' '{real_text}' @args"))
                    && body.contains(&format!("& '{real_text}' @args"))
                    && body.matches("exit $LASTEXITCODE").count() == 2,
                "the mirror road and the fallback road both wear the child's exit code:\n{body}"
            );
            let wrapper = scripts
                .iter()
                .find(|(name, _)| name == &format!("{agent}.cmd"))
                .map(|(_, body)| body)
                .unwrap_or_else(|| panic!("{agent}.cmd is missing"));
            // The agent's door, not a bare door: a Group Policy that
            // refuses the script must still leave the real agent reachable.
            assert_eq!(
                wrapper,
                &zerocode_core::agent_teams::agent_shim_cmd(&implementation, &real_text)
            );
        }
    }

    /// PowerShell ends a single-quoted string at the ASCII `'` AND at the
    /// typographic U+2018, U+2019, U+201A and U+201B, and reads a doubled
    /// one as itself. A profile folder like `O’Neil` must stay inside the
    /// literal: undoubled, the rest of the path was parsed as script and
    /// the `.impl.ps1` — so every `claude` typed in a pane — failed.
    #[test]
    fn every_quote_powershell_honours_is_doubled_inside_a_mirror_path() {
        for quote in ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'] {
            let mirror = PathBuf::from(format!(r"C:\Users\O{quote}Neil\zerocode-mirror.exe"));
            let real = PathBuf::from(format!(r"C:\Users\O{quote}Neil\.local\bin\claude.exe"));
            let body = mirror_shim_script_powershell(&mirror, &real);
            let doubled = |path: &Path| {
                path.to_string_lossy()
                    .replace(quote, &format!("{quote}{quote}"))
            };
            assert!(
                body.contains(&format!(
                    "& '{}' '{}' @args",
                    doubled(&mirror),
                    doubled(&real)
                )) && body.contains(&format!("& '{}' @args", doubled(&real))),
                "{quote:?} was left to end the literal:\n{body}"
            );
            // Nowhere does a lone quote of any kind survive inside a path.
            assert!(
                !body.contains(&format!("O{quote}N")),
                "{quote:?} undoubled:\n{body}"
            );
        }
    }

    /// The Windows companions of the four doors: eight files, each
    /// `<door>.impl.ps1` (a name PowerShell never resolves a bare `<door>`
    /// to, with a byte-order mark for Windows PowerShell 5.1) loading the
    /// same private token files the POSIX door loads, and each `<door>.cmd`
    /// naming it — pinned on every platform. No `<door>.ps1` may exist: it
    /// would shadow the `.cmd` and the default execution policy would
    /// refuse it (review t-2664 M5).
    #[test]
    fn the_windows_companions_of_every_door_are_pinned_everywhere() {
        let scripts = windows_door_scripts();
        assert_eq!(scripts.len(), 10);
        for door in [
            "zerocode-browser",
            "zerocode-computer",
            "zerocode-emulator",
            "zerocode-ssh",
            "zerocode-artifact",
        ] {
            assert!(
                !scripts
                    .iter()
                    .any(|(name, _)| name == &format!("{door}.ps1")),
                "{door}.ps1 shadows the .cmd door under PowerShell"
            );
            let implementation = format!("{door}.impl.ps1");
            let powershell = scripts
                .iter()
                .find(|(name, _)| name == &implementation)
                .map(|(_, body)| body)
                .unwrap_or_else(|| panic!("{implementation} is missing"));
            assert!(
                powershell.starts_with("\u{FEFF}$endpoint = $env:ZEROCODE_HOOK_ENDPOINT"),
                "{implementation} does not carry a BOM and load the endpoint first:\n{powershell}"
            );
            let file_var = if door == "zerocode-browser" {
                "ZEROCODE_BROWSER_TOKEN_FILE"
            } else {
                "ZEROCODE_COMPUTER_TOKEN_FILE"
            };
            assert!(
                door == "zerocode-artifact"
                    || powershell.contains(&format!("$tokenFile = $env:{file_var}")),
                "{door}.ps1 does not read its private token file"
            );
            let route = if door == "zerocode-browser" {
                "/browser"
            } else if door == "zerocode-artifact" {
                "/artifact"
            } else {
                "/computer"
            };
            assert!(powershell.contains(&format!("http://127.0.0.1:$port{route}")));
            let cmd = scripts
                .iter()
                .find(|(name, _)| name == &format!("{door}.cmd"))
                .map(|(_, body)| body)
                .unwrap_or_else(|| panic!("{door}.cmd is missing"));
            assert!(
                cmd.contains(&format!("\"%~dp0{door}.impl.ps1\" %*")),
                "{cmd}"
            );
            assert!(
                cmd.contains("-ExecutionPolicy Bypass"),
                "the policy is per-process: {cmd}"
            );
            assert!(
                !cmd.contains("Set-ExecutionPolicy"),
                "the machine policy is never changed"
            );
            assert!(cmd.starts_with("@echo off\r\n"));
        }
    }

    /// The shim directory as a Windows host sees it, written for real.
    #[cfg(any(windows, test))]
    fn write_windows_doors(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).expect("shim dir");
        for (name, body) in windows_door_scripts() {
            std::fs::write(dir.join(name), body).expect("write a door");
        }
    }

    /// M5, the real thing: `powershell.exe -ExecutionPolicy Restricted`
    /// (the default on client Windows) resolves the BARE door name from a
    /// shim directory at the head of PATH and answers the manual, because
    /// the name reaches the `.cmd`; and `cmd.exe` does too. Runs on
    /// Windows; elsewhere the layout is pinned by the test above.
    #[test]
    fn the_bare_door_names_run_under_a_restricted_execution_policy() {
        if !cfg!(windows) {
            eprintln!("skipping: PowerShell command discovery is a Windows fact");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        write_windows_doors(dir.path());
        let path = format!(
            "{};{}",
            dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        for door in [
            "zerocode-computer",
            "zerocode-browser",
            "zerocode-emulator",
            "zerocode-ssh",
        ] {
            let out = crate::proc::quiet_command("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Restricted",
                    "-Command",
                    &format!("{door} --help"),
                ])
                .env("PATH", &path)
                .output()
                .expect("powershell.exe");
            assert!(
                out.status.success(),
                "{door} under Restricted policy: {}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                String::from_utf8_lossy(&out.stdout).contains(door),
                "{door} --help did not print its manual"
            );
            let out = crate::proc::quiet_command("cmd.exe")
                .args(["/d", "/c", &format!("{door} --help")])
                .env("PATH", &path)
                .output()
                .expect("cmd.exe");
            assert!(
                out.status.success(),
                "{door} under cmd: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// The launch cost of the complete CLI path's first hop — what an
    /// agent's tool call pays before any byte reaches the window: on
    /// Windows `cmd → <door>.cmd → powershell.exe → --help`, on unix
    /// `sh <door> --help`. Answered locally (no bridge), N=20, p50/p95 in
    /// milliseconds; JSON to `ZEROCODE_COMPUTER_PERF_OUT` when
    /// `ZEROCODE_COMPUTER_PERF` is set, otherwise skipped like the provider
    /// baseline.
    #[test]
    fn door_launch_overhead() {
        if std::env::var_os("ZEROCODE_COMPUTER_PERF").is_none() {
            eprintln!("skipping door_launch_overhead: set ZEROCODE_COMPUTER_PERF=1 to measure");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let mut report = serde_json::Map::new();
        let mut axis = |name: &str, mut command: Box<dyn FnMut() -> std::process::Output>| {
            let mut durations: Vec<f64> = (0..20)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let out = command();
                    assert!(
                        out.status.success(),
                        "{name}: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p = |f: f64| durations[((durations.len() - 1) as f64 * f).round() as usize];
            report.insert(
                name.to_string(),
                serde_json::json!({ "samples": 20, "p50Ms": p(0.5), "p95Ms": p(0.95), "minMs": durations[0], "maxMs": durations[19] }),
            );
        };
        if cfg!(windows) {
            write_windows_doors(dir.path());
            let path = format!(
                "{};{}",
                dir.path().display(),
                std::env::var("PATH").unwrap_or_default()
            );
            let cmd_path = path.clone();
            axis(
                "door.cmd.powershell.help",
                Box::new(move || {
                    crate::proc::quiet_command("cmd.exe")
                        .args(["/d", "/c", "zerocode-computer --help"])
                        .env("PATH", &cmd_path)
                        .output()
                        .expect("cmd.exe")
                }),
            );
            let implementation = dir
                .path()
                .join(door_implementation_name("zerocode-computer"));
            axis(
                "door.powershell.direct.help",
                Box::new(move || {
                    crate::proc::quiet_command("powershell.exe")
                        .args([
                            "-NoProfile",
                            "-NonInteractive",
                            "-ExecutionPolicy",
                            "Bypass",
                            "-File",
                        ])
                        .arg(&implementation)
                        .arg("--help")
                        .output()
                        .expect("powershell.exe")
                }),
            );
            axis(
                "curl.exe.version",
                Box::new(|| {
                    crate::proc::quiet_command("curl.exe")
                        .arg("--version")
                        .output()
                        .expect("curl.exe")
                }),
            );
        } else {
            let door = dir.path().join("zerocode-computer");
            std::fs::write(
                &door,
                shim_script_with_private_tokens(
                    zerocode_core::computer_use::shim_script("P", "C", "H"),
                    &[("C", "CF")],
                ),
            )
            .expect("write the door");
            axis(
                "door.sh.help",
                Box::new(move || {
                    crate::proc::quiet_command("/bin/sh")
                        .arg(&door)
                        .arg("--help")
                        .output()
                        .expect("sh")
                }),
            );
        }
        let json = serde_json::to_string_pretty(&serde_json::Value::Object(report)).unwrap();
        eprintln!("{json}");
        if let Some(path) = std::env::var_os("ZEROCODE_COMPUTER_PERF_OUT") {
            std::fs::write(path, json).expect("write the door report");
        }
    }
}
