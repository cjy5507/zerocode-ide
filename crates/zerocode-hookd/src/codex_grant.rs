//! Asking Codex to trust a hook, in its own words.
//!
//! [`crate::codex_trust`] reads the trust contract. This grants it — and the only
//! sanctioned way to do that is to ask Codex. Orca does not write a trust entry
//! itself: it *removes* any it previously computed and then runs
//! `codex app-server`, calls `hooks/list`, and for anything not already trusted
//! calls `config/batchWrite` on `hooks.state` — then lists again to verify
//! (`runCodexHookTrustGrantSession`, codex-app-server-client-DseSy6-0.js:1214-1260,
//! Orca **1.4.169**).
//!
//! ## The hash that gets written is Codex's, not ours
//!
//! This is the detail that makes the round trip necessary rather than a
//! formality. The value written is `listing.currentHash` — **the hash Codex
//! itself reported for that hook**. Our own [`crate::codex_trust::trusted_hash`]
//! exists to RECOGNISE trust (is this key trusted for the hook we are about to
//! write?) and to recognise our own earlier entries. It is never the thing
//! granted. So a trust entry can only be created by a Codex that has read the
//! file and agrees what is in it — which is the whole point of the mechanism, and
//! why writing the hash ourselves would be forging the user's consent.
//!
//! ## The protocol, measured
//!
//! Newline-delimited JSON over stdio (`runCodexAppServerSession`, :1038-1180).
//! Not JSON-RPC 2.0 — there is no `jsonrpc` field; a request is
//! `{"method":…,"id":N,"params":…}` and a notification is the same without `id`.
//! Ids are numbers from 1. The session opens with an `initialize` request
//! carrying `clientInfo`, then an `initialized` notification.
//!
//! Three failures are told apart because they mean different things:
//!
//! - **Unsupported** — this Codex has no `app-server` subcommand, or does not
//!   know the method. Nothing to retry; remember it.
//! - **Timeout** — the session ran past its deadline. Retry later.
//! - **Verify failed** — Codex answered and the hooks are still not trusted.
//!   The caller must roll its hook back.
//!
//! ## What the caller must do with a failure
//!
//! Roll back. A hook written into `hooks.json` without a trust entry does not
//! fail loudly — it stalls the agent on a prompt. Orca restores the previous
//! `hooks.json` bytes and mode and restores the trust config from a snapshot
//! before falling back to a mirrored `CODEX_HOME`. This module reports; it does
//! not write `hooks.json`, so it cannot roll one back either.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::codex_trust::normalize_source_path;

/// How we introduce ourselves in `initialize`.
///
/// Ours, not Orca's — it sends `{name: "orca_desktop", title: "Orca"}`, and a
/// product that claimed to be another product while editing the user's config
/// would be lying to the tool it is asking permission from.
pub const CLIENT_NAME: &str = "zerocode";
pub const CLIENT_TITLE: &str = "ZeroCode";

/// Session deadline for a native `codex`. Orca's `NATIVE_GRANT_TIMEOUT_MS`; a
/// grant spawns a process and does two round trips, and this is the point past
/// which something is wrong rather than slow.
pub const NATIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// How much stderr to keep for the message when the child dies. Enough to carry
/// a usage error, bounded because a broken binary can print forever.
const STDERR_TAIL_BYTES: usize = 4096;

/// A single response line's ceiling. Orca kills the child on an oversized JSONL
/// line for the same reason: a hooks list is kilobytes, and anything vastly
/// larger is a runaway, not an answer.
const LINE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// What went wrong, kept apart because the caller does different things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    /// This Codex cannot do it. Do not retry — remember and stop asking.
    Unsupported(String),
    /// Ran past the deadline. Retry later.
    Timeout(String),
    /// Everything else: spawn failure, a protocol answer we cannot use.
    Failed(String),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantError::Unsupported(why) => write!(f, "codex app-server unsupported: {why}"),
            GrantError::Timeout(why) => write!(f, "codex app-server timed out: {why}"),
            GrantError::Failed(why) => write!(f, "codex app-server failed: {why}"),
        }
    }
}

impl std::error::Error for GrantError {}

/// One hook as `hooks/list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookListing {
    pub key: String,
    /// `null` in the protocol when Codex does not report one, and a listing with
    /// no command can never be matched as ours.
    #[serde(default)]
    pub command: Option<String>,
    pub current_hash: String,
    /// `trusted` or something else. Kept as the word Codex used so a message can
    /// quote it rather than paraphrasing.
    pub trust_status: String,
}

/// How the grant went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    Granted {
        /// Whether anything was actually written. False means it was already
        /// trusted, which is the common case after the first time.
        wrote_trust: bool,
        entries: Vec<GrantedEntry>,
    },
    /// Codex answered and the hooks are still not trusted. **Roll the hook
    /// back** — this is the state that stalls an agent.
    VerifyFailed { reason: String, class: VerifyClass },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantedEntry {
    pub key: String,
    pub normalized_key: String,
    /// Codex's hash, which is what a trust entry carries.
    pub trusted_hash: String,
}

/// Which verification step failed. Orca's own three (`reasonClass`), kept apart
/// because they say different things about what to look at: the first two mean
/// the hooks we wrote are not the hooks Codex sees, the third means they are and
/// consent was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyClass {
    ListMismatch,
    PostGrantMismatch,
    PostGrantUntrusted,
}

/// How to start the app server.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Cleared from the child's environment. `CODEX_HOME` when the grant is for
    /// Codex's own default home — inheriting ours would grant trust in the wrong
    /// file.
    pub env_to_delete: Vec<String>,
    pub timeout: Duration,
}

impl Invocation {
    /// A native `codex app-server` for the given home.
    ///
    /// `use_default_home` decides between clearing `CODEX_HOME` and setting it,
    /// which is Orca's own branch (`buildRequest`,
    /// managed-agent-hook-controls-Hy3KYvcp.js:3666-3679) — and the distinction
    /// matters: the trust entry lands in whichever config the child reads.
    pub fn native(command: &str, home: &std::path::Path, use_default_home: bool) -> Self {
        let (env, env_to_delete) = if use_default_home {
            (Vec::new(), vec!["CODEX_HOME".to_string()])
        } else {
            (
                vec![(
                    "CODEX_HOME".to_string(),
                    home.to_string_lossy().into_owned(),
                )],
                Vec::new(),
            )
        };
        Self {
            command: command.to_string(),
            args: vec!["app-server".to_string()],
            env,
            env_to_delete,
            timeout: NATIVE_TIMEOUT,
        }
    }
}

/// What a grant is for.
#[derive(Debug, Clone)]
pub struct GrantPlan {
    pub invocation: Invocation,
    /// The directory `hooks/list` is asked about.
    pub hooks_list_cwd: PathBuf,
    /// The normalised trust keys we expect to see, and only those.
    pub expected_keys: BTreeSet<String>,
    /// The exact command string our hooks carry. A listing whose command differs
    /// is somebody else's hook and is never touched.
    pub managed_command: String,
}

/// Does a listing report one of our hooks?
///
/// Both halves. The command must be ours *and* the key must be one we expected —
/// either alone would let us grant trust to a hook we did not write, which is
/// consent we have no business giving.
fn is_ours(listing: &HookListing, plan: &GrantPlan) -> bool {
    listing.command.as_deref() == Some(plan.managed_command.as_str())
        && plan
            .expected_keys
            .contains(&normalize_listing_key(&listing.key))
}

/// Fold a reported key the way trust lookups do
/// (`normalizeHookTrustKeyForLookup`, codex-app-server-client-DseSy6-0.js:667-671):
/// the path part folds, the three trailing fields do not.
pub fn normalize_listing_key(key: &str) -> String {
    // Split from the RIGHT: the path may contain colons, the three fields after
    // it may not.
    let mut parts = key.rsplitn(4, ':');
    let Some(handler) = parts.next() else {
        return key.to_string();
    };
    let (Some(group), Some(event), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
        return key.to_string();
    };
    // Only when the tail actually looks like the key's tail. A string with three
    // colons in it that is not a trust key must come back unchanged, or an
    // unrelated value would be silently rewritten into one.
    if group.parse::<usize>().is_err() || handler.parse::<usize>().is_err() {
        return key.to_string();
    }
    // A UNC path (`//host/share`) is left alone, which is Orca's own exception.
    let folded = if path.starts_with("//") {
        path.to_string()
    } else {
        normalize_source_path(std::path::Path::new(path))
    };
    format!("{folded}:{event}:{group}:{handler}")
}

/// A live app-server, speaking JSONL.
///
/// The pipes are taken out of the child and the child itself lives behind a
/// mutex, so the deadline watchdog can kill it while this thread is blocked
/// reading. Killing through the `Child` rather than by pid keeps this portable —
/// a `libc::kill` here would be a unix-only dependency for something the standard
/// library already does.
struct Session {
    child: Arc<Mutex<Child>>,
    stdin: Option<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl Session {
    fn start(invocation: &Invocation) -> Result<Self, GrantError> {
        let mut command = Command::new(&invocation.command);
        command
            .args(&invocation.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &invocation.env {
            command.env(name, value);
        }
        for name in &invocation.env_to_delete {
            command.env_remove(name);
        }
        let mut child = command.spawn().map_err(|error| {
            // A `codex` that is not installed is not a protocol failure and must
            // never be retried on a timer.
            if error.kind() == std::io::ErrorKind::NotFound {
                GrantError::Unsupported(format!("{} is not on PATH", invocation.command))
            } else {
                GrantError::Failed(error.to_string())
            }
        })?;
        let stdout =
            BufReader::new(child.stdout.take().ok_or_else(|| {
                GrantError::Failed("codex app-server gave no stdout".to_string())
            })?);
        let stdin = child.stdin.take();
        // Drained on its own thread. Without this the child blocks once it has
        // filled the stderr pipe, and we block reading stdout that will never
        // come — a deadlock that looks exactly like a hung agent.
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(pipe) = child.stderr.take() {
            let held = Arc::clone(&stderr);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if let Ok(mut tail) = held.lock() {
                        tail.push_str(&line);
                        // Keep the END: the useful part of a failing tool's
                        // output is its last words.
                        if tail.len() > STDERR_TAIL_BYTES {
                            let cut = tail.len() - STDERR_TAIL_BYTES;
                            // On a char boundary, or `drain` panics on UTF-8.
                            let cut = (cut..tail.len())
                                .find(|at| tail.is_char_boundary(*at))
                                .unwrap_or(tail.len());
                            tail.drain(..cut);
                        }
                    }
                    line.clear();
                }
            });
        }
        Ok(Session {
            child: Arc::new(Mutex::new(child)),
            stdin,
            stdout,
            stderr,
            next_id: 1,
        })
    }

    fn stderr_tail(&self) -> String {
        self.stderr
            .lock()
            .map(|held| held.trim().chars().take(400).collect())
            .unwrap_or_default()
    }

    fn send(&mut self, payload: &serde_json::Value) -> Result<(), GrantError> {
        let line = format!("{payload}\n");
        let wrote = match self.stdin.as_mut() {
            Some(stdin) => stdin
                .write_all(line.as_bytes())
                .and_then(|()| stdin.flush()),
            None => {
                return Err(GrantError::Failed(
                    "codex app-server closed its input".to_string(),
                ));
            }
        };
        wrote.map_err(|error| self.exit_error(&error.to_string()))
    }

    /// The error for a child that went away, distinguishing a Codex with no
    /// `app-server` from one that broke.
    fn exit_error(&self, detail: &str) -> GrantError {
        let tail = self.stderr_tail();
        if missing_app_server(&tail) {
            GrantError::Unsupported(format!("codex CLI has no app-server subcommand: {tail}"))
        } else if tail.is_empty() {
            GrantError::Failed(format!("codex app-server exited: {detail}"))
        } else {
            GrantError::Failed(format!("codex app-server exited: {tail}"))
        }
    }

    fn notify(&mut self, method: &str) -> Result<(), GrantError> {
        self.send(&serde_json::json!({ "method": method }))
    }

    /// One request, and the response with the matching id.
    ///
    /// Lines that are not our response — notifications, another id — are skipped
    /// rather than treated as an error: the server is entitled to talk.
    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, GrantError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({ "method": method, "id": id, "params": params }))?;
        let mut line = String::new();
        loop {
            line.clear();
            let read = self
                .stdout
                .read_line(&mut line)
                .map_err(|error| GrantError::Failed(error.to_string()))?;
            if read == 0 {
                return Err(self.exit_error("before answering"));
            }
            if line.len() > LINE_MAX_BYTES {
                return Err(GrantError::Failed(
                    "codex app-server emitted an oversized JSONL line".to_string(),
                ));
            }
            let Ok(message) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                continue;
            };
            if message.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let said = error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown error");
                // Method-not-found is JSON-RPC's -32601, and it means this
                // Codex is too old rather than that anything went wrong.
                let code = error.get("code").and_then(serde_json::Value::as_i64);
                if code == Some(-32601) || said.to_lowercase().contains("method not found") {
                    return Err(GrantError::Unsupported(format!("{method}: {said}")));
                }
                return Err(GrantError::Failed(format!("{method}: {said}")));
            }
            return Ok(message
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Always. A grant that returned early must not leave an app server
        // running — it holds the config file we are about to read.
        //
        // The pipe goes first: a server waiting on input exits on EOF, so this is
        // the polite version of the kill that follows.
        self.stdin = None;
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Does this stderr say the subcommand does not exist?
///
/// Read from the words a CLI uses for it rather than an exit code, because the
/// exit code for "unknown subcommand" and for "failed while running" is the same
/// 1 — and treating the first as transient means retrying forever.
fn missing_app_server(tail: &str) -> bool {
    let lowered = tail.to_lowercase();
    [
        "unrecognized subcommand",
        "unknown subcommand",
        "unexpected argument",
    ]
    .iter()
    .any(|mark| lowered.contains(mark))
        || (lowered.contains("app-server") && lowered.contains("not"))
}

/// Read the listings out of a `hooks/list` result.
///
/// Tolerant of shape and strict about fields: a listing missing any of the three
/// strings the match needs is skipped rather than defaulted, and a repeated key
/// is taken once (Orca's `collectHookListings`, :1195-1213). A default here would
/// invent a hook.
fn collect_listings(result: &serde_json::Value) -> Vec<HookListing> {
    let mut listings: Vec<HookListing> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    walk_listings(result, &mut listings, &mut seen);
    // A stable order for the messages built from this, chosen after the walk so
    // the dedup above still resolved in REPORT order — the first thing Codex said
    // about a key wins, because taking the second would be a coin flip between
    // two answers about the same hook.
    listings.sort_by(|left, right| left.key.cmp(&right.key));
    listings
}

/// Walk a `hooks/list` result in order, collecting whatever is a listing.
///
/// In order, and recursively, because the result is nested per requested cwd and
/// because the dedup depends on which mention came first. Written as a recursion
/// rather than a worklist for exactly that: a stack-based walk reverses the
/// order, which silently turned first-wins into last-wins — caught by the test
/// that gives one key two different trust statuses.
fn walk_listings(
    node: &serde_json::Value,
    into: &mut Vec<HookListing>,
    seen: &mut BTreeSet<String>,
) {
    match node {
        serde_json::Value::Array(items) => {
            for item in items {
                walk_listings(item, into, seen);
            }
        }
        serde_json::Value::Object(map) => {
            let key = map.get("key").and_then(serde_json::Value::as_str);
            let hash = map
                .get("currentHash")
                .or_else(|| map.get("current_hash"))
                .and_then(serde_json::Value::as_str);
            let status = map
                .get("trustStatus")
                .or_else(|| map.get("trust_status"))
                .and_then(serde_json::Value::as_str);
            // All three, or this is not a listing. Defaulting a missing field
            // would invent a hook, and a grant could then give consent for it.
            if let (Some(key), Some(hash), Some(status)) = (key, hash, status) {
                if seen.insert(key.to_string()) {
                    into.push(HookListing {
                        key: key.to_string(),
                        command: map
                            .get("command")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        current_hash: hash.to_string(),
                        trust_status: status.to_string(),
                    });
                }
                return;
            }
            // Not a listing itself; it may hold them. `data` is not from the
            // Orca measurement: codex-cli 0.147.0 answers `hooks/list` as
            // `{"data":[{"cwd":…,"hooks":[…]}]}` (captured live, 2026-08-10),
            // and a walk that did not know the envelope read every answer as
            // "no hooks", failed every grant, and sent every Codex to the
            // mirror home on a machine whose codex was perfectly willing.
            for nested in ["data", "hooks", "results", "entries"] {
                if let Some(found) = map.get(nested) {
                    walk_listings(found, into, seen);
                }
            }
        }
        _ => {}
    }
}

/// Ask Codex to trust our hooks, and verify that it did.
///
/// The deadline is enforced by killing the child from a watchdog thread, which
/// turns a hung server into an EOF on the read we are blocked in. A timeout that
/// only stopped waiting would leave the process behind holding the config.
pub fn grant(plan: &GrantPlan) -> Result<Grant, GrantError> {
    let mut session = Session::start(&plan.invocation)?;
    // The watchdog kills the child, which is what makes the blocked read return.
    // Stopping the wait without killing would leave a process behind holding the
    // config file the caller is about to read.
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let child = Arc::clone(&session.child);
        let deadline = plan.invocation.timeout;
        let held = Arc::clone(&done);
        let rang = Arc::clone(&fired);
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            while started.elapsed() < deadline {
                if held.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            if held.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            rang.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        });
    }
    let outcome = grant_in(&mut session, plan);
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    // The read ended because the watchdog killed the child, so whatever it
    // reported is really a timeout — and a timeout is retryable where the errors
    // it would otherwise be reported as are not.
    if fired.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(GrantError::Timeout(format!(
            "codex app-server session exceeded {}ms",
            plan.invocation.timeout.as_millis()
        )));
    }
    outcome
}

fn grant_in(session: &mut Session, plan: &GrantPlan) -> Result<Grant, GrantError> {
    session.request(
        "initialize",
        serde_json::json!({
            "clientInfo": { "name": CLIENT_NAME, "title": CLIENT_TITLE, "version": env!("CARGO_PKG_VERSION") }
        }),
    )?;
    session.notify("initialized")?;

    let list = |session: &mut Session| -> Result<Vec<HookListing>, GrantError> {
        let result = session.request(
            "hooks/list",
            serde_json::json!({ "cwds": [plan.hooks_list_cwd.to_string_lossy()] }),
        )?;
        Ok(collect_listings(&result)
            .into_iter()
            .filter(|listing| is_ours(listing, plan))
            .collect())
    };

    let ours = list(session)?;
    let covered: BTreeSet<String> = ours
        .iter()
        .map(|listing| normalize_listing_key(&listing.key))
        .collect();
    if ours.len() != plan.expected_keys.len() || !plan.expected_keys.is_subset(&covered) {
        return Ok(Grant::VerifyFailed {
            reason: format!(
                "hooks/list reported {} entries covering {} of {} expected",
                ours.len(),
                covered.len(),
                plan.expected_keys.len()
            ),
            class: VerifyClass::ListMismatch,
        });
    }

    let needing: Vec<&HookListing> = ours
        .iter()
        .filter(|listing| listing.trust_status != "trusted")
        .collect();
    let wrote_trust = !needing.is_empty();
    if wrote_trust {
        // Codex's OWN hash for each, keyed by the key it reported. Never one we
        // computed — that would be forging the user's consent.
        let mut value = serde_json::Map::new();
        for listing in &needing {
            value.insert(
                listing.key.clone(),
                serde_json::json!({ "trusted_hash": listing.current_hash }),
            );
        }
        session.request(
            "config/batchWrite",
            serde_json::json!({
                "edits": [{
                    "keyPath": "hooks.state",
                    "value": value,
                    "mergeStrategy": "upsert"
                }],
                "reloadUserConfig": true
            }),
        )?;
    }

    // And listed again, because a write that returned success is not the same as
    // a hook that will run.
    let verified = list(session)?;
    let verified_keys: BTreeSet<String> = verified
        .iter()
        .map(|listing| normalize_listing_key(&listing.key))
        .collect();
    let untrusted: Vec<&HookListing> = verified
        .iter()
        .filter(|listing| listing.trust_status != "trusted")
        .collect();
    if let Some(first) = untrusted.first() {
        return Ok(Grant::VerifyFailed {
            reason: format!(
                "post-grant verify left {} entries {}",
                untrusted.len(),
                first.trust_status
            ),
            class: VerifyClass::PostGrantUntrusted,
        });
    }
    if verified.len() != plan.expected_keys.len() || !plan.expected_keys.is_subset(&verified_keys) {
        return Ok(Grant::VerifyFailed {
            reason: format!(
                "post-grant verify reported {} entries covering {} of {} expected",
                verified.len(),
                verified_keys.len(),
                plan.expected_keys.len()
            ),
            class: VerifyClass::PostGrantMismatch,
        });
    }
    Ok(Grant::Granted {
        wrote_trust,
        entries: verified
            .iter()
            .map(|listing| GrantedEntry {
                key: listing.key.clone(),
                normalized_key: normalize_listing_key(&listing.key),
                trusted_hash: listing.current_hash.clone(),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_key_folds_its_path_and_leaves_its_indices_alone() {
        assert_eq!(
            normalize_listing_key("/a/b/../c/hooks.json:pre_tool_use:0:1"),
            "/a/c/hooks.json:pre_tool_use:0:1"
        );
        // A UNC path is Orca's own exception and stays as written.
        assert_eq!(
            normalize_listing_key("//host/share/hooks.json:stop:0:0"),
            "//host/share/hooks.json:stop:0:0"
        );
        // Something that merely has colons in it is not a trust key and must
        // come back untouched, or an unrelated value gets rewritten into one.
        for held in ["not:a:trust:key", "sha256:abc", "", "/a:stop:zero:0"] {
            assert_eq!(normalize_listing_key(held), held, "{held}");
        }
    }

    /// A listing missing any field the match needs is skipped, not defaulted —
    /// a default would invent a hook and could grant trust to it.
    #[test]
    fn listings_are_read_strictly_and_deduplicated() {
        let result = serde_json::json!({
            "hooks": [
                { "key": "/h.json:stop:0:0", "command": "ours", "currentHash": "sha256:a",
                  "trustStatus": "trusted" },
                { "key": "/h.json:stop:0:0", "command": "ours", "currentHash": "sha256:a",
                  "trustStatus": "untrusted" },
                { "key": "/h.json:stop:0:1", "command": "ours", "currentHash": "sha256:b" },
                { "key": "/h.json:stop:0:2", "currentHash": "sha256:c", "trustStatus": "trusted" },
            ]
        });
        let listings = collect_listings(&result);
        assert_eq!(listings.len(), 2, "{listings:?}");
        // First wins on a repeat: the second said something different about the
        // same key and taking it would be a coin flip.
        assert_eq!(listings[0].trust_status, "trusted");
        // The one with no command is kept but can never match as ours.
        assert_eq!(listings[1].command, None);
    }

    /// The envelope codex-cli 0.147.0 actually answers with — `data`, then a
    /// per-cwd object holding `hooks` — captured live. The walk missing `data`
    /// read every answer as zero hooks, and zero is a ListMismatch: the grant
    /// failed on a machine whose codex listed our entries perfectly well.
    #[test]
    fn the_data_envelope_of_a_current_codex_is_walked() {
        let result = serde_json::json!({
            "data": [{
                "cwd": "/h",
                "hooks": [{
                    "key": "/h/.codex/hooks.json:stop:0:0",
                    "eventName": "stop",
                    "command": "ours",
                    "currentHash": "sha256:a",
                    "trustStatus": "modified"
                }]
            }]
        });
        let listings = collect_listings(&result);
        assert_eq!(listings.len(), 1, "{listings:?}");
        assert_eq!(listings[0].key, "/h/.codex/hooks.json:stop:0:0");
        assert_eq!(listings[0].command.as_deref(), Some("ours"));
    }

    /// A hook is ours only if BOTH the command and the key say so. Either alone
    /// would let a grant give consent for something we did not write.
    #[test]
    fn a_hook_is_ours_only_when_the_command_and_the_key_agree() {
        let plan = GrantPlan {
            invocation: Invocation::native("codex", std::path::Path::new("/h"), true),
            hooks_list_cwd: PathBuf::from("/h"),
            expected_keys: ["/h/hooks.json:stop:0:0".to_string()].into_iter().collect(),
            managed_command: "ours".to_string(),
        };
        let listing = |key: &str, command: Option<&str>| HookListing {
            key: key.to_string(),
            command: command.map(str::to_string),
            current_hash: "sha256:a".into(),
            trust_status: "trusted".into(),
        };
        assert!(is_ours(
            &listing("/h/hooks.json:stop:0:0", Some("ours")),
            &plan
        ));
        // Our key, somebody else's command.
        assert!(!is_ours(
            &listing("/h/hooks.json:stop:0:0", Some("theirs")),
            &plan
        ));
        // Our command, a key we did not expect.
        assert!(!is_ours(
            &listing("/h/hooks.json:stop:9:9", Some("ours")),
            &plan
        ));
        assert!(!is_ours(&listing("/h/hooks.json:stop:0:0", None), &plan));
        // And an unnormalised spelling of our key still matches, which is the
        // point of folding it.
        assert!(is_ours(
            &listing("/h/./hooks.json:stop:0:0", Some("ours")),
            &plan
        ));
    }

    /// A `codex` that is not installed is UNSUPPORTED, not a transient failure.
    /// Getting this wrong means retrying a missing binary on a timer forever.
    #[test]
    fn a_missing_binary_is_unsupported_rather_than_retryable() {
        let plan = GrantPlan {
            invocation: Invocation::native(
                "zerocode-no-such-codex-binary",
                std::path::Path::new("/h"),
                true,
            ),
            hooks_list_cwd: PathBuf::from("/h"),
            expected_keys: BTreeSet::new(),
            managed_command: "ours".to_string(),
        };
        match grant(&plan) {
            Err(GrantError::Unsupported(_)) => {}
            other => panic!("a missing binary was not reported as unsupported: {other:?}"),
        }
    }

    /// The words a CLI uses when a subcommand does not exist.
    #[test]
    fn a_codex_without_the_subcommand_is_told_from_one_that_broke() {
        for tail in [
            "error: unrecognized subcommand 'app-server'",
            "Unknown subcommand: app-server",
            "error: unexpected argument 'app-server' found",
        ] {
            assert!(missing_app_server(tail), "{tail}");
        }
        for tail in ["panicked at src/main.rs", "connection refused", ""] {
            assert!(!missing_app_server(tail), "{tail}");
        }
    }

    /// `use_default_home` decides whether `CODEX_HOME` is cleared or set — and
    /// the trust entry lands in whichever config the child reads, so this is not
    /// cosmetic.
    #[test]
    fn the_home_is_cleared_or_set_but_never_both() {
        let default = Invocation::native("codex", std::path::Path::new("/mirror"), true);
        assert_eq!(default.env_to_delete, vec!["CODEX_HOME"]);
        assert!(default.env.is_empty());

        let mirrored = Invocation::native("codex", std::path::Path::new("/mirror"), false);
        assert!(mirrored.env_to_delete.is_empty());
        assert_eq!(
            mirrored.env,
            vec![("CODEX_HOME".to_string(), "/mirror".to_string())]
        );
        assert_eq!(default.args, vec!["app-server"]);
    }
}
