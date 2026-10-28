//! The agent hook bridge: a loopback HTTP endpoint that PTY-hosted agent CLIs
//! report their lifecycle events to.
//!
//! ## Why an HTTP bridge at all
//!
//! An agent running inside a PTY is a black box — the only signal is bytes on a
//! terminal. Screen-scraping those bytes to guess "the agent is waiting for
//! permission now" is fragile and breaks on every vendor UI change. Every major
//! agent CLI, however, already supports a hook script: a program it runs on
//! lifecycle events with a JSON payload on stdin. ZeroCode installs one tiny
//! script per agent that forwards that payload here.
//!
//! ## Rules this bridge does not bend
//!
//! - **Loopback only.** [`serve`] binds `127.0.0.1`. There is no bind-address
//!   parameter, because a hook endpoint on `0.0.0.0` is a remote code execution
//!   funnel into the user's project.
//! - **Token on every request, checked before the body is touched.** The gate
//!   is a middleware layer, not a check inside the handler: axum runs a
//!   handler's body extractor *before* the handler body, so an in-handler check
//!   would still have parsed — and waited for — an unauthenticated request's
//!   stream. The layer answers 401 without ever polling the body, and an
//!   attacker cannot distinguish "wrong token" from "wrong body".
//! - **The agent is the URL, not the body.** A payload cannot claim to be a
//!   different agent than the endpoint it posted to.
//! - **The script fails open.** See [`install_hook_scripts`]: whatever happens
//!   here, the agent CLI must keep running.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Form, FromRequest, Path as UrlPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use busy::{Busy, BusyEntry, Seat, Seats};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use zerocode_core::computer_use::COMPUTER_CLI;
use zerocode_core::{ALL_AGENTS, AgentKind, HookEnvelope};

/// Header carrying the per-run shared secret.
pub const HOOK_TOKEN_HEADER: &str = "x-zerocode-hook-token";

/// The only route a federated home may enter with its scoped bearer.
pub const FEDERATION_PATH: &str = "/federation";

/// A visible version marker lets an address book distinguish a new scoped
/// bearer from the bridge-wide token older `federation-invite` builds printed.
pub const FEDERATION_TOKEN_PREFIX: &str = "zcf1_";

const FEDERATION_TOKEN_DOMAIN: &[u8] = b"zerocode/federation-token/v1\0";

/// The bearer shared with a federated home.
///
/// This is a distinct type so diagnostics cannot accidentally print the
/// credential while route authorization and `federation-invite` share one
/// representation.
#[derive(Clone)]
pub struct FederationToken(String);

impl FederationToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for FederationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FederationToken")
            .field("token_bytes", &self.0.len())
            .finish()
    }
}

/// Derive the federation bearer from the bridge secret.
///
/// Kept as a function of the bridge token so the hook bridge and the shell's
/// invite verb cannot mint or persist disagreeing credentials.
pub fn federation_token(bridge_token: &str) -> FederationToken {
    use sha2::Digest as _;

    let mut digest = sha2::Sha256::new();
    digest.update(FEDERATION_TOKEN_DOMAIN);
    digest.update(bridge_token.as_bytes());
    let digest = digest.finalize();

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(FEDERATION_TOKEN_PREFIX.len() + digest.len() * 2);
    token.push_str(FEDERATION_TOKEN_PREFIX);
    for byte in digest {
        token.push(HEX[usize::from(byte >> 4)] as char);
        token.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    FederationToken(token)
}

/// Whether a saved bearer uses the scoped federation token format.
pub fn is_scoped_federation_token(token: &str) -> bool {
    use sha2::Digest as _;

    let Some(encoded) = token.strip_prefix(FEDERATION_TOKEN_PREFIX) else {
        return false;
    };
    encoded.len() == sha2::Sha256::output_size() * 2
        && encoded.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Header carrying the BROWSER capability's own secret. A second token
/// rather than the hook one, because the two grants are different sizes:
/// every PTY child reports hooks, but only a launched agent's pane may
/// steer the browser — a build script in a plain shell reading page text
/// with the hook token was the 1-g4 review's first finding.
pub const BROWSER_TOKEN_HEADER: &str = "x-zerocode-browser-token";
/// Which pane a `zerocode-browser` or `zerocode-emulator` command came from —
/// the shim's own `ZEROCODE_PANE_KEY`, forwarded so the window can seat what
/// it opens.
pub const BROWSER_PANE_HEADER: &str = zerocode_core::agent_browser::PANE_HEADER;
/// Computer Use is a larger grant than status hooks or page reading, so it
/// has its own launched-agent-only capability token.
pub const COMPUTER_TOKEN_HEADER: &str = "x-zerocode-computer-token";

/// Version stamped into generated scripts and echoed back in the envelope, so a
/// script left behind by an older install is identifiable rather than mysterious.
pub const HOOK_CONTRACT_VERSION: &str = "3";

/// Hook payloads are small (a JSON event, sometimes a diff summary). A megabyte
/// is already generous; past that something is wrong and we would rather answer
/// 413 than buffer it.
pub const MAX_HOOK_BODY_BYTES: usize = 1024 * 1024;

/// Environment variables the generated scripts read. Namespaced `ZEROCODE_*`
/// rather than `ZO_*` on purpose: `ZO_*` belongs to the harness CLI, and a
/// collision there would be a debugging nightmare.
pub mod env_var {
    pub const PORT: &str = "ZEROCODE_HOOK_PORT";
    pub const TOKEN: &str = "ZEROCODE_HOOK_TOKEN";
    /// Path-only capability variables. The generated shims read the value at
    /// invocation time; the secret itself never has to live in the pane's
    /// inherited environment.
    pub const BROWSER_TOKEN_FILE: &str = "ZEROCODE_BROWSER_TOKEN_FILE";
    pub const COMPUTER_TOKEN_FILE: &str = "ZEROCODE_COMPUTER_TOKEN_FILE";
    pub const TEAM_TOKEN_FILE: &str = "ZEROCODE_AGENT_TEAM_TOKEN_FILE";
    /// Claude's own nested-agent credential. The PTY boundary removes an
    /// inherited value because its matching socket is removed too; a newly
    /// launched Claude creates its own session credential.
    pub const CLAUDE_MESSAGING_TOKEN: &str = "CLAUDE_CODE_MESSAGING_TOKEN";
    /// The browser capability's legacy value variable. Normal launches pass
    /// [`BROWSER_TOKEN_FILE`] instead, and the shim reads this name only in its
    /// short-lived process. See [`crate::BROWSER_TOKEN_HEADER`].
    pub const BROWSER_TOKEN: &str = "ZEROCODE_BROWSER_TOKEN";
    /// The computer capability's legacy value variable; see [`BROWSER_TOKEN`].
    pub const COMPUTER_TOKEN: &str = "ZEROCODE_COMPUTER_TOKEN";
    /// Where this machine's second-brain vault is, when one is configured.
    ///
    /// Carried by every pane so an agent working in ANY project can find the
    /// vault — the core owns the name because the guide block written into
    /// each agent's global instructions and the `second-brain` skill both
    /// quote it, and three spellings would be three variables.
    pub const SECOND_BRAIN: &str = zerocode_core::second_brain::VAULT_ENV;
    pub const PANE_KEY: &str = zerocode_core::hook::PANE_KEY_ENV;
    /// A directory whose presence means this pane's most recent hook POST did
    /// not reach the window. The generated script creates it without writing
    /// payload bytes and removes it only after a successful delivery.
    pub const DELIVERY_FAILURE_MARKER: &str = "ZEROCODE_HOOK_DELIVERY_FAILURE_MARKER";
    pub const TAB_ID: &str = "ZEROCODE_TAB_ID";
    pub const LAUNCH_TOKEN: &str = "ZEROCODE_LAUNCH_TOKEN";
    pub const WORKTREE_ID: &str = "ZEROCODE_WORKTREE_ID";
    pub const AGENT_ENV: &str = "ZEROCODE_HOOK_ENV";
    /// The contract version, in the environment so the endpoint file can
    /// correct it alongside the port.
    pub const VERSION: &str = "ZEROCODE_HOOK_VERSION";
    /// Path of the endpoint file (see [`crate::endpoint`]). Scripts source it
    /// before reading anything else, so a bridge that restarted on a new port
    /// still hears from agents launched before the restart.
    pub const ENDPOINT: &str = "ZEROCODE_HOOK_ENDPOINT";
    /// Copilot runs ONE hook command for its many events and does not name the
    /// event in the payload; the installed command names it here instead.
    pub const COPILOT_EVENT: &str = "ZEROCODE_COPILOT_HOOK_EVENT";
    /// Antigravity, the same way — and its script also has to ANSWER
    /// differently per event, so the name decides more than a form field.
    pub const ANTIGRAVITY_EVENT: &str = "ZEROCODE_ANTIGRAVITY_EVENT";
    /// Nested hook configs know which event each command belongs to. Carry it
    /// into the shared script so providers can answer events that cannot
    /// inject context immediately, without a bridge round trip.
    pub const EVENT: &str = "ZEROCODE_HOOK_EVENT";
    /// The launch prompt already carries the provider-neutral orchestration
    /// contract. A startup hook records the session but does not inject a
    /// duplicate copy; later clear/compact boundaries still restore it.
    pub const SELECTION_SEEDED: &str = "ZEROCODE_AGENT_SELECTION_SEEDED";
}

mod brief_receipts;
pub mod busy;
pub mod codex_grant;
pub mod codex_install;
pub mod codex_mirror;
pub mod codex_runtime_auth;
pub mod codex_trust;
pub mod endpoint;
pub mod install;
mod orchestration_contract;
pub mod plugin;
pub mod pointer_mailbox;
mod private_file;
mod replays;
pub mod session_notify;
pub mod toml_lines;

/// Header naming which team a tmux-shim request belongs to, and which of its
/// panes is asking. Both are the shim's own environment read back out; the
/// TOKEN is still the bridge's, checked by the same gate as everything else.
pub const TEAM_ID_HEADER: &str = "x-zerocode-team-id";
pub const TEAM_PANE_HEADER: &str = "x-zerocode-team-pane";
pub const TEAM_TOKEN_HEADER: &str = "x-zerocode-team-token";

/// One `tmux …` the fake tmux on an orchestrating agent's `PATH` was asked to
/// run, on its way to the window that can actually cut a pane.
/// One federation call from a HOME window, handed whole to the window's
/// blocking half. The bridge checks the transport token and requires the
/// home fingerprint header; everything else — attachment ownership, the
/// contiguity of relay, whose screen a summons may cut — is ledger law on
/// the other side of this channel.
pub struct FederationRequest {
    /// The home's federation fingerprint, from [`FEDERATION_HOME_HEADER`].
    pub home: String,
    /// The call, verbatim JSON: `{"verb": "attach"|"pull"|"ack"|"import"|"stop", …}`.
    pub body: serde_json::Value,
    /// Where the JSON answer goes. Dropped means the window went away
    /// mid-call, which the route turns into 503 rather than a hang.
    pub answer: tokio::sync::oneshot::Sender<String>,
}

/// The header a home window presents its federation identity in.
pub const FEDERATION_HOME_HEADER: &str = "x-zerocode-federation-home";

/// How long a federation call may cook before the route gives up: the
/// attach half spawns a pane and waits on nothing slower than the summons
/// itself, and the original budgets its remote attach at timeout + 15s.
pub const FEDERATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(90);

pub struct TeamRequest {
    gate_open: Option<String>,
    pub team_id: String,
    /// The pane that ran the command, out of its own `TMUX_PANE`.
    pub pane: String,
    /// Per-pane capability. The window binds it to the terminal before
    /// believing the caller-provided pane header.
    pub pane_token: String,
    pub argv: Vec<String>,
    /// Where the answer goes. Dropped without a send means the window went
    /// away mid-request, which the handler turns into a refusal rather than a
    /// hang — see [`TeamAnswer`].
    pub answer: tokio::sync::oneshot::Sender<TeamAnswer>,
    /// A long-poll credit minted by the bridge. It travels through the bounded
    /// queue and the window's blocking closure, so queued plus active waits can
    /// never consume the control half of the ingress road.
    _wait_permit: Option<Seat>,
}

impl TeamRequest {
    pub fn new(
        team_id: String,
        pane: String,
        pane_token: String,
        argv: Vec<String>,
        answer: tokio::sync::oneshot::Sender<TeamAnswer>,
    ) -> Self {
        Self {
            gate_open: None,
            team_id,
            pane,
            pane_token,
            argv,
            answer,
            _wait_permit: None,
        }
    }

    pub fn gate_open(&self) -> Option<&str> {
        self.gate_open.as_deref()
    }

    fn with_wait_permit(mut self, permit: Seat) -> Self {
        self._wait_permit = Some(permit);
        self
    }

    /// Whether this request deliberately occupies a long-running seat.
    ///
    /// Kept beside the request and its limits so the bridge, the window, and
    /// tests do not grow separate ideas of which command consumes the reserved
    /// wait quota. Four verbs qualify: a `check` that said `--wait`, every
    /// `ask`, `worker-start`, whose answer now waits for the spawned TUI to
    /// accept its briefing, and `delegate`, which is a `worker-start` that also
    /// writes the work and the worker's first letter (t-34501). All are
    /// orchestration words; tmux has no bare command with any of these names.
    #[must_use]
    pub fn reserves_wait_slot(&self) -> bool {
        let verb = self.argv.first();
        verb.is_some_and(|verb| verb == "ask")
            || verb.is_some_and(|verb| verb == "worker-start")
            || verb.is_some_and(|verb| verb == "delegate")
            || (verb.is_some_and(|verb| verb == "check")
                && self.argv.iter().skip(1).any(|word| word == "--wait"))
    }

    /// Whether this request asks the window to seat a worker: `worker-start`
    /// and `delegate`, whose answers come after the worker's readiness.
    #[must_use]
    pub fn summons_a_worker(&self) -> bool {
        self.argv
            .first()
            .is_some_and(|verb| verb == "worker-start" || verb == "delegate")
    }

    /// How long the bridge holds this request's answer open.
    ///
    /// The ladder's middle rung, now per request: a wait that brought its own
    /// `--timeout-ms` gets that budget plus [`BRIDGE_GRACE`], so the sleeper
    /// (who holds exactly the budget) still gives up first and silence still
    /// comes back as `{"count":0}` instead of "timed out waiting for the
    /// window". The raw value is capped defensively — the ledger refuses
    /// out-of-range budgets on its own, but this bridge must not hold a
    /// socket open for a number it never checked. Everything that is not a
    /// short command keeps [`TEAM_DEADLINE`]. Budget-less `ask` and
    /// `worker-start` use their ledger defaults and must still finish before
    /// this bridge does.
    #[must_use]
    pub fn deadline(&self) -> std::time::Duration {
        if !self.reserves_wait_slot() {
            return TEAM_DEADLINE;
        }
        // `--timeout-ms X` and `--timeout-ms=X` are one thing to the ledger (the later one wins), so
        // they are one thing here; reading only the two-word form held a `=` spelling for the short
        // default while the window kept its wait.
        let mut budget: Option<u64> = None;
        for (at, word) in self.argv.iter().enumerate() {
            let value = if word == "--timeout-ms" {
                self.argv.get(at + 1).map(String::as_str)
            } else {
                word.strip_prefix("--timeout-ms=")
            };
            if let Some(ms) = value.and_then(|value| value.parse::<u64>().ok()) {
                budget = Some(ms.min(WAIT_BUDGET_CEILING_MS));
            }
        }
        // A `delegate --wait` answers after the pane is opened and the worker has its briefing
        // (up to the ready default) AND THEN the wait the caller asked for — so the bridge holds
        // the preparation on top of the wait, or it gives up before a window that is still right.
        let preparation = if self.argv.first().is_some_and(|verb| verb == "delegate")
            && self.argv.iter().skip(1).any(|word| word == "--wait")
        {
            std::time::Duration::from_millis(u64::from(
                zerocode_core::orchestration::delegate::WAIT_HEADROOM_MS,
            ))
        } else {
            std::time::Duration::ZERO
        };
        // A summons is answered when the window has either seated the worker or worded why it
        // could not, and that can come this long after the readiness budget (see the constant).
        // The bridge that hung up at the budget handed the caller a time-out for a failure the
        // window then worded into a closed answer (t-76890).
        let summoning = self.summons_a_worker();
        let report = if summoning {
            std::time::Duration::from_millis(u64::from(
                zerocode_core::orchestration::READY_REPORT_HEADROOM_MS,
            ))
        } else {
            std::time::Duration::ZERO
        };
        match budget {
            Some(ms) => std::time::Duration::from_millis(ms) + preparation + report + BRIDGE_GRACE,
            None if self.argv.first().is_some_and(|verb| verb == "ask") => {
                std::time::Duration::from_millis(u64::from(
                    zerocode_core::orchestration::ASK_BUDGET_DEFAULT_MS,
                )) + BRIDGE_GRACE
            }
            None if summoning => {
                std::time::Duration::from_millis(u64::from(
                    zerocode_core::orchestration::READY_TIMEOUT_DEFAULT_MS,
                )) + report
                    + BRIDGE_GRACE
            }
            None => TEAM_DEADLINE,
        }
    }
}

/// What the shim prints and exits with.
#[derive(Debug, Clone)]
pub struct TeamAnswer {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl TeamAnswer {
    /// A busy refusal that names the door that said no (t-47458).
    #[must_use]
    pub fn busy(busy: &Busy) -> Self {
        Self {
            stdout: String::new(),
            stderr: busy.message(),
            exit_code: 1,
        }
    }
}

/// One `zerocode-browser …` from an agent's pane — the browser bridge's
/// request, answered with the same stdout/stderr/exit contract the teams
/// shim reads ([`TeamAnswer`]), because both shims are the same animal: a
/// blocked CLI waiting on the window.
pub struct BrowserRequest {
    pub argv: Vec<String>,
    /// The original agent observation's control revision, validated before
    /// queueing. Omission is preserved; the receiver must not fill it in.
    pub workspace_revision: Option<u64>,
    /// The pane key (`term-<n>`) of the shell that sent this, when the shim
    /// knew its own — a hand-typed shell outside a pane sends none.
    pub pane: Option<String>,
    /// The evidence folder of the automation run whose shell sent this, when
    /// that shell was given one — the window fences the path to its own
    /// automations tree before writing a frame or a step line into it.
    pub evidence: Option<String>,
    pub answer: BrowserAnswerSender,
}

/// Per-request browser metadata stays separate from page stdout and from the
/// legacy CLI reply. Only the authenticated bridge negotiates its wire form.
pub struct BrowserAnswerSender {
    inner: tokio::sync::oneshot::Sender<zerocode_core::surface_execution::ExecutionEnvelope>,
}

impl BrowserAnswerSender {
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    pub async fn closed(&mut self) {
        self.inner.closed().await;
    }

    pub fn send(
        self,
        answer: TeamAnswer,
    ) -> Result<(), zerocode_core::surface_execution::ExecutionEnvelope> {
        self.send_with_surface(answer, None)
    }

    pub fn send_with_surface(
        self,
        answer: TeamAnswer,
        surface_execution: Option<zerocode_core::surface_execution::SurfaceExecution>,
    ) -> Result<(), zerocode_core::surface_execution::ExecutionEnvelope> {
        self.inner
            .send(zerocode_core::surface_execution::ExecutionEnvelope {
                stdout: answer.stdout,
                stderr: answer.stderr,
                exit_code: answer.exit_code,
                surface_execution,
            })
    }
}

/// One `zerocode-computer …` request. The persistent window-side provider
/// owns the accessibility snapshot cache across these otherwise short-lived
/// shim processes.
pub struct ComputerRequest {
    pub argv: Vec<String>,
    /// As on [`BrowserRequest`]: the run's evidence folder, if the shell had one.
    pub evidence: Option<String>,
    /// The directory the shell that sent this stands in, when the door said
    /// one that reads back ([`zerocode_core::computer_use::cwd_from_header`]).
    /// The window believes it only as far as the Jev door asks: a walk is
    /// judged for the workspace it was asked from, and one asked from nowhere
    /// known is judged for none.
    pub cwd: Option<String>,
    /// As on [`BrowserRequest`]: the pane key of the shell that sent this,
    /// when its door names one (the emulator door does, so the mirror an
    /// agent opens is seated in that pane's checkout).
    pub pane: Option<String>,
    pub answer: tokio::sync::oneshot::Sender<TeamAnswer>,
}

/// The run evidence folder a shim presented, if any. Read before the body is
/// taken, empty strings dropped; the window decides whether to believe it.
fn run_evidence_of(request: &Request) -> Option<String> {
    request
        .headers()
        .get(zerocode_core::computer_use::RUN_EVIDENCE_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The pane a shim said it was asked from, if it said one. Read before the
/// body is taken, empty strings dropped, the way the evidence folder is: the
/// window decides what the key names.
fn pane_of(request: &Request) -> Option<String> {
    request
        .headers()
        .get(BROWSER_PANE_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The directory a shim said its caller stands in, if it said one that reads
/// back. Read before the body is taken, like the evidence folder.
fn cwd_of(request: &Request) -> Option<String> {
    request
        .headers()
        .get(zerocode_core::computer_use::CWD_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(zerocode_core::computer_use::cwd_from_header)
}

/// How long the shim is made to wait.
///
/// A split spawns a process and a frame; a capture reads a screen. None of it
/// is slow, and a leader blocked on us is a leader not working — so the wait
/// is bounded and a timeout is a refusal the agent can read rather than a
/// command that never returns.
pub const TEAM_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// What the bridge adds on top of a caller's own wait budget before it gives
/// up — the gap that keeps the sleeper answering first. The shim adds a
/// bigger number on the same base, so the ladder holds per request exactly
/// as it holds for the defaults; the three-rung test names all of them.
pub const BRIDGE_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// The most milliseconds a `--timeout-ms` may stretch this bridge, however
/// large the argv spells it. Matches the widest budget any ledger verb
/// accepts (the blocking ask's ceiling), and exists so a socket is never
/// held open on an unchecked number.
pub const WAIT_BUDGET_CEILING_MS: u64 = 1_800_000;

/// Every team effect running or waiting on its inbox, together.
pub const TEAM_ACTIVE_LIMIT: usize = 16;
/// Requests waiting between the HTTP bridge and the window. The window admits
/// at most another [`TEAM_ACTIVE_LIMIT`]; the pre-queue body/parse stage is
/// bounded independently by [`TEAM_BODY_READ_LIMIT`].
pub const TEAM_QUEUE_CAPACITY: usize = TEAM_ACTIVE_LIMIT;
/// Bodies being read before they can contend for the queue.
pub const TEAM_BODY_READ_LIMIT: usize = TEAM_QUEUE_CAPACITY;
/// Long-poll seats. Keeping this below the total reserves progress for the
/// `send` or control request that can end those waits.
pub const TEAM_WAIT_LIMIT: usize = TEAM_ACTIVE_LIMIT / 2;

pub const TEAM_CLOSED_MESSAGE: &str = "tmux: this window is no longer listening\n";

#[derive(Clone)]
pub struct BridgeState {
    token: Arc<String>,
    federation_token: Arc<FederationToken>,
    browser_token: Arc<String>,
    computer_token: Arc<String>,
    events: mpsc::UnboundedSender<HookEnvelope>,
    teams: mpsc::Sender<TeamRequest>,
    team_bodies: Seats,
    team_waits: Seats,
    browser: mpsc::UnboundedSender<BrowserRequest>,
    computer: mpsc::UnboundedSender<ComputerRequest>,
    federation: mpsc::UnboundedSender<FederationRequest>,
    selection_context: Arc<std::sync::Mutex<orchestration_contract::SelectionContextTracker>>,
    /// Where the window keeps the second brain's answer to a prompt — see
    /// [`PromptKnowledge`]. `None` until the window installs one.
    knowledge: Option<Arc<dyn PromptKnowledge>>,
    /// Where the window keeps what the seats say at a turn's start — see
    /// [`TurnBrief`]. `None` until the window installs one.
    brief: Option<Arc<dyn TurnBrief>>,
    brief_receipts: Arc<std::sync::Mutex<brief_receipts::Receipts>>,
    brief_acks: Arc<tokio::sync::Semaphore>,
    /// Where the window answers `zerocode-find` — see [`FileFind`].
    finder: Option<Arc<dyn FileFind>>,
    artifacts: Option<Arc<dyn ArtifactCommands>>,
    /// Where a fixed ledger pointer waits for a provider's own hook — see
    /// [`pointer_mailbox::PointerMailbox`]. `None` until the window installs
    /// one, and `None` is a bridge that simply never answers with a pointer.
    pointers: Option<Arc<dyn pointer_mailbox::PointerMailbox>>,
    /// The hook posts tried again, answered once ([`replays`]).
    replays: Arc<std::sync::Mutex<replays::Replays>>,
    /// Where a hook delivery that needed more than one try is written down
    /// for the window — see [`BridgeState::with_delivery_notes`].
    delivery_notes: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

/// The second brain's answer to a prompt, for the providers whose prompt hook
/// takes `hookSpecificOutput.additionalContext`.
///
/// The bridge does not know where the vault is or how its graph is kept; the
/// window does, and hands the bridge this one question to ask. The answer is
/// the block that goes in ahead of the turn — the pages the prompt is about,
/// chosen by the same graph every pane reads, so a Claude pane and a Codex
/// pane are shown the same pages for the same words. `None` says nothing.
pub trait PromptKnowledge: Send + Sync {
    /// `pane_key` is the pane the prompt was typed in (`term-<n>`), or empty
    /// when the bridge did not know one — the recall trace names it so the
    /// graph can say which pane a page was shown to.
    fn related_block(&self, session_key: &str, pane_key: &str, prompt: &str) -> Option<String>;
}

/// The most a hook script waits for this bridge's reply: its `curl
/// --max-time`, the connect counted in. A reply later than this is thrown
/// away whole — the contract, the vault's block and the pointer with it.
pub const HOOK_REPLY_BUDGET: std::time::Duration = std::time::Duration::from_millis(1_500);

/// The most of [`HOOK_REPLY_BUDGET`] a hook script spends reaching the
/// bridge: its `curl --connect-timeout`.
pub const HOOK_CONNECT_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);

/// What a reply keeps back for writing itself once every part is in.
pub const HOOK_REPLY_MARGIN: std::time::Duration = std::time::Duration::from_millis(200);

/// The pause a hook script takes before each post again (t-78634), when the
/// one before was given up on with its body possibly with the bridge — curl
/// 28, 52 or 56. A briefing's whole submission receipt rode one post, and a
/// bridge held up past [`HOOK_REPLY_BUDGET`] (the window under load, a reply
/// composed slowly) lost it: three worker launches were rolled back on one
/// day for a receipt that never came.
pub const HOOK_RETRY_PAUSES: [std::time::Duration; 3] = [
    std::time::Duration::from_millis(500),
    std::time::Duration::from_millis(1_000),
    std::time::Duration::from_millis(2_000),
];

/// The most a hook script spends posting, every try and pause counted. A
/// try that would not fit in what is left is not made ([`hook_tries`]).
pub const HOOK_DELIVERY_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// What an agent's own hook timeout keeps back beyond the posts — reading
/// stdin, sourcing the endpoint file, printing the reply — so a script that
/// tries again is never the hook its agent kills.
pub const HOOK_TIMEOUT_MARGIN: std::time::Duration = std::time::Duration::from_secs(1);

/// How much later than one try a hook's delivery can reach the window once
/// it is tried again: the budget past the first try. A wait for a receipt
/// that rides a hook adds this to its own.
pub const HOOK_RETRY_SPAN: std::time::Duration =
    HOOK_DELIVERY_BUDGET.saturating_sub(HOOK_REPLY_BUDGET);

/// One post of a hook script: the pause before it and its own `--max-time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookTry {
    pub pause: std::time::Duration,
    pub budget: std::time::Duration,
}

/// The posts one agent's hook script may make, laid out so that the worst
/// case — every try timing out — ends within [`HOOK_DELIVERY_BUDGET`] and
/// within the agent's own hook timeout less [`HOOK_TIMEOUT_MARGIN`]. The
/// first try is the whole [`HOOK_REPLY_BUDGET`]; each later one waits its
/// pause and takes what is left, up to the same budget, and is made only
/// while what is left can still reach the bridge ([`HOOK_CONNECT_BUDGET`]).
#[must_use]
pub fn hook_tries(agent: AgentKind) -> Vec<HookTry> {
    let budget =
        HOOK_DELIVERY_BUDGET.min(install::hook_timeout(agent).saturating_sub(HOOK_TIMEOUT_MARGIN));
    let mut tries = vec![HookTry {
        pause: std::time::Duration::ZERO,
        budget: HOOK_REPLY_BUDGET.min(budget),
    }];
    let mut spent = HOOK_REPLY_BUDGET.min(budget);
    for pause in HOOK_RETRY_PAUSES {
        let left = budget.saturating_sub(spent + pause);
        if left < HOOK_CONNECT_BUDGET {
            break;
        }
        let try_budget = HOOK_REPLY_BUDGET.min(left);
        tries.push(HookTry {
            pause,
            budget: try_budget,
        });
        spent += pause + try_budget;
    }
    tries
}

/// The curl exits after which a hook script posts again: it stopped waiting
/// (28), or the answer was lost (52, 56) — each with the body possibly with
/// the bridge, which is why a post again carries the same delivery id and
/// the bridge answers it once. A bridge that is not there (7) or refused the
/// payload (22) is not asked again: an agent outside a live window would
/// pay the pauses on every hook.
pub const HOOK_RETRY_EXITS: [u8; 3] = [28, 52, 56];

/// [`HOOK_RETRY_EXITS`] as a POSIX `case` pattern.
fn posix_retry_exits() -> String {
    HOOK_RETRY_EXITS
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join("|")
}

/// How long a turn's brief may take (t-14869): the reply's budget less the
/// connect's and the reply's own margin, so a late brief is only a brief
/// left out and never a reply thrown away. A seat whose own wall is shorter
/// waits its own.
pub const TURN_BRIEF_WALL: std::time::Duration = HOOK_REPLY_BUDGET
    .saturating_sub(HOOK_CONNECT_BUDGET)
    .saturating_sub(HOOK_REPLY_MARGIN);

/// The most characters a turn's brief may carry: under the smallest context
/// any measured provider takes from a hook — Codex's `additionalContextLimit`,
/// about 2,500 tokens by default, which no tokenizer makes fewer than as
/// many characters; Claude's is 10,000 characters. A longer brief is left
/// out whole, never cut.
pub const TURN_BRIEF_CHAR_CAP: usize = 2_000;

/// What the seats say at a turn's start, for the providers whose prompt hook
/// takes `hookSpecificOutput.additionalContext` (t-14869) — the file pick
/// seat's likely files, while it acts for the pane.
///
/// The bridge does not know the seats, the ledger or which pane is a worker;
/// the window does, and answers within [`TurnBriefAsk::wall`]. `None` says
/// nothing — no seat acts, the answer abstained, came late or was refused —
/// and the reply is what it was before this trait existed. The words are a
/// fact the model may use or ignore, never an order, and the reply carries
/// them as context only: no decision, no permission, no changed input.
pub trait TurnBrief: Send + Sync {
    fn brief(&self, ask: TurnBriefAsk) -> Option<String>;
    /// Whether this text has a deferred effect that needs client acknowledgment.
    fn needs_receipt(&self, _text: &str) -> bool {
        false
    }
    /// Commit only after the matching hook client has received and forwarded
    /// the response. An error permits another acknowledgment until the slot
    /// expires; installed hook scripts make one attempt.
    ///
    /// # Errors
    /// The host could not persist its acknowledgment.
    fn delivered(&self, _ask: &TurnBriefAsk, _text: &str) -> io::Result<()> {
        Ok(())
    }
}

/// The person's prompt as this bridge reads it for a turn's context — `None`
/// for an event that is not the agent's prompt event, or an agent whose row
/// takes no context at a turn's start. The window names a turn by it, so a
/// brief and the books it waits on read the same words.
#[must_use]
pub fn turn_prompt(envelope: &HookEnvelope) -> Option<String> {
    orchestration_contract::prompt_submission(envelope).map(|submission| submission.prompt)
}

/// One turn's start, as a brief is asked about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnBriefAsk {
    pub agent: AgentKind,
    /// The pane the prompt was typed in (`term-<n>`).
    pub pane_key: String,
    /// The launch the knock says it belongs to; empty from an older script.
    pub launch_token: String,
    /// The provider's validated session identity from this exact prompt event.
    /// A repeated prompt in a new session must not match the previous book.
    pub session_id: Option<String>,
    pub supports_receipts: bool,
    /// The folder the pane works in.
    pub worktree: String,
    pub prompt: String,
    /// How long the answer may take.
    pub wall: std::time::Duration,
}

/// `zerocode-find`, answered by the window (t-14869): the files of the pane's
/// project most likely involved in what the agent is about to change or
/// debug — the bridge only authenticates, reads the argv and forwards. The
/// answer is the command's output; an error is its refusal.
pub trait FileFind: Send + Sync {
    /// # Errors
    /// Why the window could not look.
    fn find(&self, ask: zerocode_core::file_find::FindAsk) -> Result<String, String>;
}

/// The window owns the catalog; the HTTP bridge only authenticates and forwards.
pub trait ArtifactCommands: Send + Sync {
    fn execute(&self, request: serde_json::Value) -> Result<serde_json::Value, String>;
}

pub type BridgeReceivers = (
    BridgeState,
    mpsc::UnboundedReceiver<HookEnvelope>,
    mpsc::Receiver<TeamRequest>,
    mpsc::UnboundedReceiver<BrowserRequest>,
);

pub type BridgeReceiversWithComputer = (
    BridgeState,
    mpsc::UnboundedReceiver<HookEnvelope>,
    mpsc::Receiver<TeamRequest>,
    mpsc::UnboundedReceiver<BrowserRequest>,
    mpsc::UnboundedReceiver<ComputerRequest>,
    mpsc::UnboundedReceiver<FederationRequest>,
);

impl BridgeState {
    pub fn new(token: impl Into<String>, browser_token: impl Into<String>) -> BridgeReceivers {
        let token = token.into();
        let browser_token = browser_token.into();
        let (state, rx, team_rx, browser_rx, _computer_rx, _federation_rx) =
            Self::new_with_computer(token, browser_token, String::new());
        (state, rx, team_rx, browser_rx)
    }

    #[must_use]
    pub fn with_artifacts(mut self, artifacts: Arc<dyn ArtifactCommands>) -> Self {
        self.artifacts = Some(artifacts);
        self
    }

    /// Install the window's second brain as the answer to prompt hooks.
    #[must_use]
    pub fn with_prompt_knowledge(mut self, source: Arc<dyn PromptKnowledge>) -> Self {
        self.knowledge = Some(source);
        self
    }

    /// Install the window's turn brief as a further answer to prompt hooks.
    #[must_use]
    pub fn with_turn_brief(mut self, source: Arc<dyn TurnBrief>) -> Self {
        self.brief = Some(source);
        self
    }

    /// Install the window's answer to `zerocode-find`.
    #[must_use]
    pub fn with_file_find(mut self, finder: Arc<dyn FileFind>) -> Self {
        self.finder = Some(finder);
        self
    }

    /// Install the window's orchestration pointer mailbox.
    ///
    /// Without one this bridge answers hooks exactly as it always did. With
    /// one, a pane whose provider was measured to honour a turn-end
    /// continuation can be handed the fixed ledger pointer at the moment its
    /// own turn ends — the one road that reaches a working agent with no
    /// keystroke and no person.
    #[must_use]
    pub fn with_pointer_mailbox(
        mut self,
        mailbox: Arc<dyn pointer_mailbox::PointerMailbox>,
    ) -> Self {
        self.pointers = Some(mailbox);
        self
    }

    /// Install where a hook delivery that needed more than one try is
    /// written down (t-78634): one line per post again, with the try and the
    /// curl exits before it, so the posts a slow bridge lost can be counted.
    /// Called off the async runtime.
    #[must_use]
    pub fn with_delivery_notes(mut self, note: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        self.delivery_notes = Some(note);
        self
    }

    /// The window's own UI submits through the same consumer as authenticated
    /// CLI requests. This in-process handle exposes no capability token and
    /// does not skip the consumer's sequence, permission or action guards.
    #[must_use]
    pub fn computer_requests(&self) -> mpsc::UnboundedSender<ComputerRequest> {
        self.computer.clone()
    }

    pub fn new_with_computer(
        token: impl Into<String>,
        browser_token: impl Into<String>,
        computer_token: impl Into<String>,
    ) -> BridgeReceiversWithComputer {
        let token = token.into();
        let federation_token = federation_token(&token);
        let (events, rx) = mpsc::unbounded_channel();
        let (teams, team_rx) = mpsc::channel(TEAM_QUEUE_CAPACITY);
        let (browser, browser_rx) = mpsc::unbounded_channel();
        let (computer, computer_rx) = mpsc::unbounded_channel();
        let (federation, federation_rx) = mpsc::unbounded_channel();
        (
            Self {
                token: Arc::new(token),
                federation_token: Arc::new(federation_token),
                browser_token: Arc::new(browser_token.into()),
                computer_token: Arc::new(computer_token.into()),
                events,
                teams,
                team_bodies: Seats::new(BusyEntry::BodyPermit, TEAM_BODY_READ_LIMIT),
                team_waits: Seats::new(BusyEntry::WaitPermit, TEAM_WAIT_LIMIT),
                browser,
                computer,
                federation,
                knowledge: None,
                brief: None,
                brief_receipts: Arc::new(
                    std::sync::Mutex::new(brief_receipts::Receipts::default()),
                ),
                brief_acks: Arc::new(tokio::sync::Semaphore::new(4)),
                finder: None,
                artifacts: None,
                pointers: None,
                replays: Arc::new(std::sync::Mutex::new(replays::Replays::default())),
                delivery_notes: None,
                selection_context: Arc::new(std::sync::Mutex::new(
                    orchestration_contract::SelectionContextTracker::default(),
                )),
            },
            rx,
            team_rx,
            browser_rx,
            computer_rx,
            federation_rx,
        )
    }
}

/// Form body posted by the generated scripts. Field names match
/// [`HookEnvelope`] exactly; unknown fields are ignored so a newer script
/// talking to an older bridge degrades instead of failing.
#[derive(Debug, Deserialize)]
struct HookForm {
    pane_key: String,
    #[serde(default)]
    tab_id: String,
    #[serde(default)]
    launch_token: String,
    #[serde(default)]
    worktree_id: String,
    #[serde(default)]
    env: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    hook_event_name: String,
    #[serde(default)]
    selection_seeded: String,
    #[serde(default)]
    delivery_id: String,
    /// Which try of the script's this post is ([`hook_tries`]), and the
    /// curl exits the tries before it gave up with — empty from a first try
    /// and from an older script.
    #[serde(default, rename = "try")]
    attempt: String,
    #[serde(default)]
    gave_up: String,
    payload: String,
}

/// The same envelope, posted as JSON.
///
/// The shell scripts send a form because `curl --data-urlencode` is the one
/// thing every machine has; the PLUGIN agents run inside their host's Node
/// process and send JSON through `fetch`, with `payload` as an OBJECT rather
/// than a string — it is already structured there, and re-encoding it to a
/// string inside the plugin would only be undone here.
///
/// The field names are the plugin's own spelling (`paneKey`, `launchToken`):
/// those plugins are JavaScript, and asking a JS author to write snake_case is
/// how a field silently arrives empty.
/// `rename_all = "camelCase"` with a snake_case ALIAS on each field: the
/// plugins write `paneKey`, and a hand-written client that reaches for the same
/// spelling the shell scripts use should also work. Getting this backwards — an
/// alias equal to the field's own name — makes the camelCase form unrecognized
/// and every plugin post a 422, which is exactly how it was written first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookJson {
    #[serde(alias = "pane_key")]
    pane_key: String,
    #[serde(default, alias = "tab_id")]
    tab_id: String,
    #[serde(default, alias = "launch_token")]
    launch_token: String,
    #[serde(default, alias = "worktree_id")]
    worktree_id: String,
    #[serde(default)]
    env: String,
    #[serde(default)]
    version: String,
    #[serde(default, alias = "hook_event_name")]
    hook_event_name: String,
    #[serde(default, alias = "selection_seeded")]
    selection_seeded: bool,
    #[serde(default, alias = "delivery_id")]
    delivery_id: String,
    /// Any JSON — object, string, whatever the host handed the plugin. Carried
    /// on as text because that is what a payload IS to this bridge: the
    /// agent's own schema, which it is not our business to parse.
    #[serde(default)]
    payload: serde_json::Value,
}

pub fn router(state: BridgeState) -> Router {
    Router::new()
        .route("/hook/{agent}", post(receive_hook))
        .route("/hook-receipt/{agent}", post(receive_hook_receipt))
        // The orchestration road. Same server, same token, same gate — a
        // second listener for a second protocol would be a second secret to
        // keep and a second port to explain.
        .route("/agent-teams", post(receive_team_command))
        .route("/gate-sessions/open", post(receive_gate_open))
        // The browser road. Same server, same token, same reasons — and no
        // team headers, because a browser pane belongs to the window rather
        // than to any one team.
        .route("/browser", post(receive_browser_command))
        // The federation road: another MACHINE's window borrowing this
        // one's panes. Its transport token is scoped to this route; the home
        // fingerprint header is the OWNERSHIP key the ledger checks per
        // dispatch, so a second home on the same wire still cannot read the
        // first one's relay.
        .route(FEDERATION_PATH, post(receive_federation_call))
        // Desktop control is kept on the same loopback listener, but behind a
        // third capability token carried only by launched agent panes.
        .route("/computer", post(receive_computer_command))
        .route(
            zerocode_core::artifact_publish::ROUTE,
            post(receive_artifact_command),
        )
        // The file pick seat, called by the agent itself (t-14869).
        .route(zerocode_core::file_find::ROUTE, post(receive_find_command))
        .layer(DefaultBodyLimit::max(MAX_HOOK_BODY_BYTES))
        // Outermost, so it runs before routing and before any extractor: an
        // unauthenticated request is turned away with its body still unread.
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

/// One federation call: fingerprint header required, body handed whole to
/// the window, answer streamed back verbatim. 503 when the window is gone
/// or too slow — the home retries by replaying the same durable request.
async fn receive_federation_call(State(state): State<BridgeState>, request: Request) -> Response {
    let home = request
        .headers()
        .get(FEDERATION_HOME_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if home.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing federation home header").into_response();
    }
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "body too large").into_response(),
    };
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "body is not JSON").into_response(),
    };
    let (answer, answered) = tokio::sync::oneshot::channel();
    if state
        .federation
        .send(FederationRequest { home, body, answer })
        .is_err()
    {
        return (StatusCode::SERVICE_UNAVAILABLE, "window is gone").into_response();
    }
    match tokio::time::timeout(FEDERATION_DEADLINE, answered).await {
        Ok(Ok(said)) => (StatusCode::OK, said).into_response(),
        Ok(Err(_)) => {
            (StatusCode::SERVICE_UNAVAILABLE, "window went away mid-call").into_response()
        }
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "window too slow").into_response(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenRequirement {
    Bridge,
    Federation,
}

/// The one path-to-grant table for this listener. Everything is bridge-local
/// unless named here: adding a route cannot accidentally inherit the smaller
/// federation grant.
fn token_requirement(path: &str) -> TokenRequirement {
    match path {
        FEDERATION_PATH => TokenRequirement::Federation,
        _ => TokenRequirement::Bridge,
    }
}

fn token_authorizes(path: &str, presented: &[u8], state: &BridgeState) -> bool {
    let bridge = constant_time_eq(presented, state.token.as_bytes());
    match token_requirement(path) {
        TokenRequirement::Bridge => bridge,
        // Keep the old bridge-wide bearer working on /federation while saved
        // address books migrate. New invitations hand out only the scoped
        // value, which cannot satisfy any other path requirement.
        TokenRequirement::Federation => {
            constant_time_eq(presented, state.federation_token.as_str().as_bytes()) | bridge
        }
    }
}

/// The auth gate. Reads only the path and headers, then either hands the
/// request on or answers 401 — the body is never polled on the rejection path.
async fn require_token(State(state): State<BridgeState>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(HOOK_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !token_authorizes(request.uri().path(), presented.as_bytes(), &state) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// Is this request's body JSON?
///
/// Asked of the header rather than by trying one parse and falling back to the
/// other: a body can only be consumed once, so "try form, then try JSON" means
/// buffering it to retry — and the reason the auth gate is a layer is precisely
/// that this bridge does not buffer bodies it has not decided to read.
fn body_is_json(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
}

/// Answer one `tmux …` from an orchestrating agent's fake tmux.
///
/// Unlike the hook road, this one **replies**: the agent is blocked on the
/// answer, and what it reads decides whether it believes a teammate started.
/// So the shape is a request with a `oneshot` back-channel rather than a
/// fire-and-forget envelope, and every road out of here writes a status the
/// shim can turn into an exit code.
///
/// The body is the argv, NUL-of-a-kind separated (`agent_teams::ARGV_SEPARATOR`)
/// — not JSON, because the shim is a POSIX shell script and a teammate prompt
/// with a quote in it is exactly what a hand-rolled JSON encoder gets wrong.
async fn receive_team_command(State(state): State<BridgeState>, request: Request) -> Response {
    let headers = request.headers();
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let team_id = text(TEAM_ID_HEADER);
    let pane = text(TEAM_PANE_HEADER);
    let pane_token = text(TEAM_TOKEN_HEADER);
    if team_id.is_empty() || pane.is_empty() || pane_token.is_empty() {
        return refused("tmux: this shell is not part of an agent team\n");
    }
    // Admission starts BEFORE buffering. The body has a byte ceiling too, but
    // a ceiling per handler is not a process bound if arbitrarily many slow
    // authenticated bodies can all sit below it at once.
    let body_permit = match state.team_bodies.try_acquire() {
        Ok(permit) => permit,
        Err(busy) => return busy_response(&busy),
    };
    let body = match axum::body::to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return refused("tmux: unreadable command\n"),
    };
    let (answer, wait) = tokio::sync::oneshot::channel();
    let argv = zerocode_core::agent_teams::unpack_argv(&body);
    // `argv` owns every parsed word. Give the original up-to-1 MiB buffer back
    // before this request can enter the queue or wait for its answer; otherwise
    // body admission would be returned while the body it accounted for lived
    // through the whole ten-second handler.
    drop(body);
    let mut team_request = TeamRequest::new(team_id, pane, pane_token, argv, answer);
    // Read before the request is moved into the queue: the deadline is the
    // request's own fact, and the only reader left after the send is this
    // handler's wait.
    let patience = team_request.deadline();
    let summoning = team_request.summons_a_worker();
    if team_request.reserves_wait_slot() {
        let permit = match state.team_waits.try_acquire() {
            Ok(permit) => permit,
            Err(busy) => return busy_response(&busy),
        };
        team_request = team_request.with_wait_permit(permit);
    }
    match state.teams.try_send(team_request) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            return busy_response(&Busy::full_queue(TEAM_QUEUE_CAPACITY));
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            return (StatusCode::SERVICE_UNAVAILABLE, TEAM_CLOSED_MESSAGE).into_response();
        }
    }
    // The parsed argv is now accounted for by the bounded queue/active limits;
    // body admission protects only the pre-queue buffering stage.
    drop(body_permit);
    match tokio::time::timeout(patience, wait).await {
        Ok(Ok(answer)) => {
            if answer.exit_code == 0 {
                (StatusCode::OK, answer.stdout).into_response()
            } else {
                // The shim reads a failed request's BODY (`--fail-with-body`)
                // and prints it on stderr, so the reason travels as the body
                // rather than being lost with the status line.
                refused(answer.stderr)
            }
        }
        // The window took the request and never answered — a pane that could
        // not be cut, or a shell that will not start.
        Ok(Err(_)) => refused("tmux: the window dropped the command\n"),
        // This ends only the HTTP wait. An effect already admitted by the
        // window is not cancelled by dropping this receiver and may still
        // finish. Its outcome is unknown to this layer; a retry protocol must
        // reconcile it rather than infer failure from the timeout.
        // For a summons that is not the whole story: past the window's own readiness deadline
        // (see `READY_REPORT_HEADROOM_MS`) the worker may still be standing, so the caller is told
        // where to look instead of being left to ask again under a new name and cut a second pane.
        Err(_) if summoning => refused(
            "tmux: timed out waiting for the window — the outcome is not settled and the worker \
             may still be coming up; read `worker-list --run <the run you summoned it into>` \
             (the plain list shows only the run this pane is bound to, so a worker started with \
             --run is missing from it), and to ask again repeat the original --retry-request \
             name rather than a new one\n",
        ),
        Err(_) => refused("tmux: timed out waiting for the window\n"),
    }
}

async fn receive_gate_open(State(state): State<BridgeState>, request: Request) -> Response {
    let (team, pane, capability) = {
        let headers = request.headers();
        let text = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        (
            text(TEAM_ID_HEADER),
            text(TEAM_PANE_HEADER),
            text(TEAM_TOKEN_HEADER),
        )
    };
    if team.is_empty() || pane.is_empty() || capability.is_empty() {
        return refused("authority_unavailable: native gate owner");
    }
    let permit = match state.team_bodies.try_acquire() {
        Ok(permit) => permit,
        Err(busy) => return busy_response(&busy),
    };
    let body = match axum::body::to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(bytes) => match String::from_utf8(bytes.to_vec()) {
            Ok(value) => value,
            Err(_) => return refused("invalid_request: native gate declaration"),
        },
        Err(_) => return refused("invalid_request: native gate declaration"),
    };
    let (answer, wait) = tokio::sync::oneshot::channel();
    let mut request = TeamRequest::new(
        team,
        pane,
        capability,
        vec!["native-gate-open".into()],
        answer,
    );
    request.gate_open = Some(body);
    if state.teams.try_send(request).is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, TEAM_CLOSED_MESSAGE).into_response();
    }
    drop(permit);
    match tokio::time::timeout(TEAM_DEADLINE, wait).await {
        Ok(Ok(answer)) if answer.exit_code == 0 => (StatusCode::OK, answer.stdout).into_response(),
        Ok(Ok(answer)) => refused(answer.stderr),
        _ => refused("authority_unavailable: native gate owner unavailable"),
    }
}

/// The busy kind — 429 — with the door that refused in its words (t-47458).
fn busy_response(busy: &Busy) -> Response {
    (StatusCode::TOO_MANY_REQUESTS, busy.message()).into_response()
}

fn refused(why: impl Into<String>) -> Response {
    (StatusCode::CONFLICT, why.into()).into_response()
}

fn workspace_revision_of(request: &Request) -> Result<Option<u64>, String> {
    use zerocode_core::agent_browser::{WORKSPACE_REVISION_HEADER, parse_workspace_revision};
    let headers = request.headers().get_all(WORKSPACE_REVISION_HEADER);
    let mut values = headers.iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err("workspace_revision_invalid: repeated workspace revision header".into());
    }
    let raw = value
        .to_str()
        .map_err(|_| "workspace_revision_invalid: expected an unsigned decimal u64")?;
    parse_workspace_revision(raw).map(Some)
}

/// Answer one `zerocode-browser …`. The same reply contract as the teams
/// road: the agent is blocked on this, so every road out writes a status the
/// shim turns into an exit code, and the wait is bounded.
///
/// Checked against ITS OWN token on top of the outer gate: the hook token is
/// in every PTY child's env, and "reports lifecycle events" must not imply
/// "reads pages" (1-g4 리뷰 발견 1).
async fn receive_browser_command(State(state): State<BridgeState>, request: Request) -> Response {
    use zerocode_core::surface_execution::EXECUTION_ENVELOPE_HEADER;
    let presented = request
        .headers()
        .get(BROWSER_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !constant_time_eq(presented.as_bytes(), state.browser_token.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut execution_headers = request.headers().get_all(EXECUTION_ENVELOPE_HEADER).iter();
    let execution_envelope = execution_headers.next().is_some_and(|value| value == "1")
        && execution_headers.next().is_none();
    let workspace_revision = match workspace_revision_of(&request) {
        Ok(revision) => revision,
        Err(error) => return refused(format!("zerocode-browser: {error}\n")),
    };
    let evidence = run_evidence_of(&request);
    let pane = pane_of(&request);
    let body = match axum::body::to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return refused("zerocode-browser: unreadable command\n"),
    };
    let command = match zerocode_core::agent_browser::parse_workspace_command(
        &zerocode_core::agent_teams::unpack_argv(&body),
        workspace_revision,
    ) {
        Ok(command) => command,
        Err(error) => return refused(format!("zerocode-browser: {error}\n")),
    };
    let (answer, wait) = tokio::sync::oneshot::channel();
    let sent = state.browser.send(BrowserRequest {
        argv: command.argv,
        workspace_revision: command.workspace_revision,
        pane,
        evidence,
        answer: BrowserAnswerSender { inner: answer },
    });
    if sent.is_err() {
        return refused("zerocode-browser: this window is no longer listening\n");
    }
    match tokio::time::timeout(TEAM_DEADLINE, wait).await {
        Ok(Ok(answer)) => {
            if answer.exit_code == 0 {
                if execution_envelope {
                    match serde_json::to_string(&answer) {
                        Ok(body) => (StatusCode::OK, [(EXECUTION_ENVELOPE_HEADER, "1")], body)
                            .into_response(),
                        Err(_) => refused("zerocode-browser: could not encode execution reply\n"),
                    }
                } else {
                    (StatusCode::OK, answer.stdout).into_response()
                }
            } else {
                refused(answer.stderr)
            }
        }
        Ok(Err(_)) => refused("zerocode-browser: the window dropped the command\n"),
        Err(_) => refused("zerocode-browser: timed out waiting for the window\n"),
    }
}

/// Answer one launched agent's Computer Use command. The outer hook token
/// proves this is one of our panes; the dedicated token proves the pane was
/// launched as an agent rather than opened as a person's ordinary shell.
async fn receive_computer_command(State(state): State<BridgeState>, request: Request) -> Response {
    let presented = request
        .headers()
        .get(COMPUTER_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !constant_time_eq(presented.as_bytes(), state.computer_token.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let evidence = run_evidence_of(&request);
    let cwd = cwd_of(&request);
    let pane = pane_of(&request);
    let body = match axum::body::to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return refused(format!("{COMPUTER_CLI}: unreadable command\n")),
    };
    let (answer, wait) = tokio::sync::oneshot::channel();
    let argv = zerocode_core::agent_teams::unpack_argv(&body);
    // The one ladder: a handoff or a press the person is asked about waits
    // as long as the person may take, not the machine's deadline.
    let deadline_ms = zerocode_core::computer_use::computer_deadline_ms(&argv);
    if state
        .computer
        .send(ComputerRequest {
            argv,
            evidence,
            cwd,
            pane,
            answer,
        })
        .is_err()
    {
        return refused(format!(
            "{COMPUTER_CLI}: this window is no longer listening\n"
        ));
    }
    match tokio::time::timeout(std::time::Duration::from_millis(deadline_ms), wait).await {
        Ok(Ok(answer)) if answer.exit_code == 0 => (StatusCode::OK, answer.stdout).into_response(),
        Ok(Ok(answer)) => refused(answer.stderr),
        Ok(Err(_)) => refused(format!("{COMPUTER_CLI}: the window dropped the command\n")),
        Err(_) => refused(format!(
            "{COMPUTER_CLI}: timed out waiting for the window\n"
        )),
    }
}

async fn receive_artifact_command(State(state): State<BridgeState>, request: Request) -> Response {
    let Some(publisher) = state.artifacts else {
        return refused("artifact store is unavailable");
    };
    let body = match axum::body::to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return refused("unreadable artifact command"),
    };
    let request = match zerocode_core::artifact_publish::request_from_argv(
        &zerocode_core::agent_teams::unpack_argv(&body),
    ) {
        Ok(value) => value,
        Err(error) => return refused(error),
    };
    match tokio::task::spawn_blocking(move || publisher.execute(request)).await {
        Ok(Ok(value)) => (StatusCode::OK, axum::Json(value)).into_response(),
        Ok(Err(error)) => refused(error),
        Err(error) => refused(error.to_string()),
    }
}

/// Answer one `zerocode-find`: its argv read here, the window's listing sent
/// back whole, a refusal the shim prints on stderr.
async fn receive_find_command(State(state): State<BridgeState>, request: Request) -> Response {
    let Some(finder) = state.finder else {
        return refused("file find is unavailable");
    };
    let body = match axum::body::to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return refused("unreadable file find"),
    };
    let ask = match zerocode_core::file_find::ask_from_argv(
        &zerocode_core::agent_teams::unpack_argv(&body),
    ) {
        Ok(ask) => ask,
        Err(usage) => return refused(usage),
    };
    match tokio::task::spawn_blocking(move || finder.find(ask)).await {
        Ok(Ok(listing)) => (StatusCode::OK, listing).into_response(),
        Ok(Err(error)) => refused(error),
        Err(error) => refused(error.to_string()),
    }
}

#[derive(Deserialize)]
struct BriefAcknowledgment {
    pane_key: String,
    launch_token: String,
    delivery_id: String,
}

async fn receive_hook_receipt(
    UrlPath(agent): UrlPath<String>,
    State(state): State<BridgeState>,
    Form(body): Form<BriefAcknowledgment>,
) -> Response {
    let Some(agent) = AgentKind::from_slug(&agent) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let key = brief_receipts::Key {
        agent,
        pane: body.pane_key,
        launch: body.launch_token,
        id: body.delivery_id,
    };
    if !key.valid() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(source) = state.brief.as_ref().map(Arc::clone) else {
        return StatusCode::ACCEPTED.into_response();
    };
    let Ok(permit) = state.brief_acks.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let ticket = state
        .brief_receipts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take(&key, std::time::Instant::now());
    let Some((ask, text)) = ticket else {
        return StatusCode::ACCEPTED.into_response();
    };
    // The effect runs on a bounded worker, after the client attests successful
    // stdout delivery. A producer abandoned by the brief timer cannot get here.
    let held = state.brief_receipts.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = source.delivered(&ask, &text);
        held.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish(&key, result.is_err().then_some((ask, text)));
        result
    })
    .await;
    if matches!(result, Ok(Ok(()))) {
        StatusCode::ACCEPTED.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

async fn receive_hook(
    UrlPath(agent): UrlPath<String>,
    State(state): State<BridgeState>,
    request: Request,
) -> Response {
    let Some(agent) = AgentKind::from_slug(&agent) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let json = body_is_json(request.headers());
    let (envelope, selection_seeded, delivery_id, tried) = if json {
        match axum::Json::<HookJson>::from_request(request, &()).await {
            Ok(axum::Json(body)) => {
                let seeded = body.selection_seeded;
                let delivery_id = body.delivery_id;
                (
                    HookEnvelope {
                        agent,
                        pane_key: body.pane_key,
                        tab_id: body.tab_id,
                        launch_token: body.launch_token,
                        worktree_id: body.worktree_id,
                        env: body.env,
                        version: body.version,
                        hook_event_name: body.hook_event_name,
                        // Back to text, verbatim. A string payload stays the string it
                        // was rather than becoming a quoted one — the readers that go
                        // looking for an event name inside it would find nothing in
                        // `"\"…\""`.
                        payload: match body.payload {
                            serde_json::Value::String(text) => text,
                            other => other.to_string(),
                        },
                    },
                    seeded,
                    delivery_id,
                    HookTried::default(),
                )
            }
            Err(rejection) => return rejection.into_response(),
        }
    } else {
        match Form::<HookForm>::from_request(request, &()).await {
            Ok(Form(form)) => {
                let seeded = matches!(form.selection_seeded.trim(), "1" | "true");
                let delivery_id = form.delivery_id;
                let tried = HookTried {
                    attempt: form.attempt,
                    gave_up: form.gave_up,
                };
                (
                    HookEnvelope {
                        agent,
                        pane_key: form.pane_key,
                        tab_id: form.tab_id,
                        launch_token: form.launch_token,
                        worktree_id: form.worktree_id,
                        env: form.env,
                        version: form.version,
                        hook_event_name: form.hook_event_name,
                        payload: form.payload,
                    },
                    seeded,
                    delivery_id,
                    tried,
                )
            }
            Err(rejection) => return rejection.into_response(),
        }
    };

    /* A post tried again is answered once (t-78634). The script gave up on
     * an earlier try whose body may be here already, and posts again under
     * the same delivery id: that post is given the first one's answer, and
     * the window is handed the event once. */
    let key = brief_receipts::Key {
        agent,
        pane: envelope.pane_key.clone(),
        launch: envelope.launch_token.clone(),
        id: delivery_id.clone(),
    };
    let claim = key.valid().then(|| {
        state
            .replays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .claim(key, std::time::Instant::now())
    });
    if claim.is_some() {
        note_tried(
            &state,
            &envelope,
            &tried,
            matches!(claim, Some(replays::Claim::Again(_))),
        );
    }
    let first = match claim {
        Some(replays::Claim::Again(answer)) => return replays::replay(answer).await,
        Some(replays::Claim::First(first)) => Some(first),
        None => None,
    };
    /* The work runs on its own task: a client that stops waiting drops this
     * handler, never the event on its way to the window or the answer a post
     * again is owed. */
    let work = tokio::spawn(async move {
        let answer = answer_hook(state, envelope, selection_seeded, delivery_id).await;
        if let Some(first) = first {
            first.send_replace(Some(answer.clone()));
        }
        answer
    });
    work.await.map_or_else(
        |_| StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        IntoResponse::into_response,
    )
}

/// What one hook post says of its own delivery (t-78634): which try it is,
/// and the curl exits the tries before it gave up with. Only the shell
/// scripts try again; a plugin's JSON post is always a first.
#[derive(Debug, Default)]
struct HookTried {
    attempt: String,
    gave_up: String,
}

/// Write down a post that came on a later try, for the window — the try,
/// the exits before it, and whether the first one had arrived after all.
/// What the script says is held to its shape first: it is local input.
fn note_tried(state: &BridgeState, envelope: &HookEnvelope, tried: &HookTried, again: bool) {
    let Some(note) = state.delivery_notes.as_ref().map(Arc::clone) else {
        return;
    };
    let Ok(attempt) = tried.attempt.parse::<u8>() else {
        return;
    };
    if attempt < 2
        || tried.gave_up.is_empty()
        || tried.gave_up.len() > 32
        || !tried
            .gave_up
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b',')
    {
        return;
    }
    let event: String = zerocode_core::hook::envelope_event_and_session(envelope)
        .0
        .unwrap_or_default()
        .chars()
        .take(64)
        .collect();
    let line = format!(
        "hook {} {} from {} reached the bridge on try {attempt} after curl gave up with {}: {}",
        envelope.agent.slug(),
        event.escape_debug(),
        envelope.pane_key.escape_debug(),
        tried.gave_up,
        if again {
            "a try before it had arrived, answered once"
        } else {
            "the first try to arrive"
        }
    );
    drop(tokio::task::spawn_blocking(move || note(line)));
}

/// The hook's work and its answer: the envelope to the window, then the
/// reply composed from the contract, the vault, the turn's brief and the
/// pointer.
async fn answer_hook(
    state: BridgeState,
    envelope: HookEnvelope,
    selection_seeded: bool,
    delivery_id: String,
) -> replays::HookAnswer {
    // An empty payload is not an event. The POSIX script never posts one
    // (it exits before the post); the Windows batch script cannot look at
    // stdin without an interpreter and pipes it straight into curl, so the
    // same rule stands here — with Antigravity's exception, which fires
    // some events with no body at all and is answered as `{}`.
    let mut envelope = envelope;
    if envelope.payload.trim().is_empty() {
        if envelope.agent == AgentKind::Antigravity {
            envelope.payload = "{}".to_string();
        } else {
            return replays::HookAnswer::Status(StatusCode::ACCEPTED);
        }
    }
    // Decode the vendor payload before taking the shared delivery lock. A hook
    // may carry up to 1 MiB; unrelated agent inputs must not queue behind its
    // JSON parser when the critical section only owns a 256-key LRU update.
    let context_event = orchestration_contract::context_event(&envelope, selection_seeded);
    // The prompt, for the second brain and the turn's brief — read only when
    // one of them is installed and only on a provider's prompt event, before
    // the delivery lock as well.
    let submission = (state.knowledge.is_some() || state.brief.is_some())
        .then(|| orchestration_contract::prompt_submission(&envelope))
        .flatten();
    let receipt_key = brief_receipts::Key {
        agent: envelope.agent,
        pane: envelope.pane_key.clone(),
        launch: envelope.launch_token.clone(),
        id: delivery_id,
    };
    let supports_receipts = state.brief.is_some()
        && submission.is_some()
        && state
            .brief_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reserve(receipt_key.clone(), std::time::Instant::now());
    let brief_ask = state
        .brief
        .as_ref()
        .zip(submission.as_ref())
        .map(|(_, submission)| TurnBriefAsk {
            agent: envelope.agent,
            pane_key: envelope.pane_key.clone(),
            launch_token: envelope.launch_token.clone(),
            session_id: submission.session_id.clone(),
            supports_receipts,
            worktree: envelope.worktree_id.clone(),
            prompt: submission.prompt.clone(),
            wall: TURN_BRIEF_WALL,
        });

    /* The pointer, decided BEFORE the envelope goes.
     *
     * Every other reply on this road is composed after the send, so nothing
     * is spent on a payload that found no consumer. This one is the
     * exception, and the ordering is the whole point: a `Stop` that is about
     * to be CONTINUED is not a turn that ended, and the window must know that
     * before it reads the same event and rings a completion. Taken first, the
     * mark is standing when the envelope arrives; taken after, the window
     * would announce the end of a turn that is still going.
     *
     * Nothing is lost by the swap. The mailbox lives in the window's own
     * process, so a send that fails is a window that is gone — and a shelf
     * that went with it. */
    let knock = PointerKnock {
        agent: envelope.agent,
        event: zerocode_core::hook::envelope_event_and_session(&envelope)
            .0
            .unwrap_or_default(),
        payload: envelope.payload.clone(),
        launch_token: envelope.launch_token.clone(),
    };
    let pane_key = envelope.pane_key.clone();
    let pointer = pointer_reply(&state, &pane_key, &knock);
    // A dropped receiver means the server outlived the window half of the
    // bridge. A 202 here would tell the script its payload landed when nobody
    // can consume it; 503 lets the script leave its silent marker instead.
    // The script still exits zero, so this never becomes the agent's failure.
    if state.events.send(envelope).is_err() {
        /* The window half of this bridge is gone, so nothing will consume
         * that payload — and the pointer taken a moment ago is a pointer this
         * reply is not going to deliver. Handed back before the 503, because
         * a taken pointer nobody was ever offered is mail that goes quiet. */
        if let Some(mailbox) = state.pointers.as_ref() {
            mailbox.restore(&pane_key);
        }
        return replays::HookAnswer::Said(StatusCode::SERVICE_UNAVAILABLE, "window is gone");
    }
    // Context is spent only after the payload has a consumer. Advancing the
    // per-session tracker before the send would make a refused delivery steal
    // the one injection a later healthy bridge is meant to return.
    let additional_context = context_event.and_then(|event| tracked_context(&state, event));
    // The vault's answer is a scan of files on a blocking thread: the bridge
    // serves every agent's hooks from this runtime, and a warm scan is
    // milliseconds, a cold one of a large vault is not.
    let event_name = submission
        .as_ref()
        .map(|submission| submission.event_name.clone());
    let knowledge = async {
        match (state.knowledge.as_ref().map(Arc::clone), submission) {
            (Some(source), Some(submission)) => {
                let pane = pane_key.clone();
                tokio::task::spawn_blocking(move || {
                    source.related_block(&submission.session_key, &pane, &submission.prompt)
                })
                .await
                .ok()
                .flatten()
            }
            _ => None,
        }
    };
    // The turn's brief is asked beside the vault's scan, and waited for no
    // longer than its wall: a brief that has not come by then is left out,
    // and the rest of the reply still reaches the script inside its budget.
    let brief = async {
        let (source, ask) = state.brief.as_ref().map(Arc::clone).zip(brief_ask)?;
        let wall = ask.wall;
        let checking = ask.clone();
        let text = tokio::time::timeout(
            wall,
            tokio::task::spawn_blocking(move || source.brief(checking)),
        )
        .await
        .ok()?
        .ok()
        .flatten()
        .filter(|text| !text.trim().is_empty() && text.chars().count() <= TURN_BRIEF_CHAR_CAP)?;
        Some((ask, text))
    };
    let (knowledge, brief) = tokio::join!(knowledge, brief);
    let knowledge = event_name.clone().zip(knowledge);
    let mut wants_receipt = false;
    let brief = brief.and_then(|(ask, text)| {
        if state
            .brief
            .as_ref()
            .is_some_and(|source| source.needs_receipt(&text))
        {
            if !supports_receipts
                || !state
                    .brief_receipts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .ready(&receipt_key, ask, text.clone())
            {
                return None;
            }
            wants_receipt = true;
        }
        event_name.zip(Some(text))
    });
    if !wants_receipt {
        state
            .brief_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unused(&receipt_key);
    }
    /* The pointer's own moment. Decided above, before the envelope went, and
     * answered here ahead of the context reply.
     *
     * A turn that is ENDING is the only moment a running agent can be told
     * something without a keystroke — its own hook is calling, and the
     * measured providers honour a continuation decision in the answer. A
     * session STARTING takes the same pointer as context instead, which is
     * how a fresh, resumed or adopted pane learns about mail that arrived
     * while it was not there.
     *
     * Nothing here acknowledges anything. The pointer is fixed advice to run
     * `zerocode-orc check`, and `check` remains the only delivery. */
    /* A turn-end continuation is its own answer: that reply shape carries no
     * context field, and the event it answers is not one this bridge has
     * context for anyway. */
    if let Some(PointerAnswer::Continuation(decision)) = pointer {
        return replays::HookAnswer::Json(StatusCode::ACCEPTED, decision);
    }
    /* A context pointer is COMPOSED with whatever else this knock was owed,
     * never substituted for it. Returning the pointer alone would drop the
     * orchestration contract and the vault's block — and drop them after the
     * once-per-context tracker had already spent them, so the agent would
     * never be given either. */
    let pointer = match pointer {
        Some(PointerAnswer::Context { event_name, text }) => Some((event_name, text)),
        _ => None,
    };
    match compose_additional_context(additional_context, knowledge, brief, pointer) {
        Some((event_name, context)) => replays::HookAnswer::Json(
            if wants_receipt {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": event_name,
                    "additionalContext": context,
                }
            }),
        ),
        None => replays::HookAnswer::Status(StatusCode::ACCEPTED),
    }
}

/// What one hook knock says about itself, for the pointer road.
///
/// Copied out of the envelope before it is sent onward, because the envelope
/// moves and this decision is made after it has gone.
struct PointerKnock {
    agent: zerocode_core::AgentKind,
    event: String,
    payload: String,
    /// Which launch this knock says it belongs to. Empty from an older
    /// script, which carries none.
    launch_token: String,
}

/// What the mailbox had for this knock, in the shape its moment takes.
enum PointerAnswer {
    /// A turn-end decision. Its own whole reply.
    Continuation(serde_json::Value),
    /// Non-error context, on the event that asked for it. Stands WITH
    /// whatever else that knock was owed, never instead of it, and carries
    /// nothing but words — no decision, no permission, no tool output.
    Context {
        event_name: &'static str,
        text: String,
    },
}

/// The reply that hands this pane its waiting ledger pointer, if this knock is
/// the moment for one.
///
/// Every road out is a refusal by default. A provider that was not MEASURED to
/// continue a turn is never offered a decision it might not understand; a turn
/// that is already a hook's continuation is never blocked again; and a mailbox
/// with nothing in it answers nothing at all.
fn pointer_reply(
    state: &BridgeState,
    pane_key: &str,
    knock: &PointerKnock,
) -> Option<PointerAnswer> {
    use zerocode_core::hook_continuation;

    let mailbox = state.pointers.as_ref()?;
    let event = normalized_hook_event(&knock.event);
    /* The tool boundary FIRST, because it is the one that reaches an agent
     * still working. A turn end alone leaves a long implementation deaf until
     * every last thing it is doing is finished, which is the delay this road
     * exists to remove — measured on this very task, where a coordinator's
     * mail waited out a worker's turn. The reply is context and nothing else:
     * a pointer must never be able to change what a tool did, what a tool
     * answered, or what a person was asked. */
    if let Some(boundary) = hook_continuation::tool_boundary_context(knock.agent)
        && event == normalized_hook_event(boundary.event)
    {
        let notice = mailbox.take(
            pane_key,
            &knock.launch_token,
            pointer_mailbox::PointerMoment::ToolBoundary,
        )?;
        return Some(PointerAnswer::Context {
            event_name: boundary.event,
            text: notice.text().to_string(),
        });
    }
    if let Some(continuation) = hook_continuation::stop_continuation(knock.agent)
        && event == normalized_hook_event(continuation.event)
    {
        if hook_continuation::stop_is_already_a_continuation(&knock.payload) {
            return None;
        }
        let notice = mailbox.take(
            pane_key,
            &knock.launch_token,
            pointer_mailbox::PointerMoment::TurnEnding,
        )?;
        return Some(PointerAnswer::Continuation(serde_json::json!({
            "decision": continuation.decision,
            "reason": notice.text(),
        })));
    }
    let context = knock.agent.hook_additional_context()?;
    if event != normalized_hook_event(context.session_start_event) {
        return None;
    }
    let notice = mailbox.take(
        pane_key,
        &knock.launch_token,
        pointer_mailbox::PointerMoment::SessionStarting,
    )?;
    Some(PointerAnswer::Context {
        event_name: context.session_start_event,
        text: notice.text().to_string(),
    })
}

/// One spelling for an event name, so `Stop`, `stop` and `stop_hook` are the
/// same knock. The same normalisation the context road already uses.
fn normalized_hook_event(event: &str) -> String {
    event
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// One `additionalContext` out of the things a prompt hook may have to say:
/// the orchestration contract (once per model context), the second brain's
/// block (whenever the vault has pages for the prompt) and the turn's brief
/// (whenever a seat acting for the pane has something to say, t-14869). Any
/// alone is the whole answer; together they stand a blank line apart, in
/// that order.
fn compose_additional_context(
    selection: Option<orchestration_contract::ContextReply>,
    knowledge: Option<(String, String)>,
    brief: Option<(String, String)>,
    pointer: Option<(&'static str, String)>,
) -> Option<(String, String)> {
    let mut event_name = selection.as_ref().map(|reply| reply.event_name.clone());
    let mut blocks: Vec<String> = selection
        .map(|reply| reply.context.to_string())
        .into_iter()
        .collect();
    for (named, block) in [knowledge, brief].into_iter().flatten() {
        event_name.get_or_insert(named);
        blocks.push(block);
    }
    /* The pointer goes LAST and never displaces anything.
     *
     * It is the shortest and the most immediate of the three — one sentence
     * saying there is mail — so it reads best at the end, and putting it
     * there means the rule and the vault's pages keep the positions they
     * have always had.
     *
     * It carries its own event name, and that name WINS when the two differ.
     * A tool boundary is a knock the contract and the vault never answer, so
     * a reply that inherited a prompt event's name there would be addressed
     * to an event that is not happening. */
    let event_name = match (event_name, &pointer) {
        (_, Some((named, _))) => Some((*named).to_string()),
        (named, None) => named,
    };
    blocks.extend(pointer.map(|(_, text)| text));
    match (event_name, blocks.is_empty()) {
        (Some(event_name), false) => Some((event_name, blocks.join("\n\n"))),
        _ => None,
    }
}

fn tracked_context(
    state: &BridgeState,
    event: orchestration_contract::ContextEvent,
) -> Option<orchestration_contract::ContextReply> {
    let mut tracker = state
        .selection_context
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tracker.context_for(event)
}

/// Compare without an early return on the first differing byte. The token is a
/// per-run secret on loopback, so this is belt-and-braces rather than critical —
/// but it costs nothing.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// Bind the bridge on loopback and serve it until the returned task is dropped.
/// Pass port `0` to let the OS choose; the bound address comes back so the
/// caller can put the real port into the agent environment.
pub async fn serve(
    state: BridgeState,
    port: u16,
) -> io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener =
        TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)).await?;
    let addr = listener.local_addr()?;
    let app = router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((addr, handle))
}

/// Write one hook script per agent into `dir` and return the paths.
///
/// The script's contract, in order of importance:
///
/// 1. **It always exits 0.** A hook that fails a build, or blocks for seconds
///    because the bridge is gone, turns a ZeroCode bug into an agent bug. Every
///    failure path here is silent and successful.
/// 2. **It is bounded in time** — half a second to connect, a second and a half
///    total. An agent must never wait on our UI.
/// 3. **It does nothing without configuration.** No port, token, or pane key in
///    the environment means the agent is running outside ZeroCode; exit quietly.
pub fn install_hook_scripts(dir: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::with_capacity(ALL_AGENTS.len());
    for agent in ALL_AGENTS {
        let path = dir.join(format!("{}-hook.sh", agent.slug()));
        std::fs::write(&path, hook_script(agent))?;
        set_executable(&path)?;
        written.push(path);
        // The Windows dialect beside it, on every platform: a synced config
        // may name it, and the clean rule already knows the extension.
        let batch = dir.join(format!("{}-hook.cmd", agent.slug()));
        std::fs::write(&batch, hook_script_cmd(agent))?;
    }
    Ok(written)
}

fn set_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The script body for one agent: the shared skeleton with the agent's own
/// obligations in it.
///
/// The skeleton, in the order the lines run and why each one is there
/// (measured off Orca 1.4.169's generators, managed-agent-hook-controls
/// :655-680 and :1773-1813):
///
/// 1. **Answer in the provider's measured shape.** Copilot can answer `{}` at
///    once. Claude and Codex can accept context returned by the bridge, so
///    their scripts print that response.
///    Antigravity's answer depends on which event this is, which is why its
///    event name arrives in an environment variable rather than only as a form
///    field.
/// 2. **Read stdin through `command -p cat`.** The agent's PATH is the user's
///    PATH, and a shell function or alias named `cat` would otherwise sit in
///    the middle of every hook. `-p` asks for the system default.
/// 3. **Source the endpoint file.** The PTY env was copied at launch; the file
///    says where the bridge is NOW. Sourcing after the read and before the
///    guard means a bridge that restarted still gets the payload.
/// 4. **Exit quietly without configuration.** No port, token or pane key means
///    the agent is running outside this window; a hook that complains — or
///    waits — turns our absence into the agent's problem.
/// 5. **Bounded curl, every failure swallowed.** Half a second to connect, a
///    second and a half a try, tried again on a give-up that may have left the
///    body with the bridge ([`hook_tries`], [`HOOK_RETRY_EXITS`]) under one
///    delivery id, five seconds at most in all, `exit 0` — the POSIX script;
///    the batch one posts once, since it cannot hold stdin for a second post. Providers that accept context receive
///    only the bridge's valid JSON body; every other response is discarded.
///    A POST that provably did not arrive creates the path the window handed
///    this pane, and a successful one removes it — a give-up that leaves the
///    body on the wire (`delivery_gave_up`) marks nothing, because the window
///    may already hold it. Marker operations are swallowed too: the agent
///    must never be blocked or failed by the IDE watching it.
///
/// Claude's script carries two extra guards. Devin imports Claude's hook
/// configuration wholesale, so inside a Devin run (`DEVIN_PROJECT_DIR` set)
/// the Claude script would double-report every event through the wrong slug —
/// it steps aside and lets the Devin hook speak. And a `--bg`/`/background`
/// worker runs under Claude's shared daemon, inheriting the env of whichever
/// pane started that daemon — its pane key names a pane the session does not
/// run in, so posting would stamp a live pane with a stranger's state
/// (upstream measured the same defect: orca #9236/#15304, keyed on
/// `CLAUDE_JOB_DIR`, which is set only inside those workers).
/// How one vendor's hook answers and what it must guard — the facts both
/// script dialects read, in one place, so the Windows script cannot drift
/// from the POSIX one the way an upstream vendor copy did for three months
/// (docs/plans/windows-hook-scripts.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookScriptSpec {
    /// What to print before anything else — the provider's measured shape.
    pub reply: HookReply,
    /// The event name arrives in this environment variable (the installed
    /// command sets it), and is posted as `hook_event_name`.
    pub event_var: &'static str,
    /// An empty payload is still an event (Antigravity fires some with no
    /// body) rather than an accident to step over.
    pub empty_payload_is_event: bool,
    /// The Devin-import and background-worker guards (Claude only).
    pub claude_guards: bool,
    /// The bridge's JSON reply is printed back to the agent.
    pub prints_context: bool,
}

/// What a hook prints before it posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookReply {
    Nothing,
    /// `{}` — Copilot waits for an answer and accepts an empty object.
    EmptyObject,
    /// `{"decision":""}` on `Stop`, `{}` otherwise — Antigravity, measured.
    AntigravityByEvent,
}

/// The table.
#[must_use]
pub const fn hook_script_spec(agent: AgentKind) -> HookScriptSpec {
    let (reply, event_var, empty_payload_is_event) = match agent {
        AgentKind::Copilot => (HookReply::EmptyObject, env_var::COPILOT_EVENT, false),
        AgentKind::Antigravity => (
            HookReply::AntigravityByEvent,
            env_var::ANTIGRAVITY_EVENT,
            true,
        ),
        _ => (HookReply::Nothing, env_var::EVENT, false),
    };
    HookScriptSpec {
        reply,
        event_var,
        empty_payload_is_event,
        claude_guards: matches!(agent, AgentKind::Claude),
        prints_context: agent.hook_additional_context().is_some(),
    }
}

/// The same script for a Windows host, as a batch file: no interpreter
/// starts for a hook — `curl.exe` (in every Windows since 10 1803) posts
/// the payload straight from stdin, and everything else is `cmd.exe`
/// builtins. Reads [`hook_script_spec`] like [`hook_script`] does, edge for
/// edge: the reply shape first, the guards before the post, the endpoint
/// file's values over the environment's, a quiet exit without
/// configuration, curl bounded and every failure swallowed, the delivery
/// marker claimed only by a give-up that proves the window never got the
/// body (7, 22), and exit 0 on every path.
///
/// Two things this dialect cannot do and the bridge does for it: it cannot
/// look at stdin before posting, so an empty payload reaches the bridge
/// and is dropped there (or, for Antigravity, read as `{}`); and it cannot
/// choose to print only a reply that starts with `{` — curl prints the
/// body of a 2xx, which is the bridge's JSON or nothing.
#[must_use]
pub fn hook_script_cmd(agent: AgentKind) -> String {
    let spec = hook_script_spec(agent);
    let slug = agent.slug();
    let (port, token, pane_key) = (env_var::PORT, env_var::TOKEN, env_var::PANE_KEY);
    let (tab_id, launch_token) = (env_var::TAB_ID, env_var::LAUNCH_TOKEN);
    let (worktree_id, agent_env) = (env_var::WORKTREE_ID, env_var::AGENT_ENV);
    let (endpoint, marker) = (env_var::ENDPOINT, env_var::DELIVERY_FAILURE_MARKER);
    let (seeded, event_var) = (env_var::SELECTION_SEEDED, spec.event_var);
    let mut lines: Vec<String> = vec![
        "@echo off".into(),
        "rem Installed by ZeroCode. Forwards this agent's hook payload to the".into(),
        "rem local ZeroCode bridge with curl.exe; no interpreter starts for a".into(),
        "rem hook. Every failure path exits 0 on purpose: the agent must never".into(),
        "rem be blocked or failed by the IDE watching it.".into(),
        "setlocal EnableExtensions".into(),
        format!("set \"delivery_marker=%{marker}%\""),
    ];
    match spec.reply {
        HookReply::Nothing => {}
        HookReply::EmptyObject => lines.push("echo {}".into()),
        HookReply::AntigravityByEvent => {
            lines.push(format!(
                "if \"%{event_var}%\"==\"Stop\" (echo {{\"decision\":\"\"}}) else (echo {{}})"
            ));
        }
    }
    if spec.claude_guards {
        lines.push("if defined DEVIN_PROJECT_DIR exit /b 0".into());
        lines.push("if defined CLAUDE_JOB_DIR exit /b 0".into());
    }
    // The endpoint file's values override the PTY's: the file is where the
    // bridge IS, the env is where it WAS. Its values are shell-safe by
    // construction (`endpoint::is_shell_safe`), so `KEY=value` lines read
    // straight into variables.
    lines.push(format!(
        "if defined {endpoint} if exist \"%{endpoint}%\" for /f \"usebackq tokens=1,* delims==\" %%K in (\"%{endpoint}%\") do set \"%%K=%%L\""
    ));
    lines.push(format!("if not defined {port} goto :unconfigured"));
    lines.push(format!("if not defined {token} goto :unconfigured"));
    lines.push(format!("if not defined {pane_key} goto :unconfigured"));
    // One post, never a second (t-78634): cmd cannot hold stdin in a
    // variable the way the POSIX script holds `$payload`, so curl reads it
    // straight from the pipe and a post again would send an empty body.
    let quiet = if spec.prints_context {
        ""
    } else {
        " >nul 2>nul"
    };
    let post = format!(
        "curl.exe -fsS -X POST \"http://127.0.0.1:%{port}%/hook/{slug}\" --connect-timeout {connect} --max-time {budget} -H \"Content-Type: application/x-www-form-urlencoded\" -H \"{HOOK_TOKEN_HEADER}: %{token}%\" --data-urlencode \"pane_key=%{pane_key}%\" --data-urlencode \"tab_id=%{tab_id}%\" --data-urlencode \"launch_token=%{launch_token}%\" --data-urlencode \"worktree_id=%{worktree_id}%\" --data-urlencode \"selection_seeded=%{seeded}%\" --data-urlencode \"hook_event_name=%{event_var}%\" --data-urlencode \"env=%{agent_env}%\" --data-urlencode \"version={HOOK_CONTRACT_VERSION}\" --data-urlencode \"payload@-\"{quiet}",
        connect = HOOK_CONNECT_BUDGET.as_secs_f64(),
        budget = HOOK_REPLY_BUDGET.as_secs_f64(),
    );
    if spec.prints_context {
        // Only cmd builtins and the existing curl are needed. Numeric status
        // comes from curl's formatter, never from untrusted response text.
        lines.extend([
            "set \"brief_attempt=0\"".into(),
            ":brief_nonce".into(),
            "set /a brief_attempt+=1 >nul 2>nul".into(),
            "set \"brief_clock=%TIME: =0%\"".into(),
            "set \"brief_clock=%brief_clock::=%\"".into(),
            "set \"brief_clock=%brief_clock:.=%\"".into(),
            "set \"brief_clock=%brief_clock:,=%\"".into(),
            "set \"brief_id=w-%brief_clock%-%RANDOM%-%RANDOM%-%RANDOM%\"".into(),
            "set \"brief_dir=%TEMP%\\zerocode-hook-%brief_id%\"".into(),
            "mkdir \"%brief_dir%\" >nul 2>nul".into(),
            "if not errorlevel 1 goto :brief_post".into(),
            "if %brief_attempt% LSS 3 goto :brief_nonce".into(),
            "goto :plain_post".into(),
            ":brief_post".into(),
            format!("{post} --data-urlencode \"delivery_id=%brief_id%\" -o \"%brief_dir%\\reply\" -w \"%%{{http_code}}\" >\"%brief_dir%\\status\" 2>nul"),
            "set \"curl_exit=%ERRORLEVEL%\"".into(),
            "if not \"%curl_exit%\"==\"0\" (call :gave_up %curl_exit% & goto :brief_cleanup)".into(),
            "call :succeeded".into(),
            "type \"%brief_dir%\\reply\" 2>nul".into(),
            "if errorlevel 1 goto :brief_cleanup".into(),
            "set \"brief_status=\"".into(),
            "set /p brief_status=<\"%brief_dir%\\status\"".into(),
            format!("if \"%brief_status%\"==\"200\" curl.exe -fsS -X POST \"http://127.0.0.1:%{port}%/hook-receipt/{slug}\" --connect-timeout 0.1 --max-time 0.25 -H \"{HOOK_TOKEN_HEADER}: %{token}%\" --data-urlencode \"pane_key=%{pane_key}%\" --data-urlencode \"launch_token=%{launch_token}%\" --data-urlencode \"delivery_id=%brief_id%\" >nul 2>nul"),
            ":brief_cleanup".into(),
            "del /q \"%brief_dir%\\reply\" \"%brief_dir%\\status\" >nul 2>nul".into(),
            "rmdir \"%brief_dir%\" >nul 2>nul".into(),
            "exit /b 0".into(),
            ":plain_post".into(),
        ]);
    }
    lines.push(post);
    lines.push("set \"curl_exit=%ERRORLEVEL%\"".into());
    lines.push(
        "if \"%curl_exit%\"==\"0\" (call :succeeded) else (call :gave_up %curl_exit%)".into(),
    );
    lines.push("exit /b 0".into());
    lines.push(":unconfigured".into());
    lines.push("call :failed".into());
    lines.push("exit /b 0".into());
    // The marker is a directory, made with its parents in one builtin.
    lines.push(":failed".into());
    lines.push("if defined delivery_marker mkdir \"%delivery_marker%\" >nul 2>nul".into());
    lines.push("exit /b 0".into());
    lines.push(":succeeded".into());
    lines.push("if defined delivery_marker rmdir \"%delivery_marker%\" >nul 2>nul".into());
    lines.push("exit /b 0".into());
    // How curl gave up: 28 and 52/56 leave the body on the wire where the
    // window may already have read it, so they mark nothing (the POSIX
    // script's rule, word for word).
    lines.push(":gave_up".into());
    for exit in HOOK_RETRY_EXITS {
        lines.push(format!("if \"%~1\"==\"{exit}\" exit /b 0"));
    }
    lines.push("call :failed".into());
    lines.push("exit /b 0".into());
    lines.join("\r\n") + "\r\n"
}

pub fn hook_script(agent: AgentKind) -> String {
    let spec = hook_script_spec(agent);
    let mut lines: Vec<String> = vec!["#!/bin/sh".into()];
    lines.push("# Installed by ZeroCode. Forwards this agent's hook payload to the".into());
    lines.push("# local ZeroCode bridge. Every failure path exits 0 on purpose: the".into());
    lines.push("# agent must never be blocked or failed by the IDE watching it.".into());
    let failure_marker = env_var::DELIVERY_FAILURE_MARKER;
    lines.push(format!(r#"delivery_marker=${{{failure_marker}:-}}"#));
    lines.push("delivery_failed() {".into());
    lines.push(r#"  if [ -n "$delivery_marker" ]; then"#.into());
    lines.push(r#"    delivery_dir=${delivery_marker%/*}"#.into());
    lines.push(
        r#"    (umask 077; command -p mkdir -p "$delivery_dir" 2>/dev/null && command -p mkdir "$delivery_marker" 2>/dev/null) || :"#
            .into(),
    );
    lines.push("  fi".into());
    lines.push("}".into());
    lines.push("delivery_succeeded() {".into());
    lines.push(r#"  if [ -n "$delivery_marker" ]; then"#.into());
    lines.push(r#"    command -p rmdir "$delivery_marker" 2>/dev/null || :"#.into());
    lines.push("  fi".into());
    lines.push("}".into());
    // How curl gave up, not merely that it did. The marker claims the window
    // never got this payload, and only some of curl's failures say that: 28
    // (it stopped waiting) and 52/56 (the answer was lost) all leave the
    // request body on the wire, where the window may already have read,
    // parsed and enveloped it. Marking there brands a delivery that landed,
    // and nothing takes it back until this pane's NEXT successful POST — the
    // rest of the session, for an agent whose last turn ended that way.
    //
    // Nothing true is given up. A bridge that is not there (7) and a window
    // that refuses the payload (22, which its 503 produces) still mark, and a
    // window that accepts the connection and never answers is still reported
    // by the readiness deadline, which is `unreachable`'s other half.
    lines.push("delivery_gave_up() {".into());
    lines.push(r#"  case "$1" in"#.into());
    lines.push(format!("    {}) : ;;", posix_retry_exits()));
    lines.push("    *) delivery_failed ;;".into());
    lines.push("  esac".into());
    lines.push("}".into());
    let mut extra_fields: Vec<(&str, String)> = Vec::new();
    let context_hook = agent.hook_additional_context();

    match spec.reply {
        HookReply::Nothing => {}
        // Copilot waits for the hook's answer but has no measured context
        // response adapter. An empty object means "no objection, carry on".
        HookReply::EmptyObject => lines.push(r#"printf '{}\n'"#.into()),
        // Antigravity's Stop hook is asked for a decision and the rest for an
        // acknowledgement — measured verbatim, including the empty decision.
        HookReply::AntigravityByEvent => {
            let event = spec.event_var;
            lines.push(format!(r#"case "${{{event}}}" in"#));
            lines.push("  Stop)".into());
            lines.push(r#"    printf '{"decision":""}\n'"#.into());
            lines.push("    ;;".into());
            lines.push("  *)".into());
            lines.push(r#"    printf '{}\n'"#.into());
            lines.push("    ;;".into());
            lines.push("esac".into());
        }
    }

    // The payload, via the system cat even on a decorated PATH.
    lines.push("payload=$({ command -p cat 2>/dev/null || cat; })".into());
    lines.push(r#"if [ -z "$payload" ]; then"#.into());
    if spec.empty_payload_is_event {
        // Antigravity fires some events with no body at all; an empty payload
        // is an event, not an accident.
        lines.push("  payload='{}'".into());
    } else {
        lines.push("  exit 0".into());
    }
    lines.push("fi".into());

    if spec.claude_guards {
        lines.push(r#"if [ -n "$DEVIN_PROJECT_DIR" ]; then"#.into());
        lines.push("  exit 0".into());
        lines.push("fi".into());
        lines.push(r#"if [ -n "$CLAUDE_JOB_DIR" ]; then"#.into());
        lines.push("  exit 0".into());
        lines.push("fi".into());
    }

    // The endpoint file's values override the PTY's: the file is where the
    // bridge IS, the env is where it WAS.
    let endpoint = env_var::ENDPOINT;
    lines.push(format!(
        r#"if [ -n "${{{endpoint}}}" ] && [ -r "${{{endpoint}}}" ]; then"#
    ));
    lines.push(format!(r#"  . "${{{endpoint}}}" 2>/dev/null || :"#));
    lines.push("fi".into());

    let (port, token, pane_key) = (env_var::PORT, env_var::TOKEN, env_var::PANE_KEY);
    lines.push(format!(
        r#"if [ -z "${{{port}}}" ] || [ -z "${{{token}}}" ] || [ -z "${{{pane_key}}}" ]; then"#
    ));
    lines.push("  delivery_failed".into());
    lines.push("  exit 0".into());
    lines.push("fi".into());

    extra_fields.push(("hook_event_name", format!("${{{}}}", spec.event_var)));

    let slug = agent.slug();
    // The posts, tried again on a give-up that may have left the body with
    // the bridge (t-78634). Every try carries the same delivery id, so the
    // bridge answers a post again with the first one's answer and hands the
    // window the event once; the try's number and the exits before it ride
    // along, so the window can write down a delivery that needed them.
    let plan = hook_tries(agent)
        .iter()
        .map(|one| format!("{}:{}", one.pause.as_secs_f64(), one.budget.as_secs_f64()))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push("hook_id=p-$$".into());
    if context_hook.is_some() {
        lines.push("brief_nl='\n'".into());
        lines.push("reply=".into());
    }
    lines.push("hook_try=0".into());
    lines.push("hook_gave_up=".into());
    lines.push("hook_exit=".into());
    lines.push(format!("for hook_plan in {plan}; do"));
    lines.push("  hook_pause=${hook_plan%%:*}".into());
    lines.push("  hook_budget=${hook_plan#*:}".into());
    lines.push(r#"  if [ "$hook_try" -gt 0 ]; then"#.into());
    lines.push(r#"    command -p sleep "$hook_pause" 2>/dev/null || :"#.into());
    lines.push("  fi".into());
    lines.push("  hook_try=$((hook_try + 1))".into());
    if context_hook.is_some() {
        lines.push("  if reply=$(".into());
    } else {
        lines.push("  if".into());
    }
    lines.push(format!(
        r#"  printf '%s' "$payload" | curl -fsS -X POST "http://127.0.0.1:${{{port}}}/hook/{slug}" \"#
    ));
    lines.push(format!(
        r#"    --connect-timeout {} --max-time "$hook_budget" \"#,
        HOOK_CONNECT_BUDGET.as_secs_f64(),
    ));
    lines.push(r#"    -H "Content-Type: application/x-www-form-urlencoded" \"#.into());
    lines.push(format!(r#"    -H "{HOOK_TOKEN_HEADER}: ${{{token}}}" \"#));
    let (tab_id, launch_token) = (env_var::TAB_ID, env_var::LAUNCH_TOKEN);
    let (worktree_id, agent_env) = (env_var::WORKTREE_ID, env_var::AGENT_ENV);
    lines.push(format!(
        r#"    --data-urlencode "pane_key=${{{pane_key}}}" \"#
    ));
    lines.push(format!(r#"    --data-urlencode "tab_id=${{{tab_id}}}" \"#));
    lines.push(format!(
        r#"    --data-urlencode "launch_token=${{{launch_token}}}" \"#
    ));
    lines.push(format!(
        r#"    --data-urlencode "worktree_id=${{{worktree_id}}}" \"#
    ));
    lines.push(format!(
        r#"    --data-urlencode "selection_seeded=${{{}}}" \"#,
        env_var::SELECTION_SEEDED
    ));
    for (field, value) in extra_fields {
        lines.push(format!(r#"    --data-urlencode "{field}={value}" \"#));
    }
    lines.push(format!(r#"    --data-urlencode "env=${{{agent_env}}}" \"#));
    lines.push(format!(
        r#"    --data-urlencode "version={HOOK_CONTRACT_VERSION}" \"#
    ));
    lines.push(r#"    --data-urlencode "delivery_id=$hook_id" \"#.into());
    lines.push(r#"    --data-urlencode "try=$hook_try" \"#.into());
    lines.push(r#"    --data-urlencode "gave_up=$hook_gave_up" \"#.into());
    if context_hook.is_some() {
        lines.push(r#"    -w '\n%{http_code}' --data-urlencode "payload@-" 2>/dev/null"#.into());
        lines.push("  )".into());
    } else {
        lines.push(r#"    --data-urlencode "payload@-" >/dev/null 2>&1"#.into());
    }
    lines.push("  then".into());
    lines.push("    hook_exit=0".into());
    lines.push("    break".into());
    lines.push("  else".into());
    // Before anything else, which is a command and answers 0.
    lines.push("    hook_exit=$?".into());
    lines.push("  fi".into());
    if context_hook.is_some() {
        lines.push("  reply=".into());
    }
    lines.push("  hook_gave_up=${hook_gave_up:+$hook_gave_up,}$hook_exit".into());
    lines.push(r#"  case "$hook_exit" in"#.into());
    lines.push(format!("    {}) : ;;", posix_retry_exits()));
    lines.push("    *) break ;;".into());
    lines.push("  esac".into());
    lines.push("done".into());
    // Judged by the FIRST give-up: the loop only goes on after one that may
    // have left the body with the bridge, so a refused connection on a later
    // try (a bridge gone meanwhile) does not prove the window never got it.
    lines.push(r#"if [ "$hook_exit" = 0 ]; then"#.into());
    lines.push("  delivery_succeeded".into());
    lines.push("else".into());
    lines.push(r#"  delivery_gave_up "${hook_gave_up%%,*}""#.into());
    lines.push("fi".into());
    if context_hook.is_some() {
        lines.push(r#"brief_status=${reply##*"$brief_nl"}"#.into());
        lines.push(r#"reply=${reply%"$brief_nl"*}"#.into());
        lines.push(r#"case "$reply" in"#.into());
        lines.push(r#"  \{*\}) trap '' PIPE"#.into());
        lines.push(
            r#"  if printf '%s\n' "$reply" 2>/dev/null && [ "$brief_status" = 200 ]; then"#.into(),
        );
        lines.push(format!(r#"    curl -fsS -X POST "http://127.0.0.1:${{{port}}}/hook-receipt/{slug}" --connect-timeout 0.1 --max-time 0.25 -H "{HOOK_TOKEN_HEADER}: ${{{token}}}" --data-urlencode "pane_key=${{{pane_key}}}" --data-urlencode "launch_token=${{{launch_token}}}" --data-urlencode "delivery_id=$hook_id" >/dev/null 2>&1 || :"#));
        lines.push("  fi ;;".into());
        lines.push("esac".into());
    }
    lines.push("exit 0".into());
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_original_workspace_revisions_never_reach_the_browser_queue() {
        for raw in [
            "",
            "-1",
            "+1",
            "1.5",
            " 1",
            "1,2",
            "18446744073709551616",
            "x",
        ] {
            let (state, _events, _teams, mut browser) = BridgeState::new("hook", "browser");
            let request = Request::builder()
                .header(BROWSER_TOKEN_HEADER, "browser")
                .header("x-zerocode-workspace-revision", raw)
                .body(axum::body::Body::from(
                    "click\u{1f}browser-1\u{1f}#submit\u{1f}",
                ))
                .unwrap();
            let mut pending = tokio::spawn(receive_browser_command(State(state), request));
            let response = tokio::select! {
                response = &mut pending => response.unwrap(),
                queued = browser.recv() => {
                    assert!(queued.is_none(), "malformed revision {raw:?} was dispatched");
                    pending.await.unwrap()
                },
            };
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert!(browser.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn duplicate_original_workspace_revision_headers_never_reach_the_browser_queue() {
        let (state, _events, _teams, mut browser) = BridgeState::new("hook", "browser");
        let request = Request::builder()
            .header(BROWSER_TOKEN_HEADER, "browser")
            .header("x-zerocode-workspace-revision", "7")
            .header("x-zerocode-workspace-revision", "7")
            .body(axum::body::Body::from(
                "click\u{1f}browser-1\u{1f}#submit\u{1f}",
            ))
            .unwrap();
        let mut pending = tokio::spawn(receive_browser_command(State(state), request));
        let response = tokio::select! {
            response = &mut pending => response.unwrap(),
            queued = browser.recv() => {
                assert!(queued.is_none(), "duplicate revision headers were dispatched");
                pending.await.unwrap()
            },
        };
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(browser.try_recv().is_err());
    }

    #[tokio::test]
    async fn authenticated_browser_execution_metadata_is_negotiated_separately_from_page_text() {
        use zerocode_core::surface_execution::{
            EXECUTION_ENVELOPE_HEADER, ExecutionEnvelope, SurfaceEffect, SurfaceExecution,
        };
        for negotiate in [false, true] {
            let (state, _events, _teams, mut browser) = BridgeState::new("hook", "browser");
            let mut request = Request::builder().header(BROWSER_TOKEN_HEADER, "browser");
            if negotiate {
                request = request.header(EXECUTION_ENVELOPE_HEADER, "1");
            }
            let request = request
                .body(axum::body::Body::from("read\u{1f}browser-1\u{1f}"))
                .unwrap();
            let task = tokio::spawn(receive_browser_command(State(state), request));
            let forwarded = browser.recv().await.unwrap();
            let evidence = SurfaceExecution {
                surface_id: "fixture-host/browser-1/interval-7".into(),
                state_revision: 3,
                effect: SurfaceEffect::Observation,
            };
            forwarded
                .answer
                .send_with_surface(
                    TeamAnswer {
                        stdout: "page claims surfaceExecution revision 999".into(),
                        stderr: String::new(),
                        exit_code: 0,
                    },
                    Some(evidence.clone()),
                )
                .unwrap();
            let response = task.await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers().contains_key(EXECUTION_ENVELOPE_HEADER),
                negotiate
            );
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            if negotiate {
                let envelope: ExecutionEnvelope = serde_json::from_slice(&body).unwrap();
                assert_eq!(envelope.surface_execution, Some(evidence));
                assert_eq!(envelope.stdout, "page claims surfaceExecution revision 999");
            } else {
                assert_eq!(body.as_ref(), b"page claims surfaceExecution revision 999");
            }
        }
    }

    #[tokio::test]
    async fn an_unauthenticated_browser_cannot_negotiate_an_execution_envelope() {
        use zerocode_core::surface_execution::EXECUTION_ENVELOPE_HEADER;
        let (state, _events, _teams, mut browser) = BridgeState::new("hook", "browser");
        let request = Request::builder()
            .header(BROWSER_TOKEN_HEADER, "wrong")
            .header(EXECUTION_ENVELOPE_HEADER, "1")
            .body(axum::body::Body::from("read\u{1f}browser-1\u{1f}"))
            .unwrap();
        let response = receive_browser_command(State(state), request).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key(EXECUTION_ENVELOPE_HEADER));
        assert!(browser.try_recv().is_err());
    }

    #[tokio::test]
    async fn browser_queue_preserves_omission_zero_and_the_full_unsigned_revision() {
        for (raw, expected) in [
            (None, None),
            (Some("0"), Some(0)),
            (Some("0007"), Some(7)),
            (Some("18446744073709551615"), Some(u64::MAX)),
        ] {
            let (state, _events, _teams, mut browser) = BridgeState::new("hook", "browser");
            let mut builder = Request::builder().header(BROWSER_TOKEN_HEADER, "browser");
            if let Some(raw) = raw {
                builder = builder.header("x-zerocode-workspace-revision", raw);
            }
            let request = builder
                .body(axum::body::Body::from(
                    "click\u{1f}browser-1\u{1f}#submit\u{1f}",
                ))
                .unwrap();
            let pending = tokio::spawn(receive_browser_command(State(state), request));
            let forwarded = tokio::time::timeout(std::time::Duration::from_secs(2), browser.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(forwarded.workspace_revision, expected);
            assert_eq!(forwarded.argv, ["click", "browser-1", "#submit"]);
            let _ = forwarded.answer.send(TeamAnswer {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            });
            assert_eq!(pending.await.unwrap().status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn a_revision_in_both_header_and_argv_is_refused_before_queueing() {
        let (state, _events, _teams, mut browser) = BridgeState::new("hook", "browser");
        let request = Request::builder()
            .header(BROWSER_TOKEN_HEADER, "browser")
            .header("x-zerocode-workspace-revision", "7")
            .body(axum::body::Body::from(
                "click\u{1f}browser-1\u{1f}#submit\u{1f}--workspace-revision\u{1f}7\u{1f}",
            ))
            .unwrap();
        let mut pending = tokio::spawn(receive_browser_command(State(state), request));
        let response = tokio::select! {
            response = &mut pending => response.unwrap(),
            queued = browser.recv() => {
                assert!(queued.is_none(), "two revision sources were dispatched");
                pending.await.unwrap()
            },
        };
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(browser.try_recv().is_err());
    }

    /// Every vendor, both dialects, one assertion each — the shape the
    /// upstream fix (#15520) taught: a per-vendor test cannot see a vendor's
    /// copy drift. The Windows script posts with curl.exe and starts no
    /// interpreter; the POSIX script keeps curl; both guard before they
    /// post and exit zero on every path.
    #[test]
    fn every_vendors_windows_hook_posts_with_curl_and_starts_no_interpreter() {
        for agent in ALL_AGENTS {
            let batch = hook_script_cmd(agent);
            let slug = agent.slug();
            assert!(batch.starts_with("@echo off\r\n"), "{slug}: {batch}");
            assert!(
                batch.contains(&format!(
                    "curl.exe -fsS -X POST \"http://127.0.0.1:%ZEROCODE_HOOK_PORT%/hook/{slug}\""
                )),
                "{slug} does not post with curl.exe:\n{batch}"
            );
            let lower = batch.to_ascii_lowercase();
            for interpreter in ["powershell", "pwsh", "wscript", "cscript", "python", "node"] {
                assert!(
                    !lower.contains(interpreter),
                    "{slug} starts {interpreter}:\n{batch}"
                );
            }
            assert!(
                batch.contains("--data-urlencode \"payload@-\""),
                "{slug}: raw stdin is the payload"
            );
            for field in [
                "pane_key=",
                "tab_id=",
                "launch_token=",
                "worktree_id=",
                "selection_seeded=",
                "hook_event_name=",
                "env=",
                "version=",
            ] {
                assert!(
                    batch.contains(field),
                    "{slug} lost the {field} field:\n{batch}"
                );
            }
            assert!(batch.contains("x-zerocode-hook-token: %ZEROCODE_HOOK_TOKEN%"));
            assert!(batch.contains(&format!(
                "--data-urlencode \"version={HOOK_CONTRACT_VERSION}\""
            )));
            // The endpoint file is read before the guard, and the guard before the post.
            let endpoint = batch
                .find("%ZEROCODE_HOOK_ENDPOINT%")
                .expect("endpoint read");
            let guard = batch
                .find("if not defined ZEROCODE_HOOK_PORT")
                .expect("guard");
            let post = batch.find("curl.exe -fsS").expect("post");
            assert!(
                endpoint < guard && guard < post,
                "{slug}: endpoint/guard/post order"
            );
            // Every road ends in exit 0.
            assert!(!batch.contains("exit /b 1"), "{slug} can fail the agent");
            assert!(batch.trim_end().ends_with("exit /b 0"));
            // The POSIX script says the same things in its dialect.
            let posix = hook_script(agent);
            assert!(posix.contains(&format!("/hook/{slug}")) && posix.contains("curl -fsS"));
            assert!(posix.contains("--data-urlencode \"payload@-\""));
        }
    }

    /// The vendor facts land in both dialects: the reply shape, the event
    /// variable, the Claude guards — and only where they belong.
    #[test]
    fn the_vendor_table_reaches_both_dialects_alike() {
        let copilot = hook_script_cmd(AgentKind::Copilot);
        assert!(copilot.starts_with("@echo off\r\nrem"), "{copilot}");
        assert!(
            copilot.contains("\r\necho {}\r\n"),
            "copilot answers {{}} first:\n{copilot}"
        );
        assert!(copilot.contains("hook_event_name=%ZEROCODE_COPILOT_HOOK_EVENT%"));
        let antigravity = hook_script_cmd(AgentKind::Antigravity);
        assert!(
            antigravity.contains("if \"%ZEROCODE_ANTIGRAVITY_EVENT%\"==\"Stop\" (echo {\"decision\":\"\"}) else (echo {})"),
            "{antigravity}"
        );
        assert!(antigravity.contains("hook_event_name=%ZEROCODE_ANTIGRAVITY_EVENT%"));
        let claude = hook_script_cmd(AgentKind::Claude);
        assert!(claude.contains("if defined DEVIN_PROJECT_DIR exit /b 0"));
        let job_guard = claude
            .find("CLAUDE_JOB_DIR")
            .expect("background-worker guard");
        assert!(
            job_guard < claude.find("curl.exe -fsS").unwrap(),
            "the guard stands after the post"
        );
        assert!(
            !claude.contains(">nul 2>nul\r\nset \"curl_exit"),
            "claude prints the bridge's reply"
        );
        assert!(claude.contains("hook_event_name=%ZEROCODE_HOOK_EVENT%"));
        let cursor = hook_script_cmd(AgentKind::Cursor);
        assert!(
            cursor.contains("\"payload@-\" >nul 2>nul"),
            "a vendor without context prints nothing:\n{cursor}"
        );
        assert!(!cursor.contains("DEVIN_PROJECT_DIR"));
        assert!(!cursor.contains("echo {}"));
        // The spec is the single source: the POSIX dialect agrees.
        assert!(hook_script(AgentKind::Copilot).contains("printf '{}\\n'"));
        assert!(!hook_script(AgentKind::Cursor).contains("printf '{}"));
        assert!(hook_script_spec(AgentKind::Claude).prints_context);
        assert!(!hook_script_spec(AgentKind::Cursor).prints_context);
    }

    /// A screenshot reaches the authenticated browser route even in the
    /// headless unit-test process, and the absent window is said plainly. It
    /// must not look like a successful empty image or wait for the deadline.
    #[tokio::test]
    async fn a_browser_screenshot_without_a_window_is_clearly_refused() {
        let (state, _events, _teams, browser) = BridgeState::new("hook", "browser");
        drop(browser);
        let request = Request::builder()
            .header(BROWSER_TOKEN_HEADER, "browser")
            .body(axum::body::Body::from(
                zerocode_core::agent_browser::pack_argv(&[
                    "screenshot".into(),
                    "browser-1".into(),
                    "--json".into(),
                ]),
            ))
            .expect("request");
        let response = receive_browser_command(State(state), request).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), MAX_HOOK_BODY_BYTES)
            .await
            .expect("response body");
        assert_eq!(
            String::from_utf8_lossy(&body),
            "zerocode-browser: this window is no longer listening\n"
        );
    }

    #[test]
    fn a_poisoned_delivery_lock_recovers_instead_of_disabling_every_future_session() {
        let (state, _events, _teams, _browser) = BridgeState::new("token", "browser");
        let held = state.selection_context.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = held.lock().expect("fresh lock");
            panic!("poison the test lock");
        });
        assert!(state.selection_context.is_poisoned());

        let envelope = HookEnvelope {
            agent: AgentKind::Claude,
            pane_key: "pane/1".into(),
            tab_id: String::new(),
            launch_token: String::new(),
            worktree_id: String::new(),
            env: String::new(),
            version: String::new(),
            hook_event_name: "SessionStart".into(),
            payload: r#"{"session_id":"s","source":"startup"}"#.into(),
        };
        let event = orchestration_contract::context_event(&envelope, false).expect("event");
        assert!(tracked_context(&state, event).is_some());
    }
}

#[cfg(test)]
mod prompt_knowledge_tests {
    use super::compose_additional_context;
    use super::orchestration_contract::ContextReply;

    fn contract() -> ContextReply {
        ContextReply {
            event_name: "UserPromptSubmit".to_string(),
            context: "contract",
        }
    }

    /// The contract, the block and the pointer are ONE answer, and any of them
    /// alone is whole.
    ///
    /// The pointer joining rather than replacing is the property that matters:
    /// the once-per-context tracker has already spent the contract by the time
    /// this runs, so an answer that dropped it would take it away from the
    /// agent for the rest of the session.
    #[test]
    fn the_contract_the_block_and_the_pointer_share_one_additional_context() {
        assert_eq!(compose_additional_context(None, None, None, None), None);
        assert_eq!(
            compose_additional_context(Some(contract()), None, None, None),
            Some(("UserPromptSubmit".to_string(), "contract".to_string()))
        );
        assert_eq!(
            compose_additional_context(
                None,
                Some(("UserPromptSubmit".to_string(), "## block".to_string())),
                None,
                None
            ),
            Some(("UserPromptSubmit".to_string(), "## block".to_string()))
        );
        assert_eq!(
            compose_additional_context(
                Some(contract()),
                Some(("UserPromptSubmit".to_string(), "## block".to_string())),
                None,
                None
            ),
            Some((
                "UserPromptSubmit".to_string(),
                "contract\n\n## block".to_string()
            ))
        );
        // A pointer alone names the one event it can be answering.
        assert_eq!(
            compose_additional_context(
                None,
                None,
                None,
                Some(("SessionStart", "mail".to_string()))
            ),
            Some(("SessionStart".to_string(), "mail".to_string()))
        );
        // And with company it goes last, taking nothing away.
        assert_eq!(
            compose_additional_context(
                Some(contract()),
                Some(("UserPromptSubmit".to_string(), "## block".to_string())),
                None,
                Some(("UserPromptSubmit", "mail".to_string()))
            ),
            Some((
                "UserPromptSubmit".to_string(),
                "contract\n\n## block\n\nmail".to_string()
            ))
        );
        // The turn's brief stands after the vault's block and before the
        // pointer, and alone it is the whole answer.
        assert_eq!(
            compose_additional_context(
                Some(contract()),
                Some(("UserPromptSubmit".to_string(), "## block".to_string())),
                Some(("UserPromptSubmit".to_string(), "brief".to_string())),
                Some(("UserPromptSubmit", "mail".to_string()))
            ),
            Some((
                "UserPromptSubmit".to_string(),
                "contract\n\n## block\n\nbrief\n\nmail".to_string()
            ))
        );
        assert_eq!(
            compose_additional_context(
                None,
                None,
                Some(("UserPromptSubmit".to_string(), "brief".to_string())),
                None
            ),
            Some(("UserPromptSubmit".to_string(), "brief".to_string()))
        );
    }
}
