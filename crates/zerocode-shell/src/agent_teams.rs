//! The window's half of an orchestrating agent's team.
//!
//! `zerocode_core::agent_teams` decides everything; this file does the four
//! things a decision cannot do by itself — put the fake `tmux` on disk, hold
//! the teams a window has open, cut a pane, and tell the window about it.
//!
//! **Why the window is told rather than asked.** A split arrives from a
//! process, not from a gesture: nobody clicked, and the leader is blocked on
//! the answer. So the pane is spawned here and announced as `term:split`,
//! which the window turns into a real leaf of the tab already holding the
//! leader. That is the same shape `automation:started` already has, for the
//! same reason — the alternative is a command the window would have to poll
//! for.
//!
//! **The safety line this file must not cross.** The one thing a team can do
//! that nothing else in this window can is *type into a terminal a person is
//! using*. `send_keys_text` has already dropped the keys that could end
//! somebody's work, and [`run`] refuses a pane the asking team does not own —
//! but the rule underneath both is the older one: a shell nobody is watching
//! does not press Enter at a screen it did not put there. A team may only
//! reach the panes it opened, plus the leader that opened them.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use sha2::{Digest, Sha256};
use zerocode_core::agent_teams::{
    Dialect, Direction, Effect, TEAM_PANE_VAR, TEAM_TOKEN_VAR, Team, TeamsMode, capture_reply,
    plan, shim_script, team_launch_env,
};

pub(crate) const SHIM_DIR_NAME: &str = "agent-teams-bin";
/// The shims this window puts on a team member's `PATH`, and the voice each
/// refuses in.
///
/// One script, two names, and the second name is the point. `tmux` is a
/// vendor's expectation — Claude's Agent Teams runs it and reads its answers —
/// so it is installed for the agents that speak it and would be a lie in front
/// of anyone else's real tmux. `zerocode-orc` is nobody's expectation: it is
/// this window's own name for its own road, it shadows no program that exists,
/// and any agent that can read a guide can type it.
///
/// **A directory each, not two files in one.** `PATH` exposes a DIRECTORY. A
/// codex leader handed the directory that holds `tmux` would have our fake in
/// front of any real one it ran — which is exactly what the tmux shim must
/// never do to an agent that did not ask for it.
const SHIMS: &[(&str, &str, &str)] = &[
    (SHIM_DIR_NAME, "tmux", "tmux"),
    (ORC_DIR_NAME, ORC_SHIM_NAME, "orchestration"),
];

/// What an agent types to reach the ledger.
pub(crate) const ORC_SHIM_NAME: &str = "zerocode-orc";
/// Where it lives — its own directory, for the reason above.
pub(crate) const ORC_DIR_NAME: &str = "orchestration-bin";

/// Every team this window has open, by id.
///
/// A `static` rather than a field of the app state: a team outlives no window
/// and belongs to no workspace, and threading it through the state struct
/// would put a lock in the way of every command that already borrows it.
///
/// Reachable from [`crate::orchestration`] because a `worker-start` is a
/// `split-window` written in another dialect, and it has to be planned against
/// the same pane table for the same reason [`run`] holds it across a whole
/// call: a plan and its write-down must not be separated by another request.
pub(crate) fn teams() -> MutexGuard<'static, HashMap<String, Team>> {
    team_table().lock().unwrap_or_else(|held| held.into_inner())
}

/// The one static behind [`teams`].
///
/// Split out so the lock itself can be asked a question, not just held. There
/// is exactly one caller of that: a test proving that a road which reads a seat
/// has not let go of the table before it acts on what it read.
#[cfg(test)]
pub(crate) fn table_is_held_for_tests() -> bool {
    match team_table().try_lock() {
        /* A poisoned-but-free lock still ACQUIRES — only WouldBlock means
         * somebody is holding it right now. */
        Ok(_) => false,
        Err(std::sync::TryLockError::WouldBlock) => true,
        Err(std::sync::TryLockError::Poisoned(_)) => false,
    }
}
fn team_table() -> &'static Mutex<HashMap<String, Team>> {
    static TEAMS: OnceLock<Mutex<HashMap<String, Team>>> = OnceLock::new();
    TEAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether the team table is held by somebody right now.
///
/// `try_lock` and not a second thread with a deadline: a competing thread that
/// has not been scheduled yet looks exactly like a lock that was free, so a
/// deadline can only ever say "nobody got it in time", which is true of the bug
/// as well. This answers about the lock rather than about the scheduler, and it
/// answers the same way from the owning thread — `try_lock` does not block, so
/// a road calling this while holding the table reads `WouldBlock` about itself.
#[cfg(test)]
pub(crate) fn team_table_is_locked() -> bool {
    matches!(
        team_table().try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    )
}

/// One opaque inherited capability per pane.
///
/// Kept beside, not inside, `Team`: a child token is minted by the shell at
/// spawn time, while the core planner has no RNG and must stay pure. Keeping
/// it out of `Team` also keeps credentials out of that type's Debug and serde
/// representations. This separates ambient authority between cooperating
/// panes; it is not an OS sandbox against a hostile same-UID process that can
/// inspect another process's memory or environment.
fn pane_tokens() -> MutexGuard<'static, HashMap<(String, String), String>> {
    static TOKENS: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
    TOKENS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// A private, process-owned directory for per-pane team capabilities.
///
/// The file name is a digest of the non-secret team/pane address, never the
/// token itself. This lets `teammate_env` hand a path to the shim without
/// putting the pane secret in the child environment.
const TEAM_TOKEN_DIR_NAME: &str = "zerocode-agent-team-tokens";

fn team_token_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{TEAM_TOKEN_DIR_NAME}-{}", std::process::id()))
}

fn team_token_file_path(team: &str, pane: &str) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update((team.len() as u64).to_le_bytes());
    digest.update(team.as_bytes());
    digest.update((pane.len() as u64).to_le_bytes());
    digest.update(pane.as_bytes());
    let mut name = String::with_capacity(64 + 6);
    for byte in digest.finalize() {
        let _ = write!(&mut name, "{byte:02x}");
    }
    name.push_str(".token");
    team_token_dir().join(name)
}

fn ensure_team_token_file(team: &str, pane: &str, token: &str) -> Option<PathBuf> {
    let path = team_token_file_path(team, pane);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(token) {
        return Some(path);
    }
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    zerocode_hookd::endpoint::write_private_token_file(dir, name, token).ok()
}

fn remove_team_token_file(team: &str, pane: &str) {
    let _ = std::fs::remove_file(team_token_file_path(team, pane));
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkerHostPlacement {
    /// Work in the leader's checkout.
    Inherited,
    /// Cut a new checkout with this generated name.
    New(String),
    /// Reopen a durable worker in this exact existing checkout.
    Existing(String),
}

/// What the placement seat is asked about when this pane is seated: the
/// summons' own words, cut to the seat's cap, and the ids its row is about.
/// Carried on the `term:worker` event so the surface that seats the pane
/// (`ui/shell.js`) can ask the door with what it knows — panes, the room in
/// front, whether anybody is at the keyboard — and the words it does not.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerSeatWords {
    pub(crate) run: String,
    pub(crate) worker: String,
    pub(crate) dispatch: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) brief: String,
    pub(crate) brief_chars: usize,
}

impl WorkerSeatWords {
    /// The words of a fresh summons, cut to the placement seat's cap.
    pub(crate) fn of(prepared: &zerocode_core::orchestration::PreparedWorkerStart) -> Self {
        Self {
            run: prepared.run.clone(),
            worker: prepared.worker.clone(),
            dispatch: prepared.dispatch.clone(),
            task: prepared.task.clone(),
            brief: zerocode_core::jev::door::cut(
                &prepared.prompt,
                zerocode_core::jev::Cap::Chars(zerocode_core::jev::PLACEMENT_BRIEF_CHAR_CAP),
            )
            .to_string(),
            brief_chars: prepared.prompt.chars().count(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkerHostSpec {
    pub(crate) placement: WorkerHostPlacement,
    pub(crate) prompt: String,
    pub(crate) timeout_ms: u32,
    /// The placement seat's question, for a fresh summons; a restart's
    /// reseat asks nothing — the pane goes back where it was.
    pub(crate) seat: Option<WorkerSeatWords>,
    /// Present only for a restart restoration. The production host withholds
    /// `term:worker` until the durable reseat succeeds, then publishes this
    /// birth fact with the event.
    pub(crate) resumed: Option<zerocode_core::orchestration::WorkerResume>,
}

/// One typed orchestration-worker request, keyed by its child capability.
///
/// Placement, optional fresh checkout, briefing and readiness budget are one
/// launch decision and cross the host boundary together. Agent Teams splits
/// have no row here and retain their tiled-pane behaviour. This lock is a leaf:
/// nothing is acquired while it is held.
fn worker_host_asks() -> MutexGuard<'static, HashMap<String, WorkerHostSpec>> {
    static ASKS: OnceLock<Mutex<HashMap<String, WorkerHostSpec>>> = OnceLock::new();
    ASKS.get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// A worker-only pre-spawn refusal, carried back on the same child capability
/// as its launch spec. Agent Teams splits have no entry and keep tmux's generic
/// refusal; orchestration workers receive the concrete account/trust/spawn
/// reason that the host observed.
fn worker_host_failures() -> MutexGuard<'static, HashMap<String, HostStartFailure>> {
    static FAILURES: OnceLock<Mutex<HashMap<String, HostStartFailure>>> = OnceLock::new();
    FAILURES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

pub(crate) struct WorkerHostAsk {
    token: String,
}

impl WorkerHostAsk {
    pub(crate) fn place(token: &str, spec: WorkerHostSpec) -> Self {
        worker_host_asks().insert(token.to_string(), spec);
        Self {
            token: token.to_string(),
        }
    }
}

impl Drop for WorkerHostAsk {
    fn drop(&mut self) {
        worker_host_asks().remove(&self.token);
        worker_host_failures().remove(&self.token);
    }
}

pub(crate) fn take_worker_host_ask(token: &str) -> Option<WorkerHostSpec> {
    worker_host_asks().remove(token)
}

pub(crate) fn place_worker_host_failure(token: &str, failure: HostStartFailure) {
    worker_host_failures().insert(token.to_string(), failure);
}

pub(crate) fn take_worker_host_failure(token: &str) -> Option<HostStartFailure> {
    worker_host_failures().remove(token)
}

/// Which helper a split's pane IS, keyed by the child capability.
///
/// zo's pane lane cuts a helper's pane with `-e ZO_AGENT_ID=<id>` and
/// [`run`] finds the id on the planned split (`Effect::Split { helper }`). The
/// host that cuts the pane announces it to the window (`term:split`), and the
/// window needs the id ON that announcement to fold the helper's hook row and
/// the pane's row into one (t-3024). It travels here rather than as a ninth
/// `Host::split` argument for the reason the eighth already gave: the spec
/// struct that folds them belongs to the slice that touches every host. Same
/// key as the worker ask (minted once per split, never reused), same leaf
/// standing: nothing is acquired while this is held.
fn split_helpers() -> MutexGuard<'static, HashMap<String, String>> {
    static HELPERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    HELPERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// The placed identity, swept when the split that placed it is over — a host
/// that refused never took it, and an entry nobody takes is a leak keyed by a
/// token nobody will mint twice.
pub(crate) struct SplitHelperAsk {
    token: String,
}

impl SplitHelperAsk {
    pub(crate) fn place(token: &str, helper: &str) -> Self {
        split_helpers().insert(token.to_string(), helper.to_string());
        Self {
            token: token.to_string(),
        }
    }
}

impl Drop for SplitHelperAsk {
    fn drop(&mut self) {
        split_helpers().remove(&self.token);
    }
}

/// Taken by the host at the spawn, once: the id rides out on `term:split`.
pub(crate) fn take_split_helper(token: &str) -> Option<String> {
    split_helpers().remove(token)
}

/// Where each summoned pane actually sits, keyed by the child capability.
///
/// The reverse of `worktree_asks`: that channel carries the ledger's ask
/// OUT to the host, this one carries the host's answer BACK — the checkout
/// it resolved under the pane, the leader's own tree or the fresh cut a
/// `--worktree` asked for. Same key for the same reason (minted once per
/// split, never reused), and the same LEAF standing: taken alone or under
/// the team table, nothing ever taken while holding it.
fn seat_checkouts() -> MutexGuard<'static, HashMap<String, String>> {
    static SEATS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    SEATS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// The host says which checkout ended up under the pane it spawned.
pub(crate) fn place_seat_checkout(token: &str, checkout: String) {
    seat_checkouts().insert(token.to_string(), checkout);
}

/// The answer a split's capability carries back, spent by reading it. Taken
/// on every road out of the split walk, success or not, so an entry cannot
/// outlive the one call it was placed for.
pub(crate) fn take_seat_checkout(token: &str) -> Option<String> {
    seat_checkouts().remove(token)
}

pub(crate) fn remember_pane_token(team: &str, pane: &str, token: String) -> Option<String> {
    let previous = pane_tokens().insert((team.to_string(), pane.to_string()), token.clone());
    let _ = ensure_team_token_file(team, pane, &token);
    previous
}

pub(crate) fn restore_pane_token(team: &str, pane: &str, previous: Option<String>) {
    let key = (team.to_string(), pane.to_string());
    match previous {
        Some(token) => {
            let _ = ensure_team_token_file(team, pane, &token);
            pane_tokens().insert(key, token);
        }
        None => {
            pane_tokens().remove(&key);
            remove_team_token_file(team, pane);
        }
    }
}

/// The capability a pane holds right now, copied out and the guard given back.
///
/// For the one caller with nobody to present a token on its behalf: the
/// standing-order beat runs the same `worker-start` a coordinator would have
/// typed, from that coordinator's own seat, and there is no request behind it
/// carrying a header. It reads the capability here rather than being trusted to
/// skip the check — a bypass would be a second road into the ledger with
/// different rules, and the rule worth having is that there is only one road.
///
/// **The token guard is dropped before this returns, and the team table is
/// never taken while it is held.** `forget_term` takes the teams table and then
/// these tokens; a caller holding these and reaching for that table would be
/// the other direction of the same pair, which is a deadlock waiting for a
/// pane to close at the wrong moment. So it clones.
///
/// A capability that rotates between the clone and the verb is not a hole:
/// [`run`] and the ledger road both re-check it under the team guard and
/// refuse, which is exactly what should happen to a beat planned against a pane
/// that has since been respawned.
pub(crate) fn current_pane_capability(team: &str, pane: &str) -> Option<String> {
    pane_tokens()
        .get(&(team.to_string(), pane.to_string()))
        .cloned()
}

/// Borrow the exact pane incarnation a request proved it came from.
///
/// Requiring the caller's team table and returning a borrow from it makes the
/// safety boundary structural: the same guard stays alive through planning and
/// the effect. A separate boolean precheck would let a respawn rotate the
/// capability before a second lock and let an old request act on the new pane.
pub(crate) fn authorized_team_mut<'a>(
    held: &'a mut HashMap<String, Team>,
    team_id: &str,
    pane: &str,
    presented: &str,
) -> Option<&'a mut Team> {
    let team = held.get_mut(team_id)?;
    if team.id != team_id || team.pane(pane).is_none() {
        return None;
    }
    let expected = pane_tokens()
        .get(&(team_id.to_string(), pane.to_string()))
        .cloned();
    expected
        .is_some_and(|expected| constant_time_eq(expected.as_bytes(), presented.as_bytes()))
        .then_some(team)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut different = left.len() ^ right.len();
    let width = left.len().max(right.len());
    for index in 0..width {
        different |= usize::from(left.get(index).copied().unwrap_or_default())
            ^ usize::from(right.get(index).copied().unwrap_or_default());
    }
    different == 0
}

/// Write the shim if it is not already what it should be, and say where it is.
///
/// Rewritten on content rather than blindly: the file is on an agent's `PATH`
/// and replacing it under a running leader would be a `tmux` that vanished
/// mid-command.
fn install_shim(local_data_root: &Path) -> Option<Vec<PathBuf>> {
    let mut dirs = Vec::with_capacity(SHIMS.len());
    for (folder, name, voice) in SHIMS {
        let dir = local_data_root.join(folder);
        std::fs::create_dir_all(&dir).ok()?;
        let wanted = crate::hooks::shim_script_with_private_tokens(
            shim_script(
                zerocode_hookd::env_var::PORT,
                zerocode_hookd::env_var::TOKEN,
                voice,
            ),
            &[(
                zerocode_core::agent_teams::TEAM_TOKEN_VAR,
                zerocode_hookd::env_var::TEAM_TOKEN_FILE,
            )],
        );
        install_file(&dir.join(name), &wanted)?;
        /* The ledger voice gains its Windows companions beside the POSIX
         * script: PowerShell runs the `.ps1` it finds on PATH, cmd resolves
         * the `.cmd` through PATHEXT, and Git Bash keeps reading the shebang
         * file all three sit beside. The fake `tmux` stays sh-only on
         * purpose — the one tmux speaker is Claude's Agent Teams, whose Bash
         * tool is a POSIX shell on every platform including Windows. */
        #[cfg(windows)]
        if *name == ORC_SHIM_NAME {
            for (companion, body) in windows_companions(name, voice) {
                install_file(&dir.join(&companion), &body)?;
            }
        }
        dirs.push(dir);
    }
    Some(dirs)
}

/// Write one shim file if it is not already what it should be.
///
/// Rewritten on content rather than blindly: the file is on an agent's `PATH`
/// and replacing it under a running leader would be a `tmux` that vanished
/// mid-command.
fn install_file(path: &Path, wanted: &str) -> Option<()> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(wanted) {
        return Some(());
    }
    std::fs::write(path, wanted).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some(())
}

/// The two files a Windows install writes beside the POSIX shim, and what
/// goes in them.
///
/// Pure, and compiled for tests everywhere, so the shape is pinned on every
/// platform that builds this crate rather than only on the one that writes
/// the files.
#[cfg(any(windows, test))]
fn windows_companions(name: &str, voice: &str) -> [(String, String); 2] {
    let powershell = format!("{name}.ps1");
    let body = crate::hooks::powershell_shim_script_with_private_tokens(
        zerocode_core::agent_teams::shim_script_powershell(
            zerocode_hookd::env_var::PORT,
            zerocode_hookd::env_var::TOKEN,
            voice,
        ),
        &[(
            zerocode_core::agent_teams::TEAM_TOKEN_VAR,
            zerocode_hookd::env_var::TEAM_TOKEN_FILE,
        )],
    );
    let wrapper = zerocode_core::agent_teams::shim_cmd(&powershell);
    [(powershell, body), (format!("{name}.cmd"), wrapper)]
}

/// Which shim directories this dialect may see.
///
/// The ledger's is first for both, so a leader that speaks tmux still reaches
/// the same road — and the tmux directory is simply absent for everyone else
/// rather than present-and-unused.
fn dirs_for(dialect: Dialect, installed: &[PathBuf]) -> Vec<String> {
    installed
        .iter()
        .zip(SHIMS)
        .filter(|(_, (_, name, _))| dialect == Dialect::Tmux || *name == ORC_SHIM_NAME)
        .map(|(dir, _)| dir.to_string_lossy().into_owned())
        .collect()
}

/// The `PATH` a summoned worker needs, given the one it would otherwise get.
///
/// **A worker was losing the shims entirely.** Its environment starts as a copy
/// of the leader's — which carries `<shims>:<real>` — and then [`crate::hooks::pty_env`]
/// appends its own `PATH` twice, computing the second from ITS OWN vec and not
/// from the caller's. Later pairs win at spawn time, so the worker ended up
/// with `<mirror-shims>:<hydrated>` and nothing else: `zerocode-orc` was not
/// shadowed, it was gone. The briefing tells every worker to report with a
/// command it could not run, which is why a worker going quiet without
/// reporting was not an edge case but the only case.
///
/// The dialect is the WORKER's, never the leader's. A claude leader summoning a
/// codex worker must not put a fake `tmux` in front of it — the reason is
/// written where the leader's own environment is built
/// (`zerocode_core::agent_teams::team_launch_env`): a shell, a pager and an
/// editor all behave differently for a multiplexer that is not there.
///
/// `None` when there is nothing to add — the shim could not be written — and
/// then the caller leaves the `PATH` exactly as it found it.
pub(crate) fn teammate_path(
    local_data_root: &Path,
    dialect: Dialect,
    base: &str,
) -> Option<String> {
    let installed = install_shim(local_data_root)?;
    let dirs = dirs_for(dialect, &installed);
    if dirs.is_empty() {
        return None;
    }
    // The LEADER's shims come out first. A worker's environment is a copy of
    // its leader's, so the leader's directories are sitting in that `PATH`
    // already — and one of them may be the fake `tmux` this worker must not
    // find. Adding this worker's own in front of them would leave the other
    // dialect reachable one entry further down, which is the same wrong answer
    // arrived at more slowly.
    let ours: Vec<String> = installed
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    let sep = zerocode_core::agent_teams::PATH_SEPARATOR;
    let cleaned: Vec<&str> = base
        .split(sep)
        .filter(|entry| !entry.is_empty() && !ours.iter().any(|held| held == entry))
        .collect();
    let borrowed: Vec<&str> = dirs.iter().map(String::as_str).collect();
    Some(zerocode_core::agent_teams::shim_path(
        &borrowed,
        &cleaned.join(&sep.to_string()),
    ))
}

/// Open a team for a leader about to start, and hand back the environment it
/// has to carry.
///
/// Empty means "no team": the mode is off, the shim could not be written, or
/// the bridge is not listening — and in every one of those cases the agent
/// must start anyway, as the agent it was, rather than being refused a launch
/// over a feature nobody asked for yet.
pub fn open_team(
    local_data_root: &Path,
    leader_term: u32,
    mode: TeamsMode,
    path: &str,
    program: &str,
) -> Vec<(String, String)> {
    if mode != TeamsMode::Panes {
        return Vec::new();
    }
    let Some(installed) = install_shim(local_data_root) else {
        return Vec::new();
    };
    // The shim authenticates with the BRIDGE's token, so a window with no
    // bridge has nothing for it to present.
    if crate::hooks::bridge().is_none() {
        return Vec::new();
    }
    // The bridge already knows how to mint a secret from /dev/urandom, and
    // this is the same kind of secret for the same listener — a second
    // generator would be a second thing to get wrong.
    let Some(id) = crate::hooks::random_token() else {
        return Vec::new();
    };
    let Some(token) = crate::hooks::random_token() else {
        return Vec::new();
    };
    let id = format!("team-{id}");
    let dialect = Dialect::of(program);
    let dirs = dirs_for(dialect, &installed);
    let borrowed: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let mut env = team_launch_env(
        &id,
        &token,
        &borrowed,
        path,
        &std::env::var("COLORTERM").unwrap_or_default(),
        program,
    );
    teams().insert(
        id.clone(),
        Team::new(id.clone(), token.clone(), leader_term),
    );
    let _ = remember_pane_token(&id, zerocode_core::agent_teams::LEADER_PANE, token.clone());
    // `team_launch_env` is core's provider-neutral shape and therefore still
    // contains the value. Replace that one pair at the shell boundary with
    // the path-only form used by the installed shim.
    let token_file = ensure_team_token_file(&id, zerocode_core::agent_teams::LEADER_PANE, &token);
    env.retain(|(name, _)| {
        name != zerocode_core::agent_teams::TEAM_TOKEN_VAR
            && name != zerocode_hookd::env_var::TEAM_TOKEN_FILE
    });
    if let Some(path) = token_file {
        env.push((
            zerocode_hookd::env_var::TEAM_TOKEN_FILE.into(),
            path.to_string_lossy().into_owned(),
        ));
    } else {
        env.push((zerocode_core::agent_teams::TEAM_TOKEN_VAR.into(), token));
    }
    env
}

/// A shell ended. If it was a leader, its team goes with it.
///
/// Only the leader's death ends a team (`removeTeamForLeaderHandle`,
/// :173308-173310). A teammate that exits leaves a pane the leader can still
/// name — and `list-panes` is how it finds out that pane is gone, which it
/// cannot do if the whole table was thrown away.
pub fn forget_term(term: u32) {
    // The takeover gate and the attention verdict are per incarnation of
    // the number: the next pane minted as this term is a different terminal
    // under different hands, and it starts unevaluated.
    crate::orchestration_notify::forget_term(term);
    crate::orchestration::forget_taken_term(term);
    crate::orchestration::forget_pane_attention(term);
    let mut held = teams();
    let mut retired = Vec::new();
    /* A leader's exit retires the LEADER's pane, not its children's (t-2512).
     *
     * The table used to go whole, children and all — and with it every
     * child's capability token: the orphan kept working in a pane the
     * window still drew, and its `worker_done`, `ask` and `status` were all
     * answered "stale or unauthorized agent pane" for the rest of its life.
     * The ledger had already learned to keep an orphan's attempt open
     * (`team_dissolved`); the table was what made the promise empty.
     *
     * So a team whose leader died stays in the table for as long as it has a
     * child pane left, LEADERLESS: its leader pane record is gone, so no
     * caller can sign as its coordinator, and `leader_term` keeps naming the
     * dead terminal — a number this process never mints twice — so no restore
     * pass mistakes it for a living leader's team. The children keep their
     * tokens, the bridge keeps answering them, the reconciler keeps mapping
     * their seats to terminals, and each child's own exit settles it through
     * the ordinary road below. The table goes when the last child does. */
    held.retain(|team_id, team| {
        if team.leader_term != term {
            return true;
        }
        let leader = team.leader_pane.clone();
        if team.pane(&leader).is_some() {
            team.remove_pane(&leader);
            retired.push((team_id.clone(), leader));
        }
        let orphans: Vec<(String, String)> = team
            .panes()
            .filter(|pane| pane.term != term)
            .map(|pane| (team_id.clone(), pane.id.clone()))
            .collect();
        if orphans.is_empty() {
            retired.extend(team.panes().map(|pane| (team_id.clone(), pane.id.clone())));
            return false;
        }
        true
    });
    for team in held.values_mut() {
        let gone: Vec<String> = team
            .panes()
            .filter(|pane| pane.term == term)
            .map(|pane| pane.id.clone())
            .collect();
        for pane in gone {
            team.remove_pane(&pane);
            retired.push((team.id.clone(), pane));
        }
    }
    // A leaderless team whose last child just left has nothing left to hold.
    held.retain(|_, team| team.pane(&team.leader_pane).is_some() || team.panes().next().is_some());
    let living: std::collections::HashSet<(String, String)> = held
        .iter()
        .flat_map(|(team, held)| {
            held.panes()
                .map(|pane| (team.clone(), pane.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    pane_tokens().retain(|key, _| living.contains(key));
    for (team, pane) in retired {
        remove_team_token_file(&team, &pane);
    }
}

/// How a closed pane's program left (t-7538).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneExit {
    /// Its whole process group is gone.
    Gone,
    /// Something of it outlived the wait.
    Lingering(ExitWitness),
}

/// What a later look asks about a program that outlived its pane's close —
/// the ledger's own type, since the ledger holds it on the worker's row
/// until a look sees the program gone (t-7538, astra R3).
pub use zerocode_core::orchestration::ExitWitness;

/// Which managed login a pane was launched as (t-7538): the account row's id
/// and the login that row named at that launch ([`crate::usage_runtime`]'s
/// `claude_login_key`). A pane keeps the credentials it started with, so its
/// wall is judged by a reading of THIS login — an id that has since come to
/// name another login is another account's number (astra R6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLogin {
    pub account: String,
    pub login: String,
}

/// What [`run`] needs from the window, kept behind a trait so the whole
/// protocol can be driven in a test without a pty or a Tauri handle.
///
/// Every method answers `None`/`false` on failure rather than an error type:
/// there is exactly one thing to say to a leader whose request could not be
/// carried out, and [`run`] says it in tmux's own words.
pub trait Host {
    /// Cut `from`'s pane and start `command` there. `pane` is the id the child
    /// must carry as its own `TMUX_PANE`. Answers the new shell.
    ///
    /// `leader_term` is handed over rather than looked up. [`run`] holds the
    /// team table for the whole call — it has to, the plan and the write-down
    /// must not be separated by another request — and a host that reached back
    /// into that table would lock a `Mutex` its own caller is already holding.
    /// Every fact the host needs about the team travels in the arguments for
    /// that reason.
    ///
    /// `token` is the capability the child pane will hold, minted by the
    /// CALLER and handed over rather than minted here: the journaled split
    /// lane digests the token into the operation it records before the host
    /// runs, and a host that minted its own would spawn a pane whose
    /// generation the record does not name. The host still REGISTERS it
    /// (`remember_pane_token`) beside the spawn, with the same rollback it
    /// has always kept.
    /* Eight arguments: the shape every host has implemented since the
     * beginning, plus the minted capability. Folding them into a spec struct
     * belongs to the lifecycle slice that will touch every host anyway. */
    #[allow(clippy::too_many_arguments)]
    fn split(
        &self,
        team: &str,
        leader_term: u32,
        from_term: u32,
        pane: &str,
        direction: Direction,
        command: &str,
        token: &str,
    ) -> Option<u32>;

    /// Wait for a newly spawned orchestration worker to accept its briefing.
    ///
    /// The durable split calls this only after it has released the team-table
    /// fence. A production implementation may wait for a TUI and, on failure,
    /// walk back through [`forget_term`]; doing either under the fence would
    /// re-enter the same non-reentrant mutex and freeze the window. Ordinary
    /// Agent Teams splits have no launch briefing, so hosts default to ready.
    fn await_worker_ready(&self, _term: u32) -> Result<(), HostStartFailure> {
        Ok(())
    }
    fn send(&self, term: u32, text: &str) -> bool;
    /// Queue one line at a pane through the window's guarded typed-prompt
    /// door, and answer the receipt to poll — nothing is written here.
    ///
    /// The mail pointer's road. [`Host::send`] writes raw bytes the moment it
    /// is called, which is right for a tmux `send-keys` and wrong for advice
    /// typed into a composer a person may be using: it consults nobody about
    /// the draft on the line, the question parked on the pane, or the hand
    /// that arrives while the words settle. This road registers ONE delivery
    /// — the paste and, when `submit`, the Enter — with the same pump and the
    /// same [`zerocode_pty::ready::Guard`] every programmatic producer shares,
    /// so the decisions are made at the write and the outcome says what was
    /// actually done. The caller polls the receipt on later beats; a beat
    /// never blocks on it.
    ///
    /// `None` is a pane that cannot be addressed at all. The default rides
    /// `send` twice and answers at once, so a fake host that only records
    /// keystrokes sees the line and its Enter exactly as it always did; a
    /// host whose `send` refuses answers a timed-out receipt.
    fn point(
        &self,
        term: u32,
        line: &str,
        submit: bool,
    ) -> Option<std::sync::mpsc::Receiver<zerocode_pty::DeliveryOutcome>> {
        let landed = self.send(term, line) && (!submit || self.send(term, "\r"));
        let (settled, receipt) = std::sync::mpsc::sync_channel(1);
        let _ = settled.send(if landed {
            zerocode_pty::DeliveryOutcome::Delivered
        } else {
            zerocode_pty::DeliveryOutcome::TimedOut
        });
        Some(receipt)
    }
    /// Paste prose at one pane and submit it.
    ///
    /// [`Host::send`] types keys; this delivers a document. The window's real
    /// implementation wraps the bracketed envelope by the pane's ACTUAL paste
    /// mode — the grid's fact, which nothing upstream can know — and follows
    /// the close with the one submitting Enter. The sanitizing that makes an
    /// escape inert happens before this call, in the effect's own arm, where
    /// a test can witness it; the default rides `send` so every fake host
    /// that only records keystrokes observes the same text either way.
    fn paste(&self, term: u32, text: &str) -> bool {
        self.send(term, text)
    }
    /// Why this pane refuses ledger deliveries right now, when it does: the
    /// exact launch contract its zo refused (t-2773), in one sentence. The
    /// window answers from its integration record and, the first time, says
    /// so in the pane; nothing here types anything. Hosts without such a
    /// record — tmux-only, tests — refuse nobody.
    fn delivery_refusal(&self, _term: u32) -> Option<String> {
        None
    }
    /// Whether this window still holds the pane behind `term`.
    ///
    /// The default keeps small and test-only hosts source-compatible. The
    /// window's real host overrides it because capturing a pane copies its
    /// whole visible grid, while existence is only a key lookup there.
    fn pane_exists(&self, term: u32) -> bool {
        self.capture(term).is_some()
    }
    /// When this live pane became quiet enough to count as stalled.
    ///
    /// The real window combines hook and PTY facts here. Test and tmux-only
    /// hosts default to no observation rather than inventing silence.
    fn quiet_since(&self, _term: u32, _worker_started_ms: i64, _now_ms: i64) -> Option<i64> {
        None
    }

    /// When this worker pane went quiet, for a classifier decline's reading
    /// (t-6747): quiet by [`Self::quiet_since`], or — whatever its hook last
    /// said — silent on its pty for at least `hook_outlived_ms`. Claude
    /// Code's pause dialog ends no turn and says so only through a
    /// `Notification` hook this window does not install, so the pane's last
    /// word stays `working` for as long as the dialog stands, while its pty
    /// goes still. Test and tmux-only hosts default to the ordinary rule.
    fn decline_quiet_since(
        &self,
        term: u32,
        worker_started_ms: i64,
        now_ms: i64,
        _hook_outlived_ms: i64,
    ) -> Option<i64> {
        self.quiet_since(term, worker_started_ms, now_ms)
    }
    fn capture(&self, term: u32) -> Option<String>;
    fn focus(&self, term: u32) -> bool;
    fn close(&self, term: u32);
    /// Close a pane and answer only once the program that was in it is
    /// gone — its whole process group (t-7538). The account switch resumes
    /// the same conversation in a new pane right after, and a CLI still
    /// flushing the old pane's last lines is a second writer on the same
    /// transcript. [`PaneExit::Lingering`] is a group that outlived the
    /// wait, with what a later look asks about it
    /// ([`Self::exit_seen`]): the caller must not start the conversation
    /// again, and no restore may either, until a look sees it gone.
    ///
    /// Hosts without processes of their own close and answer at once.
    fn close_gone(&self, term: u32) -> PaneExit {
        self.close(term);
        PaneExit::Gone
    }

    /// The program running in a pane, read while it stands — what a switch
    /// holds on the worker's row before it closes the pane (t-7538, astra
    /// R3), and what [`Self::exit_seen`] is later asked about. `None` for a
    /// pane with no program the host can name; hosts without processes of
    /// their own have none.
    fn exit_witness(&self, _term: u32) -> Option<ExitWitness> {
        None
    }

    /// Whether the program a lingering close left behind has left since
    /// (t-7538, astra R3) — asked by every restore of that worker before it
    /// opens the conversation again. Hosts without processes of their own
    /// never leave one behind.
    fn exit_seen(&self, _witness: &ExitWitness) -> bool {
        true
    }

    /// The provider conversation already observed in a new terminal, when
    /// one raced ahead of the durable worker reseat.
    fn provider_session(&self, _term: u32) -> Option<zerocode_core::ProviderSession> {
        None
    }

    /// The live pane already holding this conversation, if one does
    /// (t-7812) — the judgment every resume door asks
    /// (`conversation_wake::claim_among`), put to the ledger's reseat
    /// before it cuts a pane. Test and tmux-only hosts hold none.
    fn conversation_standing(
        &self,
        _agent: &str,
        _session: &zerocode_core::ProviderSession,
    ) -> Option<u32> {
        None
    }

    /// Write the conversation a reseated pane IS into this window's pane
    /// table (t-7812), so a door asking before the agent's first report is
    /// told it stands there. Hosts without such a table keep nothing.
    fn carry_session(&self, _term: u32, _session: &zerocode_core::ProviderSession) {}

    /// The agent's OWN words about a quota wall in this pane — one of the
    /// two witnesses a `quota_walled` notice needs (`quota_wall.rs`).
    ///
    /// Asked only about a pane the stall probe already found quiet, and
    /// never under the team table: the real window copies the pane's grid
    /// and reads its transcript's tail here. Test and tmux-only hosts
    /// default to no observation rather than inventing a wall.
    fn quota_wall_marker(
        &self,
        _term: u32,
        _agent: &str,
    ) -> Option<zerocode_core::orchestration::QuotaWallMarker> {
        None
    }

    /// The wall this pane's own conversation last ended at, when it ended at
    /// one: a quota or a login that the next prompt would only meet again
    /// (t-6560, `quota_wall.rs`).
    ///
    /// Asked by the mail pointer at the moment it would type a fresh line,
    /// never once a beat: the real window reads the tail of the pane's
    /// transcript here. Test and tmux-only hosts default to no observation
    /// rather than inventing a wall.
    fn pane_wall(&self, _term: u32, _agent: &str) -> Option<crate::quota_wall::PaneWall> {
        None
    }

    /// Observe a quiet worker's own quota marker while holding the activity
    /// locks that can invalidate it. Call `commit` at most once, and keep the
    /// locks until it returns. This is asked after the actor dequeues terminal
    /// settlement; implementations must not call the actor or the team table.
    /// Hosts without an observation fence cannot authorize automatic stops.
    fn with_quota_wall_observation(
        &self,
        _term: u32,
        _worker_started_ms: i64,
        _agent: &str,
        _commit: &mut dyn FnMut(zerocode_core::orchestration::QuotaWallMarker),
    ) {
    }

    /// What this pane shows and records of a safety classifier's decline
    /// (t-6747, `quota_wall.rs`): its screen and its transcript's last
    /// record — the two witnesses a `classifier_declined` notice needs — and
    /// the switches of model its CLI recorded answering declines on a
    /// fallback, for `model_deviated` rows.
    ///
    /// Asked only about a pane the stall probe already found quiet, and
    /// never under the team table. Test and tmux-only hosts default to no
    /// observation rather than inventing a decline.
    fn classifier_decline_reading(
        &self,
        _term: u32,
        _agent: &str,
    ) -> Option<crate::quota_wall::DeclineReading> {
        None
    }

    /// The same reading, observed while holding the activity locks that can
    /// invalidate it — the decline's half of
    /// [`Self::with_quota_wall_observation`], for a handover's stop — with
    /// the moment the pane went quiet, which a pause dialog's witness is
    /// timed from. Quiet as [`Self::decline_quiet_since`] reads it, with
    /// [`zerocode_core::orchestration::DECLINE_DIALOG_UNANSWERED_MS`] as the
    /// hook's term. Call `commit` at most once, and keep the locks until it
    /// returns.
    fn with_classifier_decline_observation(
        &self,
        _term: u32,
        _worker_started_ms: i64,
        _agent: &str,
        _commit: &mut dyn FnMut(crate::quota_wall::DeclineReading, i64),
    ) {
    }

    /// Ask the window's usage gauge `gauge` to read its provider again, the
    /// way the status bar asks — never forced, so at most one read runs at a
    /// time, none inside its refetch floor and none while a failed read
    /// backs off (t-6427). Nothing waits on it: the answer lands in the
    /// cache the beat reads.
    ///
    /// The wait rung asks, from a held wall's reset on: the lift is judged
    /// by a number read after the reset, and the window's own poll stops
    /// while nobody looks at the window. Test and tmux-only hosts read
    /// nothing.
    fn ask_usage(&self, _gauge: &str) {}
    /// The managed login this pane was launched as, when the window recorded
    /// one (t-7538) — the wall witness judges the pane against THAT login's
    /// reading. `None` is "unknown", never "the selected one".
    fn pane_login(&self, _term: u32) -> Option<PaneLogin> {
        None
    }

    /// The account id of [`Self::pane_login`].
    fn pane_account(&self, term: u32) -> Option<String> {
        self.pane_login(term).map(|held| held.account)
    }

    /// Publish a restored worker only after its durable seat has moved. Fake
    /// hosts need no renderer surface and therefore default to doing nothing.
    fn announce_reseated_worker(
        &self,
        _term: u32,
        _worktree: &str,
        _agent: &str,
        _resumed: zerocode_core::orchestration::WorkerResume,
    ) {
    }

    /// The window's System One wire, for a question the beat asks Jev about
    /// a pane — the stall sweep's (t-4538): the key the settings pane keeps,
    /// the vendor's origin and zo's settings file, none of it read yet.
    ///
    /// `None` for a host that asks Jev nothing — tmux, and every test that is
    /// not about a Jev question — so a beat there never reaches a socket.
    fn jev_wire(&self) -> Option<crate::systemone::Wire> {
        None
    }

    /// Run `job` somewhere that does not hold the beat — work that waits on
    /// a socket for seconds, like a Jev question (t-4538). The window gives it
    /// a thread of its own; a host with no beat to protect runs it where it
    /// is, so a test sees what it did the moment the beat returns.
    fn off_the_beat(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }

    /// Which LAUNCH this terminal is currently holding, when the window
    /// recorded one.
    ///
    /// The pointer road asks, because it is the one hook road that CONSUMES:
    /// a pane id is reused, and an agent from an earlier launch finishing its
    /// last turn would otherwise collect the pointer meant for the one living
    /// there now. Every other road forwards the payload and lets identity be
    /// checked afterwards; this one has to check first.
    ///
    /// Defaulted to `None` for the same reason [`Host::agent_of`] is: a
    /// test or tmux-only host has no launch table, and `None` refuses nobody.
    fn launch_token_of(&self, _term: u32) -> Option<String> {
        None
    }

    /// Who the agent in this terminal is, for the purposes of a receipt.
    ///
    /// Asked of the host rather than handed in, so the ledger road can read it
    /// UNDER the same team-table guard it authorizes and plans in. Read before
    /// that guard and passed along, it is a fact that can go stale: a
    /// `respawn-pane` keeps the pane id and replaces the shell, the session,
    /// and the agent behind it, so a request authorized in one incarnation
    /// would file its receipt under another's name.
    ///
    /// Defaulted to `None` because most hosts have no idea: the tmux dialect
    /// does not need one, and a test host that does not answer is a test that
    /// is not about identity. `None` means "this pane has not said who it is",
    /// and the ledger road refuses mutations under it rather than filing them
    /// under nobody.
    ///
    /// The window's own implementation is the only one that knows
    /// (`TeamWindow::actor_for`), and even that one answers `None` until the
    /// agent has reported its session.
    fn actor_for(&self, _term: u32) -> Option<String> {
        None
    }

    /// Which agent runs in this terminal, as the catalog spells it.
    ///
    /// The mail pointer's one exception reads this: a `cursor` composer
    /// submits what lands in it without waiting for Enter, so the pointer is
    /// typed and the Enter withheld — Orca detects the same case by pane
    /// title, and a title is a guess where this window has the launch record.
    /// Defaulted to `None` for the same reason as [`Self::actor_for`]: most
    /// hosts have no idea, and "unknown" only costs the exception.
    fn agent_of(&self, _term: u32) -> Option<String> {
        None
    }
    /// Which checkout this terminal is sitting in, as the WINDOW recorded it
    /// at the launch — the pane's own `ZEROCODE_WORKTREE`, or the seat the
    /// ledger was told about.
    ///
    /// Asked by the evidence read for its bare form: the caller wants to know
    /// about the tree it is working in, and a coordinator's pane is nobody's
    /// worker row, so no ledger row can answer. Never a path the caller sent
    /// — this is the window's own record of where it put the pane. Hosts with
    /// no such record answer `None`, and the read says it could not resolve
    /// a checkout rather than guessing one.
    fn worktree_of(&self, _term: u32) -> Option<std::path::PathBuf> {
        None
    }
    /// Whether this pane is a shell with nothing running in front of it — its
    /// own interactive shell holding the terminal, the kernel's answer at the
    /// moment of asking.
    ///
    /// A pane's last turn ending at rest licenses the pointer only while an
    /// agent still holds the terminal. An agent a person started by hand in a
    /// shell can be quit after that turn, and no hook says so: a claude that
    /// reported first is spoken for by its hooks, and the foreground sweep
    /// never asks after a pane whose last word was `Done`. Advice typed then
    /// lands in the shell, which runs the backticks around `zerocode-orc
    /// check` as a command. Hosts that cannot ask — tmux, tests — answer
    /// `false`, which changes nothing.
    fn shell_in_front(&self, _term: u32) -> bool {
        false
    }
}

/// Why a spawned worker could not become usable, with its bounded last screen.
///
/// This is a host result rather than a log line so the coordinator that asked
/// for the worker receives the evidence. The screen is already the terminal's
/// visible grid, not scrollback or an unbounded process transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStartFailure {
    pub reason: String,
    pub screen: Option<String>,
    pub permanent: bool,
}

impl HostStartFailure {
    pub fn new(reason: impl Into<String>, screen: Option<String>) -> Self {
        Self {
            reason: reason.into(),
            screen: screen.filter(|screen| !screen.trim().is_empty()),
            permanent: false,
        }
    }

    pub fn permanent(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            screen: None,
            permanent: true,
        }
    }
}

impl std::fmt::Display for HostStartFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)?;
        if let Some(screen) = &self.screen {
            formatter.write_str("\nlast worker screen:\n")?;
            formatter.write_str(screen.trim_end())?;
        }
        Ok(())
    }
}

/* `PaneBinding` used to live here — team, pane and term carried together so
 * a settlement could say WHICH incarnation it settled. The actor's fact door
 * made the term itself the pin: `terminal_gone(term)` resolves the seat by
 * the terminal under the actor's own table lock, so a re-pointed seat
 * resolves to nothing instead of to the wrong incarnation, and there is no
 * triple left for a caller to carry. */

/// Carry out one `tmux …`, and say what the shim should print.
///
/// The order inside `Respawn` is deliberate and is the one measured
/// (:173169-173178): the replacement is started BEFORE the old shell is ended.
/// A pane closed first is a hole in the layout for as long as a spawn takes,
/// and a spawn that then fails leaves the leader with neither.
pub fn run(
    host: &dyn Host,
    team_id: &str,
    pane: &str,
    pane_token: &str,
    argv: &[String],
) -> zerocode_hookd::TeamAnswer {
    let mut held = teams();
    // Authorization shares the team guard with planning and the effect. A
    // precheck followed by a second lock would let a respawn rotate the token
    // in between and let the old request act on the new pane incarnation.
    let Some(team) = authorized_team_mut(&mut held, team_id, pane, pane_token) else {
        return answer(zerocode_core::agent_teams::Reply::refused(
            "stale or unauthorized agent pane",
        ));
    };
    let planned = plan(team, argv, pane);
    match planned.effect.clone() {
        Effect::None => answer(planned.reply),
        Effect::Split {
            pane: new_pane,
            from,
            direction,
            command,
            helper,
        } => {
            let Some(from_term) = team.term_of(&from) else {
                return answer(zerocode_core::agent_teams::Reply::refused(format!(
                    "unknown pane: {from}"
                )));
            };
            let (team_id, leader) = (team.id.clone(), team.leader_term);
            // A bare split is not a journaled effect — no ledger row rides on
            // it — so the token's only reader is the host that registers it.
            let Some(token) = crate::hooks::random_token() else {
                return answer(zerocode_core::agent_teams::Reply::refused(
                    "could not mint a capability for the new pane",
                ));
            };
            // Which helper the pane is, placed beside the capability for the
            // host to take at the spawn and say on `term:split`; swept with
            // this arm if the host refused and never took it.
            let _placed = helper
                .as_deref()
                .map(|id| SplitHelperAsk::place(&token, id));
            match host.split(
                &team_id, leader, from_term, &new_pane, direction, &command, &token,
            ) {
                Some(term) => {
                    team.record_split(&new_pane, term, &from, direction);
                    answer(planned.reply)
                }
                None => {
                    // The host's own sentence rides back to the shim, so the
                    // agent that asked reads WHY (an unattended login it
                    // could not confirm, a checkout git refused) instead of
                    // the bare refusal — measured 2026-09-06: a zo child pane
                    // was turned away with "could not open a pane" while the
                    // reason sat only in the window log.
                    let why = take_worker_host_failure(&token).map(|failure| failure.reason);
                    let said = match why {
                        Some(reason) => format!("could not open a pane: {reason}"),
                        None => "could not open a pane".to_string(),
                    };
                    answer(zerocode_core::agent_teams::Reply::refused(&said))
                }
            }
        }
        Effect::Respawn {
            pane: held_pane,
            from,
            direction,
            command,
            term: old,
        } => {
            let Some(from_term) = team.term_of(&from) else {
                return answer(zerocode_core::agent_teams::Reply::refused(format!(
                    "unknown pane: {from}"
                )));
            };
            let (team_id, leader) = (team.id.clone(), team.leader_term);
            let Some(token) = crate::hooks::random_token() else {
                return answer(zerocode_core::agent_teams::Reply::refused(
                    "could not mint a capability for the restarted pane",
                ));
            };
            match host.split(
                &team_id, leader, from_term, &held_pane, direction, &command, &token,
            ) {
                Some(term) => {
                    /* The old dispatch is settled BEFORE the seat is re-pointed.
                     *
                     * `respawn_pane` replaces what sits behind a pane id and
                     * keeps the id. The ledger addresses its workers by
                     * `(team, pane)`, so after the re-point the seat means the
                     * new shell — and the old worker would be unreachable by
                     * any road, its dispatch open forever, holding a standing
                     * order's ceiling for an attempt that had ended.
                     *
                     * The settlement walks through the actor now, which
                     * resolves the seat by the OLD terminal under its own
                     * table lock — so this guard is given back first: a
                     * caller that held it while waiting on the actor's
                     * mailbox would deadlock the window. The pin is the TERM:
                     * anything that re-points the seat inside this gap makes
                     * the old term resolve to no seat, and nothing settles —
                     * the wrong incarnation cannot be settled, only possibly
                     * none.
                     */
                    drop(held);
                    crate::orchestration::seat_left_its_terminal(old, crate::now_epoch_ms());
                    /* And the seat is re-pointed only if it still means the
                     * OLD shell. A pane that moved in the gap keeps its
                     * current occupant — and the shell this road just spawned
                     * is closed rather than leaked beside it. */
                    let repointed = {
                        let mut retaken = teams();
                        match retaken.get_mut(&team_id) {
                            Some(team) if team.term_of(&held_pane) == Some(old) => {
                                team.respawn_pane(&held_pane, term);
                                true
                            }
                            _ => false,
                        }
                    };
                    if !repointed {
                        host.close(term);
                        return answer(zerocode_core::agent_teams::Reply::refused(
                            "the pane changed while it was being restarted — ask again",
                        ));
                    }
                    /* TODO — the Host's own failure is still unrecorded.
                     *
                     * `close` answers nothing, so a window that could not end
                     * the old shell leaves a ledger saying the attempt is over
                     * and a terminal that is still there. That is a real
                     * `Unknown`, and it is deliberately NOT papered over with
                     * an extra state here: it needs a durable record of the
                     * effect written before the effect is attempted — the
                     * lifecycle lanes (`durable_lifecycle`) are that record,
                     * and wiring them is the lifecycle cutover slice.
                     */
                    host.close(old);
                    answer(planned.reply)
                }
                None => answer(zerocode_core::agent_teams::Reply::refused(
                    "could not restart the pane",
                )),
            }
        }
        Effect::Send { term, text } => {
            if host.send(term, &text) {
                answer(planned.reply)
            } else {
                answer(zerocode_core::agent_teams::Reply::refused("pane is gone"))
            }
        }
        // The tmux dialect plans keys, never documents — a paste here is a
        // planner bug. Said rather than silently delivered, because the one
        // thing this road must not do is hand a terminal unsanitized prose.
        Effect::Paste { .. } => answer(zerocode_core::agent_teams::Reply::refused(
            "the tmux road does not paste",
        )),
        Effect::Capture { term, .. } => match host.capture(term) {
            Some(tail) => answer(capture_reply(&planned, &tail)),
            None => answer(zerocode_core::agent_teams::Reply::refused("pane is gone")),
        },
        // A read by ledger seat is the orchestration road's; tmux names panes
        // in its own table only, and its planner never asks for this. A
        // checkout's evidence is the same road's for the same reason: tmux
        // knows nothing about worktrees.
        Effect::CaptureSeat { .. }
        | Effect::WorkerTerminal { .. }
        | Effect::WorktreeEvidence { .. }
        | Effect::WorkerTranscript { .. } => answer(zerocode_core::agent_teams::Reply::refused(
            "the tmux road reads its own panes only",
        )),
        Effect::Focus { term } => {
            host.focus(term);
            answer(planned.reply)
        }
        Effect::Close {
            pane: gone_pane,
            term,
        } => {
            /* The settlement comes before `remove_pane`, for the same reason
             * as ever: after the removal the ledger's worker sits at a seat
             * the team no longer has, so nothing can settle it and nothing
             * can address it. The actor resolves the seat by THIS term under
             * its own table lock — the term scopes it to the right team even
             * though two teams in one window each have a `%2` — so this
             * guard is given back first, same deadlock rule as Respawn.
             *
             * `kill-pane` refuses the leader pane (`agent_teams.rs` in core),
             * so this road cannot dissolve a team; the leader's own death is
             * `orchestration::terminal_gone`, which does. That refusal is
             * pinned, so if it ever softens this arm is not silently the wrong
             * shape.
             */
            let team_id = team.id.clone();
            drop(held);
            crate::orchestration::seat_left_its_terminal(term, crate::now_epoch_ms());
            /* The pane leaves the table only while it still means THIS term —
             * a respawn that lands in the gap keeps its replacement, and the
             * mismatch refuses WITHOUT closing the new occupant's shell. */
            let removed = {
                let mut retaken = teams();
                match retaken.get_mut(&team_id) {
                    Some(team) if team.term_of(&gone_pane) == Some(term) => {
                        team.remove_pane(&gone_pane);
                        true
                    }
                    _ => false,
                }
            };
            if !removed {
                return answer(zerocode_core::agent_teams::Reply::refused(
                    "the pane changed while it was being closed — ask again",
                ));
            }
            host.close(term);
            answer(planned.reply)
        }
    }
}

fn answer(reply: zerocode_core::agent_teams::Reply) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: reply.stdout,
        stderr: reply.stderr,
        exit_code: reply.exit_code,
    }
}

/// The environment a teammate's own shell carries: its team's, with the pane
/// id swapped for its own.
///
/// Without the swap every teammate would report as the leader, and the first
/// `split-window` one of them ran would cut the leader's pane instead of its
/// own.
pub fn teammate_env(
    team_id: &str,
    pane: &str,
    pane_token: &str,
    leader_env: &[(String, String)],
) -> Vec<(String, String)> {
    // Both names, and only the ones the leader actually had. A ledger team has
    // no `TMUX_PANE` to swap, and adding one here would hand a teammate the
    // multiplexer its leader was deliberately not given.
    let had_tmux = leader_env.iter().any(|(key, _)| key == "TMUX_PANE");
    // Nor the window's router keys a zo leader was handed: they ride a zo
    // launch only, and a zo teammate is handed its own through the launch
    // door (`hooks::agent_launch_env_with_lock`) — a codex, claude or gemini
    // one never reads the table they are named in.
    let mut env: Vec<(String, String)> = leader_env
        .iter()
        .filter(|(key, _)| {
            key != "TMUX_PANE"
                && key != TEAM_PANE_VAR
                && key != TEAM_TOKEN_VAR
                && key != zerocode_hookd::env_var::TEAM_TOKEN_FILE
                && !crate::api_routers::is_router_env(key)
        })
        .cloned()
        .collect();
    if had_tmux {
        env.push(("TMUX_PANE".into(), pane.to_string()));
    }
    env.push((TEAM_PANE_VAR.into(), pane.to_string()));
    if let Some(path) = ensure_team_token_file(team_id, pane, pane_token) {
        env.push((
            zerocode_hookd::env_var::TEAM_TOKEN_FILE.into(),
            path.to_string_lossy().into_owned(),
        ));
    } else {
        // A filesystem failure must not silently kill Agent Teams. This is the
        // deliberately documented degraded fallback; the normal path above
        // remains value-free.
        env.push((TEAM_TOKEN_VAR.into(), pane_token.to_string()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A window that records instead of drawing.
    ///
    /// `base` is what keeps these tests apart. The team table is a process-wide
    /// `static` — it has to be, a team belongs to no workspace — so two tests
    /// running at once share it, and `forget_term` takes a shell number out of
    /// EVERY team. Numbering each test's shells from its own thousand is the
    /// cheap half of the fix; the expensive half would be a lock nothing in
    /// production needs.
    struct Recorder {
        base: u32,
        made: RefCell<Vec<(u32, String, Direction, String)>>,
        typed: RefCell<Vec<(u32, String)>>,
        focused: RefCell<Vec<u32>>,
        closed: RefCell<Vec<u32>>,
        /// Which helper each split said it was — what the production host
        /// takes at the spawn and rides out on `term:split`.
        helpers: RefCell<Vec<Option<String>>>,
        next: RefCell<u32>,
        refuse: bool,
        /// What a refusing host says, the way the window's host does when an
        /// unattended login cannot be confirmed or git turns a checkout down.
        refusal_reason: Option<String>,
        /// The production window's close: it walks back into `forget_term`.
        /// The deadlock regression test is the only caller.
        forgetful: bool,
    }

    impl Recorder {
        fn new(base: u32) -> Self {
            Self {
                base,
                made: RefCell::new(Vec::new()),
                typed: RefCell::new(Vec::new()),
                focused: RefCell::new(Vec::new()),
                closed: RefCell::new(Vec::new()),
                helpers: RefCell::new(Vec::new()),
                next: RefCell::new(0),
                refuse: false,
                refusal_reason: None,
                forgetful: false,
            }
        }

        fn refusing(base: u32) -> Self {
            Self {
                refuse: true,
                ..Self::new(base)
            }
        }

        fn refusing_because(base: u32, reason: &str) -> Self {
            Self {
                refuse: true,
                refusal_reason: Some(reason.to_string()),
                ..Self::new(base)
            }
        }

        fn forgetful(base: u32) -> Self {
            Self {
                forgetful: true,
                ..Self::new(base)
            }
        }
    }

    impl Host for Recorder {
        fn split(
            &self,
            _team: &str,
            _leader: u32,
            from_term: u32,
            pane: &str,
            direction: Direction,
            command: &str,
            token: &str,
        ) -> Option<u32> {
            if self.refuse {
                if let Some(reason) = &self.refusal_reason {
                    place_worker_host_failure(token, HostStartFailure::new(reason.clone(), None));
                }
                return None;
            }
            self.helpers.borrow_mut().push(take_split_helper(token));
            self.made.borrow_mut().push((
                from_term,
                pane.to_string(),
                direction,
                command.to_string(),
            ));
            let mut next = self.next.borrow_mut();
            *next += 1;
            Some(self.base + *next)
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.typed.borrow_mut().push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            Some("teammate said this".to_string())
        }
        fn focus(&self, term: u32) -> bool {
            self.focused.borrow_mut().push(term);
            true
        }
        fn close(&self, term: u32) {
            self.closed.borrow_mut().push(term);
            if self.forgetful {
                super::forget_term(term);
            }
        }
        /// Who is sitting there, which the ledger road now requires before it
        /// will let a pane change anything.
        ///
        /// The default answers `None` — a window that has not heard from its
        /// agent yet — and a pane with no identity is refused every mutation,
        /// which is the shipped behaviour and not something to work around.
        /// What a test host owes is an ANSWER, so the road it is driving is the
        /// one a seated agent walks.
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(format!("actor-of-term-{term}"))
        }
    }

    /// A team whose leader is `base`, and whose teammates will be `base + 1`
    /// upward. One base per test — see [`Recorder`].
    fn open(base: u32) -> (String, Recorder) {
        let id = format!("team-test-{base}");
        teams().insert(id.clone(), Team::new(id.clone(), "tok", base));
        let _ = remember_pane_token(&id, zerocode_core::agent_teams::LEADER_PANE, "tok".into());
        (id, Recorder::new(base))
    }

    /// Most protocol tests speak as the leader. Keep their prose focused on
    /// the behavior under test while production still requires the explicit
    /// per-pane capability at the boundary.
    fn run(
        host: &dyn Host,
        team_id: &str,
        pane: &str,
        argv: &[String],
    ) -> zerocode_hookd::TeamAnswer {
        super::run(host, team_id, pane, "tok", argv)
    }

    fn authorizes(team_id: &str, pane: &str, presented: &str) -> bool {
        authorized_team_mut(&mut teams(), team_id, pane, presented).is_some()
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_small_host_gets_existence_from_its_capture_answer() {
        let host = Recorder::new(900);
        assert!(host.pane_exists(901));
    }

    #[test]
    fn four_teammates_become_four_panes_and_the_leader_is_cut_once() {
        // The reported break, end to end: an orchestrating agent asks four
        // times and four panes exist, in the measured shape — the leader is
        // divided ONCE and the rest stack in the column beside it.
        let (id, host) = open(1000);
        for _ in 0..4 {
            let answer = run(&host, &id, "%1", &words("split-window -d -h -P claude"));
            assert_eq!(answer.exit_code, 0, "{}", answer.stderr);
        }
        let made = host.made.borrow().clone();
        assert_eq!(made.len(), 4, "an ask did not become a pane");
        // First cuts the leader side by side; the other three stack onto the
        // bottom of the column that first one made.
        assert_eq!(made[0].0, 1000);
        assert_eq!(made[0].2, Direction::Vertical);
        assert_eq!(made[1].0, 1001, "the second teammate re-cut the leader");
        assert_eq!(made[1].2, Direction::Horizontal);
        assert_eq!(made[2].0, 1002);
        assert_eq!(made[3].0, 1003);
        assert!(made.iter().all(|one| one.3 == "claude"));
        // Ids counted up, and each pane was told its own.
        assert_eq!(
            made.iter().map(|one| one.1.clone()).collect::<Vec<_>>(),
            ["%2", "%3", "%4", "%5"]
        );
        // Nothing about opening a teammate moved the keyboard.
        assert!(host.focused.borrow().is_empty());
        forget_term(1000);
    }

    #[test]
    fn a_pane_that_could_not_be_opened_is_not_written_down() {
        // Otherwise the leader would address a pane that does not exist, and
        // every later command about it would be answered about nothing.
        let (id, _) = open(2000);
        let host = Recorder::refusing(2000);
        let answer = run(&host, &id, "%1", &words("split-window -h claude"));
        assert_eq!(answer.exit_code, 1);
        assert_eq!(answer.stderr, "tmux: could not open a pane\n");
        let listed = run(&host, &id, "%1", &words("list-panes"));
        assert_eq!(listed.stdout, "%1\n");
        forget_term(2000);
    }

    /// One identity, one row (t-3024): the window folds a helper's hook row
    /// into its pane row by the `helper` key on `term:split`. The key is
    /// present on every split — `null` when the split named none — so the
    /// listener reads one shape and a missing key can never be mistaken for
    /// a helper. Tested here rather than beside the struct: the runtime file
    /// owns no test fence (`the_shipped_half_of_every_backend_file_ends_where_its_tests_begin`).
    #[test]
    fn a_split_announcement_names_its_helper() {
        use crate::agent_tools_runtime::TermSplit;
        let said = serde_json::to_value(TermSplit {
            parent: 1,
            term: 2,
            direction: "vertical",
            agent: Some("zo".to_string()),
            helper: Some("agent-1757".to_string()),
        })
        .expect("a split announcement serialises");
        assert_eq!(said["parent"], 1);
        assert_eq!(said["term"], 2);
        assert_eq!(said["direction"], "vertical");
        assert_eq!(said["agent"], "zo");
        assert_eq!(said["helper"], "agent-1757");

        let bare = serde_json::to_value(TermSplit {
            parent: 1,
            term: 3,
            direction: "horizontal",
            agent: Some("claude".to_string()),
            helper: None,
        })
        .expect("a bare split serialises");
        assert!(
            bare.get("helper").is_some_and(serde_json::Value::is_null),
            "a split that names no helper still carries the key: {bare}"
        );
    }

    /// One identity, one row (t-3024): the helper id a zo split names
    /// reaches the host that cuts the pane, on the same capability the
    /// spawn registers, so `term:split` can carry it. A split that names
    /// none hands none; a refused split leaves nothing behind.
    #[test]
    fn a_zo_split_hands_the_host_the_helper_it_named() {
        let (id, host) = open(2200);
        let named = run(
            &host,
            &id,
            "%1",
            &words("split-window -h -e ZO_AGENT_ID=agent-9 -- zo --teammate /tmp/x"),
        );
        assert_eq!(named.exit_code, 0, "{}", named.stderr);
        run(&host, &id, "%1", &words("split-window -h claude"));
        assert_eq!(
            *host.helpers.borrow(),
            vec![Some("agent-9".to_string()), None],
            "the host did not hear which helper the pane is"
        );
        // The pair is tmux's, not zo's: the command the host ran is bare.
        let made = host.made.borrow();
        assert_eq!(made[0].3, "zo --teammate /tmp/x");
        drop(made);
        // Placed and swept: a refusal never takes it, and the guard clears it.
        let placed = SplitHelperAsk::place("tok-t3024", "agent-1");
        drop(placed);
        assert_eq!(take_split_helper("tok-t3024"), None);
        forget_term(2200);
    }

    /// The reason a host gives for refusing rides back to the shim, so the
    /// agent that asked reads it — not the bare "could not open a pane" that
    /// left the window log as the only place the sentence existed.
    #[test]
    fn a_refused_pane_names_the_reason_the_host_gave() {
        let (id, _) = open(2100);
        let host = Recorder::refusing_because(
            2100,
            "an unattended worker cannot start: zo의 선택된 런타임 로그인 상태를 확인하지 못했습니다",
        );
        let answer = run(
            &host,
            &id,
            "%1",
            &words("split-window -h zo --teammate /tmp/x"),
        );
        assert_eq!(answer.exit_code, 1);
        assert_eq!(
            answer.stderr,
            "tmux: could not open a pane: an unattended worker cannot start: zo의 선택된 런타임 로그인 상태를 확인하지 못했습니다\n"
        );
        // Spent by reading: the next refusal without a reason is bare again.
        let bare = run(
            &Recorder::refusing(2100),
            &id,
            "%1",
            &words("split-window -h zo"),
        );
        assert_eq!(bare.stderr, "tmux: could not open a pane\n");
        forget_term(2100);
    }

    #[test]
    fn the_leader_dying_takes_its_team_and_a_teammate_dying_takes_one_row() {
        let (id, host) = open(3000);
        run(&host, &id, "%1", &words("split-window -h claude"));
        run(&host, &id, "%1", &words("split-window -h claude"));
        assert_eq!(
            run(&host, &id, "%1", &words("list-panes")).stdout,
            "%1\n%2\n%3\n"
        );
        // A teammate's shell ends: its row goes, the team stays. That is the
        // distinction — a team thrown away here would leave the leader unable
        // to learn which of its teammates had stopped.
        forget_term(3001);
        assert_eq!(
            run(&host, &id, "%1", &words("list-panes")).stdout,
            "%1\n%3\n"
        );
        // The leader's does: nothing is left to answer at all.
        forget_term(3000);
        let after = run(&host, &id, "%1", &words("list-panes"));
        assert_eq!(after.exit_code, 1);
        assert_eq!(after.stderr, "tmux: stale or unauthorized agent pane\n");
    }

    #[test]
    fn one_team_cannot_reach_another_teams_panes() {
        // Two leaders running side by side is the ordinary case, and a pane
        // id is a small integer both of them have.
        let (mine, host) = open(4000);
        let (yours, _) = open(5000);
        run(&host, &mine, "%1", &words("split-window -h claude"));
        // `%2` exists — in the OTHER team. Asked for here it is unknown, and
        // asking AS it is unknown too.
        let reached = run(
            &host,
            &yours,
            "%1",
            &words("send-keys -t %2 rm -rf / Enter"),
        );
        assert_eq!(reached.exit_code, 1);
        assert_eq!(reached.stderr, "tmux: unknown pane: %2\n");
        let impersonated = run(&host, &yours, "%2", &words("list-panes"));
        assert_eq!(
            impersonated.stderr,
            "tmux: stale or unauthorized agent pane\n"
        );
        assert!(host.typed.borrow().is_empty(), "a key reached another team");
        forget_term(4000);
        forget_term(5000);
    }

    #[test]
    fn a_stale_pane_capability_cannot_reach_an_effect() {
        let (id, host) = open(4_500);
        let refused = super::run(
            &host,
            &id,
            zerocode_core::agent_teams::LEADER_PANE,
            "old-or-sibling-token",
            &words("split-window -h claude"),
        );
        assert_eq!(refused.exit_code, 1);
        assert_eq!(refused.stderr, "tmux: stale or unauthorized agent pane\n");
        assert!(host.made.borrow().is_empty(), "an unauthorized split ran");
        assert!(
            teams()
                .get(&id)
                .is_some_and(|team| team.panes().count() == 1),
            "an unauthorized split changed the table"
        );
        forget_term(4_500);
    }

    #[test]
    fn the_leader_reads_and_types_and_only_a_named_pane_takes_the_keyboard() {
        let (id, host) = open(6000);
        run(&host, &id, "%1", &words("split-window -h claude"));
        let typed = run(
            &host,
            &id,
            "%1",
            &words("send-keys -t %2 look at src Enter"),
        );
        assert_eq!(typed.exit_code, 0);
        assert_eq!(
            host.typed.borrow().clone(),
            [(6001, "look at src\r".to_string())]
        );
        let read = run(&host, &id, "%1", &words("capture-pane -p -t %2"));
        assert_eq!(read.stdout, "teammate said this\n");
        // And the one road to the keyboard.
        assert!(host.focused.borrow().is_empty());
        run(&host, &id, "%1", &words("select-pane -t %2"));
        assert_eq!(host.focused.borrow().clone(), [6001]);
        forget_term(6000);
    }

    #[test]
    fn a_restart_starts_the_replacement_before_it_ends_the_old_one() {
        // The order is the whole point: closed-first is a hole in the layout
        // for as long as a spawn takes, and a spawn that fails leaves neither.
        let (id, host) = open(7000);
        run(&host, &id, "%1", &words("split-window -h claude"));
        let answer = run(&host, &id, "%1", &words("respawn-pane -k -t %2 claude"));
        assert_eq!(answer.exit_code, 0, "{}", answer.stderr);
        assert_eq!(host.made.borrow().len(), 2);
        assert_eq!(host.closed.borrow().clone(), [7001]);
        // The pane kept its name and now stands on the new shell.
        let typed = run(&host, &id, "%1", &words("send-keys -t %2 hi Enter"));
        assert_eq!(typed.exit_code, 0);
        assert_eq!(host.typed.borrow().last().expect("typed").0, 7002);
        forget_term(7000);
    }

    #[test]
    fn a_close_leaves_the_table_before_it_ends_a_shell() {
        // The window's own close reaches back into `forget_term` — that is
        // production, not a test contrivance, and calling it under the table
        // lock froze every thread that touches a terminal (2026-08-17). Both
        // closing arms must have let go of the lock first; if either takes
        // it back this test never returns.
        let (id, _) = open(8000);
        let host = Recorder::forgetful(8000);
        run(&host, &id, "%1", &words("split-window -h claude"));
        let restarted = run(&host, &id, "%1", &words("respawn-pane -k -t %2 claude"));
        assert_eq!(restarted.exit_code, 0, "{}", restarted.stderr);
        assert_eq!(host.closed.borrow().clone(), [8001]);
        let killed = run(&host, &id, "%1", &words("kill-pane -t %2"));
        assert_eq!(killed.exit_code, 0, "{}", killed.stderr);
        assert_eq!(host.closed.borrow().clone(), [8001, 8002]);
        forget_term(8000);
    }

    /// A stop the ledger wrote down is a terminal that actually ends.
    ///
    /// `worker-stop` spends the attempt and marks the worker released BEFORE
    /// the window is asked to do anything — that ordering is deliberate, and it
    /// is why the window failing afterwards is not a harmless refusal. Until
    /// this landed the ledger road had no `Close` arm at all: the record said
    /// released, the agent read `this window cannot carry out Close`, and the
    /// pane stayed open with a worker still in it. The two halves of the same
    /// fact disagreed, and only one of them was on screen.
    ///
    /// Lives beside the tmux deadlock test and uses the same forgetful host on
    /// purpose: the window's own close reaches back into `forget_term`, which
    /// takes the table this road holds. If the arm ever takes that lock back,
    /// this test does not fail — it never returns.
    #[test]
    fn a_stopped_worker_loses_the_terminal_the_ledger_says_it_lost() {
        let _window = crate::orchestration::tests::the_window();
        let (id, host) = open(8100);
        let now = 5_000;
        let orc = |host: &Recorder, line: &str, at: i64| {
            // The leader's capability, presented the way a real request does:
            // the ledger road proves it under the same guard it plans in, so
            // there is no signature left that lets a test skip it. And the
            // retry name the ledger road now requires of anything that changes
            // it — asked for rather than assumed, so this line does not carry a
            // second copy of that verb list.
            let mut argv = words(line);
            if zerocode_core::orchestration::needs_a_retry_name(&argv) {
                argv.push("--retry-request".to_string());
                argv.push(format!("team-test-{at}"));
            }
            crate::orchestration::run(host, Vec::new(), &id, "%1", "tok", &argv, at)
        };

        let made = orc(&host, "run-create --name stopping", now);
        assert_eq!(made.exit_code, 0, "{}", made.stderr);
        let task: serde_json::Value =
            serde_json::from_str(&orc(&host, "task-create --spec migrate", now + 1).stdout)
                .expect("a task");
        let task_id = task["taskId"].as_str().expect("an id").to_string();

        let started: serde_json::Value = serde_json::from_str(
            &orc(
                &host,
                &format!("worker-start --agent claude --task {task_id}"),
                now + 2,
            )
            .stdout,
        )
        .expect("a worker");
        let worker = started["workerId"].as_str().expect("an id").to_string();
        // The worker's own terminal, read where the two vocabularies meet: the
        // ledger answered with a pane id, and only the table knows its term.
        let seat = started["pane"].as_str().expect("a pane").to_string();
        let term = teams()
            .get(&id)
            .expect("the team")
            .term_of(&seat)
            .expect("the pane was never seated");
        assert!(host.closed.borrow().is_empty());

        let stopped = orc(&host, &format!("worker-stop --worker {worker}"), now + 3);
        assert_eq!(
            stopped.exit_code, 0,
            "the window refused a stop the ledger had already written: {}",
            stopped.stderr
        );
        assert_eq!(
            host.closed.borrow().clone(),
            [term],
            "the ledger spent the attempt and the terminal stayed open"
        );
        // And the pane is out of the table, or the leader's `list-panes` keeps
        // naming a window that is gone.
        assert!(
            teams()
                .get(&id)
                .expect("the team")
                .panes()
                .all(|pane| pane.term != term),
            "a closed pane is still in the team table"
        );
        forget_term(8100);
    }

    #[test]
    fn a_teammate_carries_its_own_pane_id_and_nothing_of_the_leaders() {
        let held = |env: &[(String, String)], name: &str| {
            env.iter()
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>()
        };
        let leader = team_launch_env("team-x", "tok", &["/shim"], "/usr/bin", "", "claude");
        let teammate = teammate_env("team-x", "%4", "pane-4-secret", &leader);
        assert_eq!(held(&teammate, "TMUX_PANE"), ["%4"]);
        assert_eq!(held(&teammate, TEAM_PANE_VAR), ["%4"]);
        assert!(held(&teammate, TEAM_TOKEN_VAR).is_empty());
        let teammate_token_file = held(&teammate, zerocode_hookd::env_var::TEAM_TOKEN_FILE);
        assert_eq!(teammate_token_file.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&teammate_token_file[0]).expect("teammate token file"),
            "pane-4-secret"
        );
        assert_ne!(teammate_token_file, held(&leader, TEAM_TOKEN_VAR));
        // Exactly once each — a second one left behind would be read by
        // whichever the environment happened to keep.
        assert_eq!(held(&teammate, "TMUX_PANE").len(), 1);
        assert_eq!(held(&teammate, TEAM_PANE_VAR).len(), 1);
        // Everything else about the team travels unchanged, or the teammate
        // could not reach the bridge to open a pane of its own.
        assert_eq!(
            held(&teammate, zerocode_core::agent_teams::TEAM_ID_VAR),
            held(&leader, zerocode_core::agent_teams::TEAM_ID_VAR)
        );
        assert_eq!(held(&teammate, "TMUX"), held(&leader, "TMUX"));

        // A worker under a leader that was never given a multiplexer is not
        // given one either. It still knows which pane it is — in the name that
        // does not claim tmux — which is the whole reason that name exists.
        let ledger = team_launch_env("team-y", "tok", &["/orc"], "/usr/bin", "", "codex");
        let worker = teammate_env("team-y", "%7", "pane-7-secret", &ledger);
        assert_eq!(held(&worker, TEAM_PANE_VAR), ["%7"]);
        assert!(held(&worker, TEAM_TOKEN_VAR).is_empty());
        let worker_token_file = held(&worker, zerocode_hookd::env_var::TEAM_TOKEN_FILE);
        assert_eq!(worker_token_file.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&worker_token_file[0]).expect("worker token file"),
            "pane-7-secret"
        );
        assert!(
            held(&worker, "TMUX_PANE").is_empty(),
            "a ledger worker was handed a TMUX_PANE its leader never had"
        );
        assert!(held(&worker, "TMUX").is_empty());
    }

    #[test]
    fn a_teammate_does_not_export_its_pane_secret_to_its_children() {
        let leader = team_launch_env(
            "team-secret-file",
            "leader-secret",
            &[],
            "/usr/bin",
            "",
            "codex",
        );
        let teammate = teammate_env(
            "team-secret-file",
            "%2",
            "child-secret-must-not-be-in-env",
            &leader,
        );
        assert!(
            teammate.iter().all(|(name, _)| name != TEAM_TOKEN_VAR),
            "the pane secret was still exported directly"
        );
        assert!(
            teammate
                .iter()
                .any(|(name, _)| { name == zerocode_hookd::env_var::TEAM_TOKEN_FILE }),
            "the child had no file-only pane credential"
        );
    }

    /// A zo leader carries the window's router keys; a teammate copied from it
    /// must not. A codex, claude or gemini teammate never reads `providers[]`,
    /// so a key it inherits is exposure with no reader — and a zo teammate is
    /// handed its own through the launch door (`agent_launch_env_with_lock`).
    #[test]
    fn a_teammate_does_not_inherit_the_leaders_router_keys() {
        let mut leader = team_launch_env("team-router", "tok", &[], "/usr/bin", "", "zo");
        leader.push((
            "ZEROCODE_ROUTER_OPENROUTER_KEY".to_string(),
            "sk-or-dummy".to_string(),
        ));
        leader.push(("ZO_HOME".to_string(), "/zo-home".to_string()));
        let teammate = teammate_env("team-router", "%3", "pane-3-secret", &leader);
        assert!(
            teammate
                .iter()
                .all(|(name, _)| !crate::api_routers::is_router_env(name)),
            "a teammate inherited the leader's router key: {teammate:?}"
        );
        assert!(
            teammate
                .iter()
                .any(|(name, value)| name == "ZO_HOME" && value == "/zo-home"),
            "the rest of the leader's environment stopped travelling"
        );
    }

    #[test]
    fn one_team_member_cannot_speak_as_its_sibling() {
        let id = format!("team-pane-capability-{}", line!());
        let leader_term = 82_000 + line!();
        let child_term = leader_term + 1;
        let host = Recorder::new(leader_term);
        let mut team = Team::new(&id, "leader-secret", leader_term);
        team.record_split(
            "%2",
            child_term,
            zerocode_core::agent_teams::LEADER_PANE,
            Direction::Vertical,
        );
        teams().insert(id.clone(), team);
        let _ = remember_pane_token(
            &id,
            zerocode_core::agent_teams::LEADER_PANE,
            "leader-secret".to_string(),
        );
        let _ = remember_pane_token(&id, "%2", "child-secret".to_string());

        assert!(authorizes(
            &id,
            zerocode_core::agent_teams::LEADER_PANE,
            "leader-secret"
        ));
        assert!(authorizes(&id, "%2", "child-secret"));
        assert!(!authorizes(&id, "%2", "leader-secret"));
        assert!(!authorizes(
            &id,
            zerocode_core::agent_teams::LEADER_PANE,
            "child-secret"
        ));
        assert!(!authorizes(&id, "%2", ""));
        assert!(!authorizes(&id, "%9", "child-secret"));
        let forged = super::run(
            &host,
            &id,
            "%2",
            "leader-secret",
            &words("send-keys -t %1 forged Enter"),
        );
        assert_eq!(forged.exit_code, 1);
        assert!(host.typed.borrow().is_empty(), "a sibling token typed");

        // A successful respawn rotates the capability. An already queued old
        // request reaches the locked check after the rotation and cannot act
        // on the replacement pane.
        let previous = remember_pane_token(&id, "%2", "replacement-secret".to_string());
        assert!(authorizes(&id, "%2", "replacement-secret"));
        let stale = super::run(
            &host,
            &id,
            "%2",
            "child-secret",
            &words("send-keys -t %1 stale Enter"),
        );
        assert_eq!(stale.exit_code, 1);
        let fresh = super::run(
            &host,
            &id,
            "%2",
            "replacement-secret",
            &words("send-keys -t %1 fresh Enter"),
        );
        assert_eq!(fresh.exit_code, 0, "{}", fresh.stderr);
        assert_eq!(
            host.typed.borrow().as_slice(),
            [(leader_term, "fresh\r".into())]
        );

        // A spawn that failed restores the old capability and rejects the one
        // that belonged to the child which never started.
        restore_pane_token(&id, "%2", previous);
        assert!(authorizes(&id, "%2", "child-secret"));
        assert!(!authorizes(&id, "%2", "replacement-secret"));
        let never_started = super::run(
            &host,
            &id,
            "%2",
            "replacement-secret",
            &words("send-keys -t %1 never Enter"),
        );
        assert_eq!(never_started.exit_code, 1);

        forget_term(child_term);
        assert!(!authorizes(&id, "%2", "child-secret"));
        let revoked = super::run(
            &host,
            &id,
            "%2",
            "child-secret",
            &words("send-keys -t %1 after-death Enter"),
        );
        assert_eq!(revoked.exit_code, 1);
        assert_eq!(host.typed.borrow().len(), 1, "a dead team still typed");

        assert!(authorizes(
            &id,
            zerocode_core::agent_teams::LEADER_PANE,
            "leader-secret"
        ));
        forget_term(leader_term);
        assert!(!authorizes(
            &id,
            zerocode_core::agent_teams::LEADER_PANE,
            "leader-secret"
        ));
    }

    #[test]
    fn a_window_with_the_feature_off_opens_no_team_at_all() {
        let local_data = tempfile::tempdir().expect("no local data dir");
        // The default. Nothing is written to disk, nothing is registered, and
        // the agent's environment is the one it always had — for every agent,
        // not only the one that speaks tmux.
        for program in ["claude", "codex"] {
            assert!(
                open_team(local_data.path(), 600, TeamsMode::Off, "/usr/bin", program).is_empty()
            );
            assert!(
                open_team(
                    local_data.path(),
                    600,
                    TeamsMode::InProcess,
                    "/usr/bin",
                    program
                )
                .is_empty()
            );
        }
    }

    #[test]
    fn the_team_shim_is_written_below_the_injected_local_data_root() {
        let local_data = tempfile::tempdir().expect("no local data dir");
        let dirs = install_shim(local_data.path()).expect("install shim");
        assert_eq!(dirs.len(), SHIMS.len());
        for (dir, (folder, name, voice)) in dirs.iter().zip(SHIMS) {
            assert_eq!(dir, &local_data.path().join(folder));
            let path = dir.join(name);
            assert!(path.is_file(), "{name} was not written");
            let said = std::fs::read_to_string(&path).expect("read the shim");
            // Each name refuses under its own. A `zerocode-orc` that said
            // `tmux:` would send an agent looking for a multiplexer it never
            // invoked; a `tmux` that said `orchestration:` would be a word
            // Claude's Agent Teams has never heard.
            assert!(
                said.contains(&format!("{voice}: could not reach the window")),
                "{name} refuses in the wrong voice"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
                assert_eq!(mode & 0o111, 0o111, "{name} is not executable");
            }
        }
        // One script, two names — a second script would be a second thing to
        // keep in step with the bridge.
        let scripts: std::collections::HashSet<String> = dirs
            .iter()
            .zip(SHIMS)
            .map(|(dir, (_, name, _))| {
                std::fs::read_to_string(dir.join(name))
                    .expect("read")
                    .replace("tmux:", "")
                    .replace("orchestration:", "")
            })
            .collect();
        assert_eq!(scripts.len(), 1, "the two shims drifted apart");

        // And a directory each: the tmux shim must not be reachable from the
        // path a ledger-only leader is given.
        let ledger_only = dirs_for(Dialect::Ledger, &dirs);
        assert_eq!(ledger_only.len(), 1, "{ledger_only:?}");
        assert!(
            !std::path::Path::new(&ledger_only[0]).join("tmux").exists(),
            "a ledger leader can reach the fake tmux"
        );
        assert_eq!(dirs_for(Dialect::Tmux, &dirs).len(), SHIMS.len());
    }

    /// A summoned worker gets the road back to the ledger, and gets it on top
    /// of the PATH it was actually going to run with.
    ///
    /// The break this was written for: a worker's environment starts as a copy
    /// of its leader's — `<shims>:<real>` — and then the hook coordinates
    /// append a `PATH` of their own, twice, computed from their own vec. The
    /// later pair wins at spawn, so the worker ran with the shims GONE and
    /// could not execute the one command its briefing tells it to run. Every
    /// dispatch this window summoned stayed open forever, and B-1's "a worker
    /// went quiet" was not an edge case but the only case.
    ///
    /// Two things are measured, and the second is the one that decays: the base
    /// has to be the PATH the environment ended with, so the mirror shims that
    /// were put there survive. Rebuilding from the process's own PATH would
    /// look identical in a diff and take the nested-run page with it.
    #[test]
    fn a_summoned_worker_keeps_the_shims_on_top_of_the_path_it_was_given() {
        let local_data = tempfile::tempdir().expect("no local data dir");
        let sep = zerocode_core::agent_teams::PATH_SEPARATOR;
        let base = format!("/mirror-shims{sep}/usr/bin");

        let ledger = teammate_path(local_data.path(), Dialect::Ledger, &base)
            .expect("a worker with no road back to the ledger");
        assert!(
            ledger.ends_with(&base),
            "the PATH the worker was going to run with was thrown away: {ledger}"
        );
        let dirs: Vec<&str> = ledger.split(sep).collect();
        assert_eq!(
            dirs[0],
            local_data.path().join(ORC_DIR_NAME).to_string_lossy(),
            "`zerocode-orc` is not the first thing found: {ledger}"
        );
        assert!(
            !ledger.contains(SHIM_DIR_NAME),
            "a codex worker can reach the fake tmux its leader had: {ledger}"
        );

        // And a worker that DOES speak tmux gets both, in the same order its
        // own leader would have got them.
        let tmux = teammate_path(local_data.path(), Dialect::Tmux, &base).expect("a tmux worker");
        assert!(tmux.contains(SHIM_DIR_NAME) && tmux.contains(ORC_DIR_NAME));
        assert!(tmux.ends_with(&base));

        // A worker inherits its LEADER's PATH, so the leader's directories are
        // already in the base — including a fake `tmux` a codex worker must not
        // find. They come out; the worker's own go in.
        let leader = format!(
            "{}{sep}{}{sep}{base}",
            local_data.path().join(SHIM_DIR_NAME).to_string_lossy(),
            local_data.path().join(ORC_DIR_NAME).to_string_lossy(),
        );
        let inherited =
            teammate_path(local_data.path(), Dialect::Ledger, &leader).expect("a ledger worker");
        assert!(
            !inherited.contains(SHIM_DIR_NAME),
            "the leader's fake tmux survived into a codex worker: {inherited}"
        );
        assert_eq!(
            inherited.matches(ORC_DIR_NAME).count(),
            1,
            "the ledger shim is on the PATH twice: {inherited}"
        );
        assert!(
            inherited.ends_with(&base),
            "cleaning the leader's shims took the real PATH with them: {inherited}"
        );
    }

    #[test]
    fn the_windows_companions_carry_the_ledger_voice_and_nothing_else() {
        let [(powershell, script), (wrapper_name, wrapper)] =
            windows_companions(ORC_SHIM_NAME, "orchestration");
        // The names PATHEXT and PowerShell discovery actually resolve, beside
        // the shebang file Git Bash keeps reading.
        assert_eq!(powershell, "zerocode-orc.ps1");
        assert_eq!(wrapper_name, "zerocode-orc.cmd");
        // The script refuses in the ledger's voice, reads the bridge's own
        // environment words, and the wrapper points at the sibling by the
        // exact name written beside it.
        assert!(script.contains("orchestration: this shell is not part of an agent team"));
        assert!(script.contains(&format!("$env:{}", zerocode_hookd::env_var::PORT)));
        assert!(
            wrapper.contains("\"%~dp0zerocode-orc.ps1\" %*"),
            "{wrapper}"
        );
    }
}
