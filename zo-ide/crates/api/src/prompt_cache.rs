use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::types::{
    CacheControl, InputContentBlock, InputMessage, MessageRequest, MessageResponse, SystemBlock,
    Usage,
};

/// Local TTL for the on-disk completion cache: how long a stored
/// `MessageResponse` may be replayed for a byte-identical request. Deliberately
/// short — the request fingerprint covers model/system/tools/messages but not
/// the files those messages reference, so a longer window risks replaying an
/// answer after the underlying tree changed. Identical requests are rare inside
/// a turn loop (messages grow each turn), so a small window loses little.
const DEFAULT_COMPLETION_TTL_SECS: u64 = 30;
/// Mirrors the *provider's* server-side prompt-cache lifetime so
/// [`detect_cache_break`] can tell a legitimate TTL expiry from an unexpected
/// break.
///
/// This must match the TTL the **conversation** breakpoints request
/// (`runtime::mark_conversation_cache_breakpoints`, now
/// `CacheControl::ephemeral` — 5 minutes), because a drop in cache reads is a
/// statement about the conversation prefix. The system blocks keep the
/// 1-hour `extended-cache-ttl-2025-04-11` beta and outlive it; when only they
/// survive a gap the read falls to roughly the fixed prefix rather than to
/// zero, and that drop is still the conversation TTL expiring on schedule.
/// Anthropic's cache is a sliding window (each hit refreshes the TTL), so
/// within an active session the prefix stays warm regardless.
///
/// What this costs, stated plainly: between 5 minutes and 1 hour a genuine
/// provider-side eviction is now indistinguishable from an expiry and gets
/// excused as one. That band held 5.7% of the evictions this ledger recorded
/// while running at 1h, and buying the distinction back means paying the 2.0x
/// write premium on every request — the trade
/// `mark_conversation_cache_breakpoints` measured and declined.
const DEFAULT_PROMPT_TTL_SECS: u64 = 5 * 60;
const DEFAULT_BREAK_MIN_DROP: u32 = 2_000;
const MAX_SANITIZED_LENGTH: usize = 80;
const REQUEST_FINGERPRINT_VERSION: u32 = 1;
const REQUEST_FINGERPRINT_PREFIX: &str = "v1";
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Minimum re-billed volume (input + cache-creation tokens) for a request to
/// count toward the low-cache-hit-ratio streak. Below this floor a poor ratio
/// is cheap noise (a short request naturally has little to read from cache);
/// above it, a poor ratio means real money re-billed.
const LOW_CACHE_HIT_VOLUME_FLOOR: u64 = 50_000;
/// Consecutive low-cache-hit requests that trip the one-time warning. An edge
/// trigger — the warning fires only the request the streak first reaches this
/// value, not on every subsequent request, so a long-running degraded session
/// gets one line instead of one per turn. Public so the doctor warns at the
/// same point the live session would have.
pub const LOW_CACHE_HIT_STREAK_WARNING_THRESHOLD: u32 = 3;

#[derive(Debug, Clone)]
pub struct PromptCacheConfig {
    pub session_id: String,
    pub completion_ttl: Duration,
    pub prompt_ttl: Duration,
    pub cache_break_min_drop: u32,
    /// Whether [`PromptCache::lookup_completion`] may answer a request from
    /// the stored completion of an identical earlier one.
    ///
    /// The cache does two separable jobs: it RECORDS what each request cost
    /// (stats, break rows, request rows) and it can ANSWER a repeat request
    /// without asking the provider. A caller that wants only the first job
    /// sets this false — see [`Self::recording_only`].
    pub completion_lookup: bool,
}

impl PromptCacheConfig {
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            completion_ttl: Duration::from_secs(DEFAULT_COMPLETION_TTL_SECS),
            prompt_ttl: Duration::from_secs(DEFAULT_PROMPT_TTL_SECS),
            cache_break_min_drop: DEFAULT_BREAK_MIN_DROP,
            completion_lookup: true,
        }
    }

    /// A cache that only keeps the books: it writes `stats.json`,
    /// `breaks.jsonl` and `requests.jsonl`, and never answers a request from a
    /// stored completion.
    ///
    /// This is what a spawned agent's client gets. Its requests were the ones
    /// no ledger recorded — Anthropic caching is prefix-based, so the cache
    /// SCOPE a sub-agent pins is a no-op there and nothing else attached a
    /// recorder — which left the most common provider path unable to say what
    /// an agent's attempt cost. Recording is purely additive; answering from a
    /// stored completion would not be, so it stays off.
    #[must_use]
    pub fn recording_only(session_id: impl Into<String>) -> Self {
        Self {
            completion_lookup: false,
            ..Self::new(session_id)
        }
    }
}

impl Default for PromptCacheConfig {
    fn default() -> Self {
        Self::new("default")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCachePaths {
    pub root: PathBuf,
    pub session_dir: PathBuf,
    pub completion_dir: PathBuf,
    pub session_state_path: PathBuf,
    pub stats_path: PathBuf,
    /// Append-only per-request cache-break ledger (`breaks.jsonl`), one JSON
    /// line per [`CacheBreakEvent`]-bearing request. `stats.json` keeps only
    /// the LAST break's reason, which made multi-break sessions untraceable
    /// (which request broke, on which axis, was lost). No live code
    /// deserializes `PromptCachePaths` today (`for_session` is the only
    /// constructor); should an old serialized copy ever be loaded,
    /// `#[serde(default)]` yields an empty path and the best-effort ledger
    /// writer simply no-ops (`open("")` = `NotFound`) rather than failing.
    #[serde(default)]
    pub breaks_path: PathBuf,
    /// Append-only per-request ledger (`requests.jsonl`), one JSON line per
    /// recorded provider response — the same event that increments
    /// [`PromptCacheStats::tracked_requests`], so the row count and that
    /// counter are the same number by construction.
    ///
    /// `breaks.jsonl` records only break *transitions*, which biases every
    /// question asked of it: the gap distribution that priced the 5-minute TTL
    /// (`docs/analysis/cache-ttl-verdict-r21.md`) had to be borrowed from a
    /// Claude Code corpus because zo's own ledger held timestamps for broken
    /// requests only, and those skew long by definition. One row per request
    /// with its own clock removes that proxy.
    ///
    /// `#[serde(default)]` for the same reason as [`Self::breaks_path`]: an
    /// old serialized copy yields an empty path and the best-effort appender
    /// no-ops rather than failing.
    #[serde(default)]
    pub requests_path: PathBuf,
}

impl PromptCachePaths {
    #[must_use]
    pub fn for_session(session_id: &str) -> Self {
        let root = base_cache_root();
        let session_dir = root.join(sanitize_path_segment(session_id));
        let completion_dir = session_dir.join("completions");
        Self {
            root,
            session_state_path: session_dir.join("session-state.json"),
            stats_path: session_dir.join("stats.json"),
            breaks_path: session_dir.join("breaks.jsonl"),
            requests_path: session_dir.join("requests.jsonl"),
            session_dir,
            completion_dir,
        }
    }

    #[must_use]
    pub fn completion_entry_path(&self, request_hash: &str) -> PathBuf {
        self.completion_dir.join(format!("{request_hash}.json"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCacheStats {
    pub tracked_requests: u64,
    /// Microcompact firings whose trim credit this session consumed, and the
    /// estimated tokens those trims cleared — the aggregate side of the
    /// per-row `trimmed_tokens_estimate` pairing, and the denominator that
    /// still counts a firing whose following drop stayed under the row
    /// threshold.
    #[serde(default)]
    pub context_trims_noted: u64,
    #[serde(default)]
    pub context_trim_tokens_noted: u64,
    pub completion_cache_hits: u64,
    pub completion_cache_misses: u64,
    pub completion_cache_writes: u64,
    pub expected_invalidations: u64,
    pub unexpected_cache_breaks: u64,
    pub total_cache_creation_input_tokens: u64,
    pub total_cache_read_input_tokens: u64,
    /// Lifetime uncached (`input_tokens`) volume. Required for an honest hit
    /// ratio: providers without a cache-creation concept (the OpenAI-compat
    /// path reports `cache_creation_input_tokens: 0` always) make
    /// `read/(read+creation)` degenerate to a constant 100%, hiding every
    /// cold request in the denominator it never entered.
    #[serde(default)]
    pub total_input_tokens: u64,
    /// The model this session last sent a request on, and the provider family
    /// derived from it.
    ///
    /// Without these a session's numbers cannot be attributed. Measured: a
    /// sub-agent session showed 13 requests, 1.22M `total_input_tokens`, and
    /// **zero** cache read or creation — and nothing anywhere could say whether
    /// that was a provider with no prompt cache, a prompt under its threshold,
    /// or a defect. `breaks.jsonl` rows do carry model and provider, but a
    /// session that never broke has no rows at all, which is exactly the
    /// all-uncached case that most needs explaining.
    ///
    /// `#[serde(default)]` so a state file written before these existed still
    /// loads (see the two round-trip tests that would otherwise turn red).
    ///
    /// These shipped with a test and no writer: nothing assigned them for the
    /// whole life of the fields, and 720 of 720 real session directories on
    /// this machine carried `last_model: null` while
    /// `every_request_stamps_what_it_ran_on` stayed green. The test reused a
    /// fixed session id and had no cache root of its own, and `read_json`
    /// falls back to the developer's real `~/.zo` even under a test
    /// `ZO_CONFIG_HOME` (that variable PREPENDS a root, it does not replace
    /// the set — `core_types::paths::zo_global_config_roots`). So
    /// `PromptCache::new` loaded a stats file some lost build had written with
    /// the model already in it, and the assertion passed on that file instead
    /// of on the code. The cost landed on the round that trusted it: r20's
    /// provider split had to be inferred from arithmetic artifacts of the
    /// numbers themselves.
    #[serde(default)]
    pub last_model: Option<String>,
    #[serde(default)]
    pub last_provider: Option<String>,
    pub last_cache_creation_input_tokens: Option<u32>,
    pub last_cache_read_input_tokens: Option<u32>,
    pub last_request_hash: Option<String>,
    pub last_completion_cache_key: Option<String>,
    pub last_break_reason: Option<String>,
    pub last_cache_source: Option<String>,
    /// Index of the first message whose hash differs from the immediately
    /// preceding request's message at the same position. `None` when the
    /// current request's messages are a pure prefix-preserving extension of
    /// the previous request (ordinary turn growth) or when there is no prior
    /// request to compare against (first request this process has observed).
    /// See `first_divergence`.
    #[serde(default)]
    pub last_first_divergence_index: Option<usize>,
    /// Length of the matching prefix between this request's messages and the
    /// previous request's — i.e. how many leading messages are byte-identical
    /// before [`Self::last_first_divergence_index`] (or the full overlap when
    /// there is no divergence).
    #[serde(default)]
    pub last_prefix_stable_messages: usize,
    /// Message count of the immediately preceding tracked request (0 if none).
    #[serde(default)]
    pub last_prev_message_count: usize,
    /// Message count of the most recently tracked request.
    #[serde(default)]
    pub last_message_count: usize,
    /// Consecutive requests (ending at the most recent) whose cache-hit ratio
    /// was below 20% while re-billing more than
    /// `LOW_CACHE_HIT_VOLUME_FLOOR` tokens. Resets to 0 the moment a request
    /// clears either threshold.
    #[serde(default)]
    pub low_cache_hit_streak: u32,
    /// Lifetime count of requests that counted toward a low-cache-hit streak
    /// (i.e. every request that incremented [`Self::low_cache_hit_streak`],
    /// including ones that did not themselves trip the warning).
    #[serde(default)]
    pub total_low_cache_hit_requests: u64,
    /// Re-billed tokens (input + cache-creation) accumulated across the
    /// in-progress low-cache-hit streak, reset whenever the streak breaks.
    /// Backs the "~`XXk` tokens" figure in `format_low_cache_hit_warning`.
    /// Persisted (rather than kept in a transient field) for the same reason
    /// `TrackedPromptState::message_hashes` is: the non-Anthropic path
    /// reconstructs `PromptCache` fresh on every call, so only disk-backed
    /// state survives between consecutive requests in a streak.
    #[serde(default)]
    pub low_cache_hit_streak_tokens: u64,
}

/// What one wire message IS, without its text: enough to name a prefix rewrite
/// without storing (or ever logging) prompt content.
///
/// The per-message hashes answer *where* history diverged; they cannot answer
/// *what* changed there, and that gap cost a full investigation: a ledger full
/// of "history diverged at message 275/1142" rows, five of them at the same
/// index, with no way to tell an in-memory microcompact clear from a
/// tool-history rewrite from a re-truncated result. A role, the block kinds,
/// and a byte count separate those three on sight.
/// Wire form: `"role|kinds|bytes"`, one line instead of six.
///
/// Not cosmetic. `session-state.json` is rewritten on every request and holds
/// one entry per message — 1,157 on a real session — and the shared writer
/// pretty-prints. As a struct that is ~170 kB re-written per request; as a
/// string it is ~40 kB. A diagnostic must not cost a gigabyte of disk writes a
/// day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMessageShape {
    /// Wire role — `"user"` or `"assistant"`.
    pub role: String,
    /// Block kinds in wire order, comma-joined, tool blocks named
    /// (`"tool_use:Agent,text"`). Truncated past
    /// `MAX_SHAPE_KINDS` blocks with a `"+N"` tail, since a coalesced tool
    /// run can carry dozens and the tail adds nothing to the diagnosis.
    pub kinds: String,
    /// Serialized bytes with `cache_control` stripped — the same form the
    /// hashes are taken over, so a size change here is exactly a change the
    /// provider's cache would see.
    pub bytes: u32,
}

/// Block kinds kept in [`WireMessageShape::kinds`] before summarizing the rest.
const MAX_SHAPE_KINDS: usize = 6;

impl Serialize for WireMessageShape {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{}|{}|{}", self.role, self.kinds, self.bytes))
    }
}

impl<'de> Deserialize<'de> for WireMessageShape {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        // Split the byte count off the RIGHT and the role off the LEFT, so the
        // middle field is free to hold anything without a `|`.
        let (head, bytes) = raw
            .rsplit_once('|')
            .ok_or_else(|| serde::de::Error::custom("expected role|kinds|bytes"))?;
        let (role, kinds) = head
            .split_once('|')
            .ok_or_else(|| serde::de::Error::custom("expected role|kinds|bytes"))?;
        Ok(Self {
            role: role.to_string(),
            kinds: kinds.to_string(),
            bytes: bytes
                .parse()
                .map_err(|_| serde::de::Error::custom("shape byte count must be a u32"))?,
        })
    }
}

impl WireMessageShape {
    /// Build a shape from one already-`cache_control`-stripped message value.
    fn from_stripped(value: &serde_json::Value) -> Self {
        let role = value
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_string();
        let blocks = value.get("content").and_then(serde_json::Value::as_array);
        let kinds = blocks.map_or_else(String::new, |blocks| {
            let mut rendered: Vec<String> = blocks
                .iter()
                .take(MAX_SHAPE_KINDS)
                .map(Self::block_kind)
                .collect();
            if blocks.len() > MAX_SHAPE_KINDS {
                rendered.push(format!("+{}", blocks.len() - MAX_SHAPE_KINDS));
            }
            rendered.join(",")
        });
        Self {
            role,
            kinds,
            bytes: u32::try_from(
                serde_json::to_string(value).map_or(0, |serialized| serialized.len()),
            )
            .unwrap_or(u32::MAX),
        }
    }

    /// One block's kind, with the tool name appended for `tool_use` — naming
    /// the tool is what turns "a tool block changed" into "the `Agent` call was
    /// rewritten", which is the whole point of recording shapes.
    fn block_kind(block: &serde_json::Value) -> String {
        let kind = block
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?");
        match block.get("name").and_then(serde_json::Value::as_str) {
            Some(name) => format!("{kind}:{name}"),
            None => kind.to_string(),
        }
    }

    /// `"user [tool_result] 12.4kB -> 96B"` — the one-line form the break
    /// reason carries.
    fn describe_change(previous: &Self, current: &Self) -> String {
        let role = if previous.role == current.role {
            previous.role.clone()
        } else {
            format!("{} -> {}", previous.role, current.role)
        };
        let kinds = if previous.kinds == current.kinds {
            format!("[{}]", previous.kinds)
        } else {
            format!("[{}] -> [{}]", previous.kinds, current.kinds)
        };
        format!(
            "{role} {kinds} {} -> {}",
            format_bytes(previous.bytes),
            format_bytes(current.bytes)
        )
    }
}

fn format_bytes(bytes: u32) -> String {
    if bytes >= 1024 {
        format!("{:.1}kB", f64::from(bytes) / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

/// The message a prefix rewrite happened at, in both its old and new shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DivergedWireMessage {
    pub index: usize,
    pub previous: WireMessageShape,
    pub current: WireMessageShape,
    /// Whether this index was rewritten in place or merely slid — see
    /// `divergence_shift`. `None` when no alignment explains the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// The four axis flags mirror the four independent request-fingerprint hashes
// (model/system/tools/messages) — any subset can change together, so they are
// genuinely independent booleans, not an enum in disguise.
#[allow(clippy::struct_excessive_bools)]
pub struct CacheBreakEvent {
    pub unexpected: bool,
    pub reason: String,
    pub previous_cache_read_input_tokens: u32,
    pub current_cache_read_input_tokens: u32,
    pub token_drop: u32,
    /// Which fingerprint axes changed versus the previous request — the
    /// structured form of `reason`, so the per-request break ledger can be
    /// filtered without string parsing. Key signatures: `messages_changed`
    /// with `messages_truncated` is a history shrink (compaction/rewind/
    /// elision — the most common legitimate full-prefix rewrite); all four
    /// `false` with `unexpected: true` means our payload was byte-stable yet
    /// cache reads dropped (provider-side miss / eviction).
    #[serde(default)]
    pub model_changed: bool,
    #[serde(default)]
    pub system_changed: bool,
    #[serde(default)]
    pub tools_changed: bool,
    #[serde(default)]
    pub messages_changed: bool,
    /// True when the message history got SHORTER while its remaining prefix
    /// stayed intact — `first_divergence` alone cannot tell that apart from
    /// an ordinary tail append (both report no divergence index).
    #[serde(default)]
    pub messages_truncated: bool,
    /// Seconds since the previous tracked request — the TTL-expiry evidence
    /// for the all-axes-stable case.
    #[serde(default)]
    pub elapsed_secs: u64,
    /// What changed at the divergence, when there was one and both requests
    /// recorded a shape for that index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diverged_message: Option<DivergedWireMessage>,
    /// Tool names this request advertises that the previous one did not, and
    /// vice versa. Both empty on a `tools_changed` break means the NAMES are
    /// identical and something else about the definitions moved (order, a
    /// description, a schema) — a different defect with a different fix, and
    /// previously indistinguishable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_removed: Vec<String>,
    /// Wire model id of the request that broke, and its provider family. See
    /// [`CacheBreakLedgerRow::model`] for what they are for and how the family
    /// is derived — the row is the persisted copy, this is the live one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheRecord {
    pub cache_break: Option<CacheBreakEvent>,
    pub stats: PromptCacheStats,
    /// Set on the request where [`PromptCacheStats::low_cache_hit_streak`]
    /// first reaches [`LOW_CACHE_HIT_STREAK_WARNING_THRESHOLD`] — a one-line,
    /// one-time-per-streak notice for a caller to surface to the user
    /// (independent of `cache_break`, which stays `None` when the cache is
    /// merely *staying* cold rather than freshly dropping).
    pub low_cache_hit_warning: Option<String>,
}

/// Pending context-trim credits, keyed by prompt-cache session id.
///
/// The compaction planner (in `runtime`, which depends on this crate and so
/// cannot be called back into) deposits the estimated tokens a microcompact
/// cleared; the SAME session's next `record_usage` withdraws it and stamps
/// the figure on the break row that trim is about to cause. This is the
/// "missing third column" the ledger doc used to declare unrecordable: a row
/// said what got re-billed, never what the trim bought, so the firing could
/// not be priced. Keyed by session id so concurrent sessions/subagents in
/// one process can never claim each other's credit; a credit whose session
/// never sends another request idles harmlessly for the process lifetime.
fn pending_context_trims() -> &'static Mutex<std::collections::HashMap<String, u64>> {
    static PENDING: std::sync::OnceLock<Mutex<std::collections::HashMap<String, u64>>> = std::sync::OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Deposit a microcompact's estimated cleared tokens for `session_id`.
/// Accumulates: two firings before the next request price as one combined
/// trim, which is what the single following break row actually reflects.
pub fn note_context_trim(session_id: &str, estimated_tokens_cleared: u64) {
    if estimated_tokens_cleared == 0 {
        return;
    }
    let mut pending = pending_context_trims()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *pending.entry(session_id.to_string()).or_default() += estimated_tokens_cleared;
}

fn take_pending_context_trim(session_id: &str) -> u64 {
    pending_context_trims()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id)
        .unwrap_or(0)
}

/// The attempt each prompt-cache session is currently spending requests on,
/// keyed by prompt-cache session id.
///
/// The same shape, and the same reason, as [`pending_context_trims`]: the
/// depositor lives in `runtime` (which depends on this crate and so cannot be
/// called back into) and the withdrawer is [`PromptCache::record_usage`],
/// which cannot be given the value as an argument on every path — the
/// Anthropic stream records from inside `MessageStream::observe_event`, far
/// below any caller that knows the attempt. A field on `PromptCacheInner`
/// would not work either: the non-Anthropic recorder constructs a fresh
/// `PromptCache` per request, so the value must outlive the instance without
/// costing a file read on a path that runs once per provider response.
///
/// Keyed by session id so concurrent sessions and sub-agents in one process
/// never claim each other's attempt. A session whose runtime went away leaves
/// one small entry idling for the process lifetime — the same bounded cost
/// the trim map accepts.
fn attempt_slots() -> &'static Mutex<std::collections::HashMap<String, String>> {
    static SLOTS: std::sync::OnceLock<Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Declare the attempt `session_id`'s next requests belong to. Called once per
/// turn by the conversation runtime — never per request — so the cost is a map
/// write per turn, not per response. An empty `attempt` clears the slot, so a
/// session that stops having one stops stamping a stale key.
pub fn note_attempt(session_id: &str, attempt: &str) {
    let mut slots = attempt_slots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if attempt.is_empty() {
        slots.remove(session_id);
        return;
    }
    match slots.get_mut(session_id) {
        Some(current) if current == attempt => {}
        Some(current) => {
            current.clear();
            current.push_str(attempt);
        }
        None => {
            slots.insert(session_id.to_string(), attempt.to_string());
        }
    }
}

/// The attempt `session_id` is currently spending requests on, or empty.
#[must_use]
pub fn attempt_for_session(session_id: &str) -> String {
    attempt_slots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session_id)
        .cloned()
        .unwrap_or_default()
}

/// The warm prefix each wire model still holds for `session_id`, read from
/// the session's persisted state — the same file both provider paths write —
/// so a caller that never holds the `PromptCache` instance (the turn host,
/// deciding before the first request leaves) can still price a stay against
/// a switch. Empty for a session that has not recorded a request.
#[must_use]
pub fn warm_prefixes_for_session(session_id: &str) -> BTreeMap<String, WarmPrefix> {
    let paths = PromptCachePaths::for_session(session_id);
    read_json::<TrackedPromptState>(&paths.session_state_path)
        .map(|state| state.warm_prefixes)
        .unwrap_or_default()
}

/// How many turn attempts keep their plan shape in the process slot. A
/// verdict about a turn can land a turn or two after it (a review spawn that
/// finishes during the next turn; a gate at the turn's end); nothing looks
/// further back, so the slot forgets the oldest beyond this many and never
/// grows with a long session.
const PLAN_SHAPE_SLOTS: usize = 64;

fn plan_shape_slots() -> &'static Mutex<std::collections::VecDeque<(String, String)>> {
    static SLOTS: std::sync::OnceLock<Mutex<std::collections::VecDeque<(String, String)>>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(std::collections::VecDeque::new()))
}

/// Remember the plan shape a turn attempt ran under (a `PlanShape` label —
/// `solo`, `host-prelude:<w>`, …), keyed by the attempt, so a verdict
/// recorded about that attempt later — by a gate, a check command, a review
/// spawn — carries the shape without reaching the host that decided it. The
/// same door pattern as [`note_attempt`]: the host deposits at the moment it
/// decides, the recorders withdraw by key. Empty arguments deposit nothing.
pub fn note_plan_shape(attempt: &str, shape_label: &str) {
    if attempt.is_empty() || shape_label.is_empty() {
        return;
    }
    let mut slots = plan_shape_slots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = slots.iter_mut().find(|(key, _)| key == attempt) {
        entry.1.clear();
        entry.1.push_str(shape_label);
        return;
    }
    if slots.len() >= PLAN_SHAPE_SLOTS {
        slots.pop_front();
    }
    slots.push_back((attempt.to_string(), shape_label.to_string()));
}

/// The plan shape label deposited for `attempt`, if the host said one.
#[must_use]
pub fn plan_shape_for_attempt(attempt: &str) -> Option<String> {
    plan_shape_slots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(key, _)| key == attempt)
        .map(|(_, shape)| shape.clone())
}

#[derive(Debug, Clone)]
pub struct PromptCache {
    inner: Arc<Mutex<PromptCacheInner>>,
}

impl PromptCache {
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_config(PromptCacheConfig::new(session_id))
    }

    #[must_use]
    pub fn with_config(config: PromptCacheConfig) -> Self {
        let paths = PromptCachePaths::for_session(&config.session_id);
        maybe_spawn_stale_session_sweep(&paths);
        let stats = read_json::<PromptCacheStats>(&paths.stats_path).unwrap_or_default();
        let previous = read_json::<TrackedPromptState>(&paths.session_state_path);
        Self {
            inner: Arc::new(Mutex::new(PromptCacheInner {
                config,
                paths,
                stats,
                previous,
            })),
        }
    }

    #[must_use]
    pub fn paths(&self) -> PromptCachePaths {
        self.lock().paths.clone()
    }

    #[must_use]
    pub fn stats(&self) -> PromptCacheStats {
        self.lock().stats.clone()
    }

    #[must_use]
    pub fn lookup_completion(&self, request: &MessageRequest) -> Option<MessageResponse> {
        let request_hash = request_hash_hex(request);
        let (paths, ttl) = {
            let inner = self.lock();
            // A recording-only cache never answers: not a miss (which would
            // be a fact about the store), simply not asked.
            if !inner.config.completion_lookup {
                return None;
            }
            (inner.paths.clone(), inner.config.completion_ttl)
        };
        let entry_path = paths.completion_entry_path(&request_hash);
        let entry = read_json::<CompletionCacheEntry>(&entry_path);
        let Some(entry) = entry else {
            let mut inner = self.lock();
            inner.stats.completion_cache_misses += 1;
            inner.stats.last_completion_cache_key = Some(request_hash);
            persist_state(&inner);
            return None;
        };

        if entry.fingerprint_version != current_fingerprint_version() {
            let mut inner = self.lock();
            inner.stats.completion_cache_misses += 1;
            inner.stats.last_completion_cache_key = Some(request_hash.clone());
            let _ = fs::remove_file(entry_path);
            persist_state(&inner);
            return None;
        }

        let expired = now_unix_secs().saturating_sub(entry.cached_at_unix_secs) >= ttl.as_secs();
        let mut inner = self.lock();
        inner.stats.last_completion_cache_key = Some(request_hash.clone());
        if expired {
            inner.stats.completion_cache_misses += 1;
            let _ = fs::remove_file(entry_path);
            persist_state(&inner);
            return None;
        }

        inner.stats.completion_cache_hits += 1;
        apply_usage_to_stats(
            &mut inner.stats,
            &entry.response.usage,
            &request_hash,
            "completion-cache",
        );
        inner.previous = Some(TrackedPromptState::from_usage(
            request,
            &entry.response.usage,
        ));
        persist_state(&inner);
        Some(entry.response)
    }

    #[must_use]
    pub fn record_response(
        &self,
        request: &MessageRequest,
        response: &MessageResponse,
    ) -> PromptCacheRecord {
        self.record_usage_internal(request, &response.usage, Some(response))
    }

    #[must_use]
    pub fn record_usage(&self, request: &MessageRequest, usage: &Usage) -> PromptCacheRecord {
        self.record_usage_internal(request, usage, None)
    }

    fn record_usage_internal(
        &self,
        request: &MessageRequest,
        usage: &Usage,
        response: Option<&MessageResponse>,
    ) -> PromptCacheRecord {
        let request_hash = request_hash_hex(request);
        let mut inner = self.lock();
        adopt_peer_stats(&mut inner);
        let trimmed_tokens = withdraw_trim_credit(&mut inner);
        let previous = inner.previous.clone();
        let fingerprints = RequestFingerprints::from_request(request);
        let mut current = TrackedPromptState::from_fingerprints(&fingerprints, usage);
        current.carry_warm_prefixes(previous.as_ref(), request, usage);

        // `previous.message_hashes` — NOT a separate in-memory field — is the
        // basis for divergence comparison. This matters: `record_usage_internal`
        // is called through two very different lifetimes. The Anthropic client
        // holds one `PromptCache` for the whole session, so an in-memory-only
        // field would survive there; but `record_non_anthropic_prompt_cache_usage`
        // (the GPT / OpenAI-compatible path) constructs a *fresh* `PromptCache`
        // on every single call — any state that lived only in `PromptCacheInner`
        // would be discarded before the next request and `first_divergence_index`
        // would silently degrade to always-`None` (worse: it would mislabel a
        // real mid-history edit as "append-only"). Riding along on
        // `TrackedPromptState`, which already round-trips through
        // `session-state.json` on every `PromptCache::new()` regardless of
        // instance lifetime, is what makes this work identically on both paths.
        let previous_message_hashes = previous.as_ref().map(|state| state.message_hashes.as_slice());
        let (first_divergence_index, prefix_stable_count) =
            first_divergence(previous_message_hashes, &fingerprints.message_hashes);
        let prev_message_count = previous.as_ref().map_or(0, |state| state.message_hashes.len());
        let current_message_count = fingerprints.message_hashes.len();

        let cache_break = detect_cache_break(
            &inner.config,
            previous.as_ref(),
            &current,
            first_divergence_index,
            current_message_count,
            &request.model,
        );

        inner.stats.tracked_requests += 1;
        // Stamp the attribution on every request, not only on a break: the
        // sessions that most need explaining are the ones that never broke.
        // These two lines were missing for the whole life of the fields — see
        // `PromptCacheStats::last_model` for how the guarding test hid that.
        inner.stats.last_model = Some(request.model.clone());
        inner.stats.last_provider = Some(provider_family_for_model(&request.model).to_string());
        apply_usage_to_stats(&mut inner.stats, usage, &request_hash, "api-response");
        inner.stats.last_first_divergence_index = first_divergence_index;
        inner.stats.last_prefix_stable_messages = prefix_stable_count;
        inner.stats.last_prev_message_count = prev_message_count;
        inner.stats.last_message_count = current_message_count;
        if let Some(event) = &cache_break {
            if event.unexpected {
                inner.stats.unexpected_cache_breaks += 1;
            } else {
                inner.stats.expected_invalidations += 1;
            }
            inner.stats.last_break_reason = Some(event.reason.clone());
            // Durable per-request record: `last_break_reason` above only keeps
            // the final break, which made multi-break sessions untraceable.
            append_break_row(
                &inner.paths,
                &CacheBreakLedgerRow {
                    seq: inner.stats.tracked_requests,
                    ts_unix_secs: now_unix_secs(),
                    unexpected: event.unexpected,
                    reason: event.reason.clone(),
                    model_changed: event.model_changed,
                    system_changed: event.system_changed,
                    tools_changed: event.tools_changed,
                    messages_changed: event.messages_changed,
                    messages_truncated: event.messages_truncated,
                    first_divergence_index,
                    prefix_stable_messages: prefix_stable_count,
                    prev_message_count,
                    message_count: current_message_count,
                    prev_cache_read: event.previous_cache_read_input_tokens,
                    cache_read: event.current_cache_read_input_tokens,
                    cache_creation: usage.cache_creation_input_tokens,
                    token_drop: event.token_drop,
                    elapsed_secs: event.elapsed_secs,
                    diverged_message: event.diverged_message.clone(),
                    tools_added: event.tools_added.clone(),
                    tools_removed: event.tools_removed.clone(),
                    // Copied from the event rather than re-derived from
                    // `request` so the live event a caller surfaces and the
                    // persisted row can never disagree about which model broke.
                    model: event.model.clone(),
                    provider: event.provider.clone(),
                    trimmed_tokens_estimate: (trimmed_tokens > 0).then_some(trimmed_tokens),
                    cache_breakpoints: current.cache_breakpoints.clone(),
                    marker_roles: current.marker_roles.clone(),
                    prev_cache_breakpoints: previous
                        .as_ref()
                        .map(|state| state.cache_breakpoints.clone())
                        .unwrap_or_default(),
                },
            );
        }

        let low_cache_hit_warning = record_low_cache_hit_streak(
            &mut inner.stats,
            usage,
            first_divergence_index,
            current_message_count,
        );

        inner.previous = Some(current);
        if let Some(response) = response {
            write_completion_entry(&inner.paths, &request_hash, response);
            inner.stats.completion_cache_writes += 1;
        }
        persist_state(&inner);
        // AFTER `persist_state` — see `append_request_row` for why the order
        // matters. `tracked_requests` was incremented above for THIS request,
        // so the seq passed here is the same ordinal `breaks.jsonl` stamps.
        let row = request_ledger_row(
            inner.stats.tracked_requests,
            &inner.config.session_id,
            request,
            usage,
            current_message_count,
            cache_break.is_some(),
            // The same predecessor `detect_cache_break` compared against, so
            // `moved` and `read_drop` describe the transition `broke` does.
            MarkerColumns::new(&fingerprints, previous.as_ref(), usage),
        );
        append_request_row(&inner.paths, &row);

        PromptCacheRecord {
            cache_break,
            stats: inner.stats.clone(),
            low_cache_hit_warning,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PromptCacheInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Debug)]
struct PromptCacheInner {
    config: PromptCacheConfig,
    paths: PromptCachePaths,
    stats: PromptCacheStats,
    previous: Option<TrackedPromptState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompletionCacheEntry {
    cached_at_unix_secs: u64,
    #[serde(default = "current_fingerprint_version")]
    fingerprint_version: u32,
    response: MessageResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TrackedPromptState {
    observed_at_unix_secs: u64,
    #[serde(default = "current_fingerprint_version")]
    fingerprint_version: u32,
    model_hash: u64,
    system_hash: u64,
    tools_hash: u64,
    messages_hash: u64,
    cache_read_input_tokens: u32,
    /// Per-message hash vector for this request, in order — the basis for
    /// [`first_divergence`] on the *next* request. Persisted alongside the
    /// rest of `TrackedPromptState` (mirrored to `session-state.json`)
    /// rather than kept in a separate process-memory field: `PromptCache`
    /// is reconstructed fresh on every call on the non-Anthropic path
    /// (`record_non_anthropic_prompt_cache_usage` builds a new instance per
    /// request), so anything that isn't disk-backed here would silently
    /// never see a "previous" vector to compare against on that path.
    /// `#[serde(default)]` so a `session-state.json` written before this
    /// field existed deserializes as an empty vector — divergence detection
    /// degrades to "no basis for comparison" for one request after an
    /// upgrade, then resumes normally, rather than failing to load at all.
    #[serde(default)]
    message_hashes: Vec<u64>,
    /// Per-message shape, parallel to [`Self::message_hashes`]. Persisted for
    /// the same reason the hashes are (the non-Anthropic path rebuilds
    /// `PromptCache` per request, so in-memory state would never see a
    /// previous), and `#[serde(default)]` so a state file written before this
    /// field existed still loads — the first break after an upgrade simply
    /// reports no shape.
    #[serde(default)]
    message_shapes: Vec<WireMessageShape>,
    /// Advertised tool names in wire order. Turns a `tools_changed` break from
    /// "some hash moved" into a name-level diff.
    #[serde(default)]
    tool_names: Vec<String>,
    /// Breakpoint indices this request carried
    /// ([`RequestFingerprints::cache_breakpoints`]). Persisted so the NEXT
    /// request's break row can say whether its anchor landed on a position the
    /// previous request actually wrote. `#[serde(default)]` so a state file
    /// from before this field loads and simply reports no anchor for one
    /// request.
    #[serde(default)]
    cache_breakpoints: Vec<usize>,
    /// Wire role at each of those positions, same order — see
    /// [`RequestFingerprints::marker_roles`]. `#[serde(default)]` for the same
    /// reason as the field above: this state is persisted, and a file written
    /// by a build without it must still load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    marker_roles: Vec<String>,
    /// The warm prefix each wire model of this session still holds, keyed by
    /// model — what a decision to stay or switch is worth in tokens. One
    /// `previous` slot could not carry this: the moment the session moved
    /// from A to B, A's numbers were overwritten, so a return to A (or a
    /// switch-cost comparison between the two) had nothing to read. Carried
    /// forward request to request and persisted with the rest of this state
    /// for the same reason the hashes are (the non-Anthropic path rebuilds
    /// `PromptCache` per request). `#[serde(default)]` so an older state
    /// file loads with an empty table.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    warm_prefixes: BTreeMap<String, WarmPrefix>,
}

/// What one wire model still has cached for this session, as of the last
/// request that went to it: the prefix the provider reported as read or
/// written (so it is warm now), when that was, and the shortest TTL the
/// request's markers asked for. `age < ttl` is the live test; the plan
/// scorer prices staying against switching with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarmPrefix {
    /// `cache_read_input_tokens + cache_creation_input_tokens` of that
    /// request — everything the provider holds for this prefix afterwards.
    pub prefix_tokens: u32,
    pub observed_at_unix_secs: u64,
    pub ttl_secs: u64,
}

/// The lifetime a warm prefix is assumed to have when the request carried no
/// TTL-bearing marker (the non-Anthropic encoders never serialize one):
/// five minutes, the provider-neutral floor the r21 TTL study settled on for
/// the rolling conversation markers. A marker that names a TTL wins.
const DEFAULT_WARM_TTL_SECS: u64 = 300;

/// The shortest lifetime a request's markers asked for, in seconds: `"5m"`
/// → 300, `"1h"` → 3600, a mixed `"5m+1h"` → the shorter (the conversation
/// markers are the ones that expire first), no marker → the default above.
fn warm_ttl_secs(marker_ttl_label: &str) -> u64 {
    marker_ttl_label
        .split('+')
        .filter_map(|label| match label.trim() {
            "5m" => Some(300),
            "1h" => Some(3600),
            _ => None,
        })
        .min()
        .unwrap_or(DEFAULT_WARM_TTL_SECS)
}

impl TrackedPromptState {
    fn from_usage(request: &MessageRequest, usage: &Usage) -> Self {
        let hashes = RequestFingerprints::from_request(request);
        Self::from_fingerprints(&hashes, usage)
    }

    fn from_fingerprints(hashes: &RequestFingerprints, usage: &Usage) -> Self {
        Self {
            observed_at_unix_secs: now_unix_secs(),
            fingerprint_version: current_fingerprint_version(),
            model_hash: hashes.model,
            system_hash: hashes.system,
            tools_hash: hashes.tools,
            messages_hash: hashes.messages,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            message_hashes: hashes.message_hashes.clone(),
            message_shapes: hashes.message_shapes.clone(),
            tool_names: hashes.tool_names.clone(),
            cache_breakpoints: hashes.cache_breakpoints.clone(),
            marker_roles: hashes.marker_roles.clone(),
            // Filled by the recorder from the previous state; a bare
            // fingerprint knows no other model's prefix.
            warm_prefixes: BTreeMap::new(),
        }
    }

    /// Carry every model's warm prefix forward from `previous` and refresh
    /// this request's model with what the provider just reported: the table
    /// outlives the request it was observed on, so a return to an earlier
    /// model still finds that model's row.
    fn carry_warm_prefixes(
        &mut self,
        previous: Option<&TrackedPromptState>,
        request: &MessageRequest,
        usage: &Usage,
    ) {
        self.warm_prefixes = previous
            .map(|state| state.warm_prefixes.clone())
            .unwrap_or_default();
        self.warm_prefixes.insert(
            request.model.clone(),
            WarmPrefix {
                prefix_tokens: usage
                    .cache_read_input_tokens
                    .saturating_add(usage.cache_creation_input_tokens),
                observed_at_unix_secs: self.observed_at_unix_secs,
                ttl_secs: warm_ttl_secs(&conversation_marker_ttl(&request.messages)),
            },
        );
    }
}

#[derive(Debug, Clone)]
struct RequestFingerprints {
    model: u64,
    system: u64,
    tools: u64,
    /// Aggregate hash of the whole `messages` array (single hash over the
    /// serialized Vec, `cache_control` markers stripped — see
    /// [`strip_message_cache_markers`]) — what [`detect_cache_break`]'s
    /// "message payload changed" check keys off of. Kept alongside
    /// `message_hashes` (below) rather than derived from it so the
    /// break-detection comparison stays a single-hash equality.
    messages: u64,
    /// Per-message hash, one entry per `messages[i]`, in order. Powers
    /// [`first_divergence`] — this is the piece the aggregate `messages` hash
    /// cannot answer ("which message changed", not just "something changed").
    message_hashes: Vec<u64>,
    /// Per-message shape, parallel to `message_hashes` — answers "what changed
    /// there", the piece a hash cannot.
    message_shapes: Vec<WireMessageShape>,
    /// Advertised tool names in wire order.
    tool_names: Vec<String>,
    /// Message indices carrying a `cache_control` marker, ascending — the
    /// breakpoints `mark_breakpoints_with_ttl` chose for THIS request.
    ///
    /// Everything else in this struct is deliberately marker-blind
    /// ([`strip_message_cache_markers`]); this field is the one place the
    /// markers are the subject. Without it the ledger could say a read was
    /// lost but never whether the anchor sat where the previous request wrote
    /// one — which is exactly the question the anchor+rolling fix
    /// (`runtime::convert_messages::previous_request_tail`) exists to answer,
    /// and answering it by re-deriving positions from a stored history is not
    /// possible after the fact.
    cache_breakpoints: Vec<usize>,
    /// The wire role at each [`Self::cache_breakpoints`] index, in the same
    /// order.
    ///
    /// The indices alone cannot tell an anchor that worked from one that
    /// overshot. The policy anchors on the newest *user-role* message behind
    /// the tail, so a marker pair reads as intended when it is
    /// `["user", …]` two apart, and as a shape the policy was not written for
    /// when the two sit adjacent — which is what the first rows carrying these
    /// fields showed, and what no clean agentic sequence reproduces. Recording
    /// the roles is the difference between seeing that and guessing at it.
    marker_roles: Vec<String>,
    /// Every marker on the request — system blocks included — with its slot,
    /// role and TTL, for [`RequestLedgerRow::markers`].
    ///
    /// Not persisted onto [`TrackedPromptState`] the way the two fields above
    /// are: the only thing the NEXT request needs from this one is the set of
    /// positions, which `cache_breakpoints` already carries, and
    /// `session-state.json` is rewritten whole on every request.
    markers: Vec<MarkerRecord>,
}

impl RequestFingerprints {
    fn from_request(request: &MessageRequest) -> Self {
        // Strip ONCE and derive all three message-side fingerprints from the
        // same values: the aggregate hash, the per-message hashes, and the
        // shapes. Serializing a 1,000-message history is not free, and the
        // three must agree about what they describe.
        let stripped = strip_message_cache_markers(&request.messages);
        Self {
            model: hash_serializable(&request.model),
            system: hash_serializable(&request.system),
            tools: hash_serializable(&request.tools),
            messages: hash_serializable(&stripped),
            message_hashes: stripped.iter().map(hash_serializable).collect(),
            message_shapes: stripped
                .iter()
                .map(WireMessageShape::from_stripped)
                .collect(),
            tool_names: request
                .tools
                .iter()
                .flatten()
                .map(|tool| tool.name.clone())
                .collect(),
            cache_breakpoints: cache_breakpoint_indices(&request.messages),
            marker_roles: marker_roles(&request.messages),
            markers: request_markers(request),
        }
    }
}

/// One `cache_control` marker on a request: where it sat, which slot of the
/// placement policy put it there, the wire role underneath it, and the
/// lifetime it asked for.
///
/// Wire form: `"index:slot:role:ttl"` — a string, not an object, for the same
/// reason [`WireMessageShape`] is one. This rides
/// [`RequestLedgerRow::markers`], which is written once per request forever;
/// as objects the four markers of a full Anthropic request cost ~150 bytes of
/// repeated key names, as strings ~80. See
/// `a_request_ledger_row_stays_small_enough_to_keep_forever` for the
/// arithmetic that budget comes from.
///
/// `index` is `-1` for a marker that is not on a message (a system block):
/// those have no position in the conversation, and `slot` already names them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkerRecord {
    /// Index into `messages`, or [`NON_MESSAGE_MARKER_INDEX`] for a system
    /// block.
    pub index: i32,
    /// Which slot of the placement policy this marker is:
    /// [`MARKER_SLOT_SYSTEM`], [`MARKER_SLOT_ANCHOR`] or
    /// [`MARKER_SLOT_ROLLING`].
    ///
    /// Derived from POSITION, not threaded from `runtime`: among the
    /// message-level markers the highest index is the rolling one — the tail
    /// marker `mark_breakpoints_with_ttl` places last and moves forward every
    /// request — and everything before it is an anchor. That derivation holds
    /// because the policy's anchor is always found strictly behind the rolling
    /// slot (`previous_request_tail` searches `messages[..rolling]`), and
    /// `breakpoint_slots_match_the_placement_policy` in
    /// `runtime::convert_messages` pins the two together against the real
    /// output of the marking function.
    ///
    /// One case the derivation cannot see: on a session's FIRST request there
    /// is no predecessor tail, so the policy falls back to the last two
    /// cacheable messages and the earlier of the two anchors nothing. The row
    /// still labels it `anchor`; [`RequestLedgerRow::moved`] is where that
    /// shows up, as the absence of a `rolled`.
    pub slot: String,
    /// The wire role of the message this marker sits on (`"user"` /
    /// `"assistant"`), empty for a system block.
    ///
    /// Not derivable from anything else in the row, and the field the first
    /// three rows that ever carried it made the case for: a marker pair reads
    /// as intended when it is `["user", …]` two apart and as a shape the
    /// policy was not written for when the two sit adjacent
    /// (`docs/analysis/cache-miss-anatomy-r29.md` §4).
    pub role: String,
    /// `"5m"` or `"1h"` — an absent `ttl` key is Anthropic's 5-minute default,
    /// recorded as `"5m"` for the reason [`conversation_marker_ttl`] gives.
    pub ttl: String,
}

/// [`MarkerRecord::index`] for a marker that is not on a message.
pub const NON_MESSAGE_MARKER_INDEX: i32 = -1;
/// A marker on a `system` block — 1h, written by `runtime::push_cache_block`.
pub const MARKER_SLOT_SYSTEM: &str = "system";
/// A message marker behind the rolling one: the position the PREVIOUS request
/// ended on, which the provider has already written a cache entry for.
pub const MARKER_SLOT_ANCHOR: &str = "anchor";
/// The newest message marker — the entry the NEXT request will anchor on.
pub const MARKER_SLOT_ROLLING: &str = "rolling";

impl Serialize for MarkerRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!(
            "{}:{}:{}:{}",
            self.index, self.slot, self.role, self.ttl
        ))
    }
}

impl<'de> Deserialize<'de> for MarkerRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        let mut parts = raw.splitn(4, ':');
        let (Some(index), Some(slot), Some(role), Some(ttl)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(serde::de::Error::custom("expected index:slot:role:ttl"));
        };
        Ok(Self {
            index: index
                .parse()
                .map_err(|_| serde::de::Error::custom("marker index must be an i32"))?,
            slot: slot.to_string(),
            role: role.to_string(),
            ttl: ttl.to_string(),
        })
    }
}

/// The lifetime a marker asked for, as the ledger writes it: an absent `ttl`
/// key is Anthropic's default, which is 5 minutes.
fn marker_ttl(control: &CacheControl) -> &str {
    control.ttl.as_deref().unwrap_or("5m")
}

/// The `cache_control` on one wire block, or `None` for the two variants that
/// cannot carry one.
///
/// Read off the TYPED block rather than off `serde_json::to_value(message)`.
/// The two find exactly the same set — `cache_control` exists on precisely
/// these five variants, and `InputMessage`'s other two fields are
/// `#[serde(skip)]` so they never reach the JSON at all — but the typed read
/// costs nothing, and this runs on every request over a history that reaches
/// 635 messages at this store's p90. Recording markers per-request rather than
/// per-break (r30) would have made that the *fifth* full serialization of the
/// same history per request; folding the three marker readers onto this one
/// leaves two.
fn block_marker(block: &InputContentBlock) -> Option<&CacheControl> {
    match block {
        InputContentBlock::Text { cache_control, .. }
        | InputContentBlock::Image { cache_control, .. }
        | InputContentBlock::Document { cache_control, .. }
        | InputContentBlock::ToolUse { cache_control, .. }
        | InputContentBlock::ToolResult { cache_control, .. } => cache_control.as_ref(),
        // Thinking blocks take no `cache_control` — the wire type has no field
        // for it, which is why `mark_last_cacheable_block` skips past them.
        InputContentBlock::Thinking { .. } | InputContentBlock::RedactedThinking { .. } => None,
    }
}

/// The marker a message carries: the LAST marked block, which is the one
/// `mark_last_cacheable_block` places.
fn message_marker(message: &InputMessage) -> Option<&CacheControl> {
    message.content.iter().rev().find_map(block_marker)
}

/// Indices of the messages carrying a `cache_control` marker, ascending.
///
/// Read off the RAW messages, before [`strip_message_cache_markers`] removes
/// them: the markers are what this one field is about.
fn cache_breakpoint_indices(messages: &[InputMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message_marker(message).is_some())
        .map(|(index, _)| index)
        .collect()
}

/// The role of every message carrying a marker, ascending by index — the
/// companion to [`cache_breakpoint_indices`], read off the same RAW messages.
fn marker_roles(messages: &[InputMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message_marker(message).is_some())
        .map(|message| message.role.clone())
        .collect()
}

/// Every message-level marker on this request, ascending by index, with the
/// rolling slot named.
///
/// Public so `runtime::convert_messages` can assert its own placement policy
/// against the labels the ledger will record — the same read-only seam
/// [`conversation_marker_ttl`] already provides for the TTL, and for the same
/// reason: `api` cannot call back into `runtime` to test the pairing from this
/// side.
#[must_use]
pub fn conversation_markers(messages: &[InputMessage]) -> Vec<MarkerRecord> {
    let mut records: Vec<MarkerRecord> = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let control = message_marker(message)?;
            Some(MarkerRecord {
                index: i32::try_from(index).unwrap_or(i32::MAX),
                slot: MARKER_SLOT_ANCHOR.to_string(),
                role: message.role.clone(),
                ttl: marker_ttl(control).to_string(),
            })
        })
        .collect();
    if let Some(newest) = records.last_mut() {
        newest.slot = MARKER_SLOT_ROLLING.to_string();
    }
    records
}

/// Every `cache_control` marker on a request — system blocks first, then the
/// conversation in wire order.
///
/// The `tools` array is deliberately absent, and that is a statement about the
/// wire type rather than an omission: [`crate::types::ToolDefinition`] has no
/// `cache_control` field, so a tool block cannot carry a marker at all in this
/// codebase and scanning for one would serialize the whole tool array per
/// request to prove a negative. `a_tool_definition_cannot_carry_a_marker` pins
/// that, so the day a tool-level breakpoint becomes possible the pin fails
/// here rather than the ledger silently under-reporting the slot count.
fn request_markers(request: &MessageRequest) -> Vec<MarkerRecord> {
    let mut records: Vec<MarkerRecord> = request
        .system
        .iter()
        .flatten()
        .filter_map(|block| {
            let SystemBlock::Text { cache_control, .. } = block;
            Some(MarkerRecord {
                index: NON_MESSAGE_MARKER_INDEX,
                slot: MARKER_SLOT_SYSTEM.to_string(),
                role: String::new(),
                ttl: marker_ttl(cache_control.as_ref()?).to_string(),
            })
        })
        .collect();
    records.extend(conversation_markers(&request.messages));
    records
}

/// How this request's marker positions compare with the previous request's —
/// [`RequestLedgerRow::moved`].
///
/// The question the provider axis needs answered is not "where are the
/// markers" but "did they land somewhere already written". `rolled` is the
/// shape the anchor+rolling policy exists to produce (this request's anchor is
/// a position the previous request marked, and the tail moved on);
/// `anchor_moved` is the shape that guarantees a miss, because every marker
/// points at content no earlier request wrote.
///
/// Empty when there is nothing to compare: no previous request recorded, or
/// neither side carried a marker (the non-Anthropic encoders never serialize
/// one). Empty is not `same` — "the instrument had no basis" and "the markers
/// did not move" are opposite readings, and r29 §4 lost a round to a field
/// that could not tell its two kinds of silence apart.
fn marker_movement(previous: Option<&[usize]>, current: &[usize]) -> &'static str {
    let Some(previous) = previous else {
        return "";
    };
    if current.is_empty() && previous.is_empty() {
        return "";
    }
    if current == previous {
        return "same";
    }
    if current.len() > previous.len() {
        return "added";
    }
    if current.len() < previous.len() {
        return "removed";
    }
    // Same count, different positions. The anchor is the oldest marker, and
    // whether IT sits on a written position is the whole question.
    match current.first() {
        Some(anchor) if previous.contains(anchor) => "rolled",
        _ => "anchor_moved",
    }
}

/// The TTL the conversation breakpoints on this request asked for, as it will
/// be written to [`RequestLedgerRow::ttl`].
///
/// `mark_breakpoints_with_ttl` stamps an `api::CacheControl` onto the message
/// blocks it marks, and that struct — `{"type":"ephemeral"}` or
/// `{"type":"ephemeral","ttl":"1h"}` — is still in the `MessageRequest` handed
/// to `record_usage_internal`. So the policy that priced this request is
/// readable here directly; no argument had to be threaded from `runtime`
/// through every `record_usage`/`record_response` call site to learn it.
///
/// An absent `ttl` key is Anthropic's default lifetime, which is 5 minutes —
/// recorded as `"5m"` rather than as an empty string so that "the default"
/// and "no marker at all" stay distinguishable. Markers that disagree join
/// with `+` (ascending, deduplicated): the anchor and the rolling marker are
/// written by one call today, but a future split of the two is exactly the
/// change this column would have to survive without silently reporting one
/// half.
///
/// Every marked block is read, not one per message: a message that somehow
/// carried two markers with different lifetimes must show as mixed here, which
/// is the one place [`MarkerRecord`] (one entry per message, the last marked
/// block) would round off.
#[must_use]
pub fn conversation_marker_ttl(messages: &[InputMessage]) -> String {
    let mut ttls: Vec<&str> = messages
        .iter()
        .flat_map(|message| message.content.iter().filter_map(block_marker))
        .map(marker_ttl)
        .collect();
    ttls.sort_unstable();
    ttls.dedup();
    ttls.join("+")
}

/// Lower `messages` to JSON with every `cache_control` key removed, for
/// fingerprinting only.
///
/// The conversation breakpoint markers (`mark_conversation_cache_breakpoints`)
/// ride the newest two messages and therefore *move forward on every
/// request by design*. The provider's prefix cache keys on content, not on
/// the markers, so a moved marker is invisible to the cache — but hashing the
/// raw blocks made the fingerprints see a fake mid-history edit at the old
/// marker position on every call: `first_divergence` pinned a bogus
/// "history diverged at message N" a couple of messages from the tail, and
/// [`detect_cache_break`] misfiled genuinely *unexpected* token drops under
/// the expected "message payload changed" reason. Stripping the markers makes
/// the fingerprint track what the provider cache actually keys on.
fn strip_message_cache_markers(messages: &[InputMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|message| {
            let mut value = serde_json::to_value(message).unwrap_or_default();
            strip_cache_control(&mut value);
            value
        })
        .collect()
}

fn strip_cache_control(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("cache_control");
            for nested in map.values_mut() {
                strip_cache_control(nested);
            }
        }
        serde_json::Value::Array(items) => {
            for nested in items {
                strip_cache_control(nested);
            }
        }
        _ => {}
    }
}

/// Find the first position where `current`'s per-message hashes diverge from
/// `previous`'s, returning `(first divergent index, matching-prefix length)`.
///
/// Returns `(None, 0)` when there is no previous vector to compare against
/// (the first request this process has observed). Returns `(None, n)` when
/// `current` is a prefix-preserving extension (or contraction) of
/// `previous` — the ordinary case of a turn appending new messages, which
/// must NOT be reported as a divergence even though the aggregate
/// `messages_hash` differs on every such turn. Otherwise returns
/// `(Some(index), index)` for the first index whose hash differs.
fn first_divergence(previous: Option<&[u64]>, current: &[u64]) -> (Option<usize>, usize) {
    let Some(previous) = previous else {
        return (None, 0);
    };
    let common = previous.len().min(current.len());
    match (0..common).find(|&index| previous[index] != current[index]) {
        Some(index) => (Some(index), index),
        None => (None, common),
    }
}

/// How far apart two histories may be re-aligned before [`divergence_shift`]
/// gives up. Every shape seen in the ledger moves by one or two messages; a
/// wider search would start finding coincidences rather than shifts.
const MAX_DIVERGENCE_SHIFT: usize = 4;

/// Whether a divergence is one message rewritten where it stands, or the whole
/// tail sliding because messages were inserted or removed ahead of it.
///
/// [`DivergedWireMessage`] compares `previous[i]` against `current[i]`, which
/// reads as an in-place edit either way: a big `tool_result` "becoming" a small
/// one is exactly what a single deletion ahead of it looks like, because index
/// `i` now holds the message that used to sit at `i + 1`. The two have
/// completely different causes and completely different fixes, and the ledger
/// could not tell them apart — 81 rows carrying 21.5M dropped cache-read tokens
/// sat unexplained on that ambiguity (`docs/analysis/prompt-cache-breaks.md`).
///
/// Decided by re-aligning the hash tails: if `current[i + k..]` matches
/// `previous[i..]` then `k` messages were inserted, if `current[i..]` matches
/// `previous[i + k..]` then `k` were removed, and if the tails past `i` already
/// match then message `i` alone was rewritten. `None` when no alignment
/// explains it — the tails genuinely differ, which is its own answer.
fn divergence_shift(previous: &[u64], current: &[u64], index: usize) -> Option<&'static str> {
    // Tails that align vacuously (nothing left to compare) prove nothing about
    // a shift, so the honest reading of a divergence at the very end is that
    // the message itself changed.
    let aligns = |left: &[u64], right: &[u64]| {
        let overlap = left.len().min(right.len());
        overlap > 0 && left[..overlap] == right[..overlap]
    };
    if index >= previous.len() || index >= current.len() {
        return None;
    }
    let (current_tail, previous_tail) = (&current[index + 1..], &previous[index + 1..]);
    // Two empty tails align vacuously, and `aligns` rejects that on purpose —
    // but at the very last message there is no tail to slide, so a rewrite in
    // place is the only thing the change can be.
    if aligns(current_tail, previous_tail) || (current_tail.is_empty() && previous_tail.is_empty()) {
        return Some("edited");
    }
    for shift in 1..=MAX_DIVERGENCE_SHIFT {
        if index + shift < current.len() && aligns(&current[index + shift..], &previous[index..]) {
            return Some(match shift {
                1 => "inserted:1",
                2 => "inserted:2",
                3 => "inserted:3",
                _ => "inserted:4",
            });
        }
        if index + shift < previous.len() && aligns(&current[index..], &previous[index + shift..]) {
            return Some(match shift {
                1 => "removed:1",
                2 => "removed:2",
                3 => "removed:3",
                _ => "removed:4",
            });
        }
    }
    None
}

/// Provider family for a wire model id, as
/// [`crate::ProviderKind::rate_limit_key`].
///
/// Reuses the provider registry's own classification (`metadata_for_model`,
/// which resolves aliases and `provider/model` refs on the way) instead of
/// prefix-matching model ids here — a second taxonomy would drift from the one
/// that actually routes requests, and this value is only useful if it agrees
/// with it. `""` when the id belongs to no known family (an unknown or
/// custom-only id), which the row then omits entirely.
///
/// Registry-only: no I/O and no auth probing (`detect_provider_kind`'s
/// credential fallbacks are deliberately NOT used — a row must describe the
/// request, not the machine's current logins). Called only when a break has
/// already been detected, so its allocations are off the per-request path.
fn provider_family_for_model(model: &str) -> &'static str {
    crate::providers::metadata_for_model(model)
        .map_or("", |metadata| metadata.provider.rate_limit_key())
}

/// The `fingerprint version changed` break: our own schema moved, so nothing
/// about the request can be compared across it. Split out of
/// [`detect_cache_break`] to keep that function readable, not because it varies.
fn fingerprint_bump_break(
    previous: &TrackedPromptState,
    current: &TrackedPromptState,
    elapsed: u64,
    model: &str,
) -> CacheBreakEvent {
    CacheBreakEvent {
        unexpected: false,
        reason: format!(
            "fingerprint version changed (v{} -> v{})",
            previous.fingerprint_version, current.fingerprint_version
        ),
        previous_cache_read_input_tokens: previous.cache_read_input_tokens,
        current_cache_read_input_tokens: current.cache_read_input_tokens,
        token_drop: previous
            .cache_read_input_tokens
            .saturating_sub(current.cache_read_input_tokens),
        model_changed: false,
        system_changed: false,
        tools_changed: false,
        messages_changed: false,
        messages_truncated: false,
        elapsed_secs: elapsed,
        diverged_message: None,
        tools_added: Vec::new(),
        tools_removed: Vec::new(),
        model: model.to_string(),
        provider: provider_family_for_model(model).to_string(),
    }
}

/// Which fingerprint axes moved — the four independent hashes plus the shrink
/// flag, grouped so the reason builder takes one argument instead of five
/// booleans in a row.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools, reason = "one flag per independent fingerprint axis")]
struct BreakAxes {
    model_changed: bool,
    system_changed: bool,
    tools_changed: bool,
    messages_changed: bool,
    messages_truncated: bool,
}

/// Wire message counts on either side of the break.
#[derive(Debug, Clone, Copy)]
struct MessageCounts {
    previous: usize,
    current: usize,
}

/// One human-readable clause per changed axis, in fingerprint order. Empty when
/// nothing our side controls moved — the caller reads that as "provider-side or
/// TTL".
fn break_reasons(
    axes: BreakAxes,
    tools_added: &[String],
    tools_removed: &[String],
    diverged_message: Option<&DivergedWireMessage>,
    counts: MessageCounts,
    first_divergence_index: Option<usize>,
) -> Vec<String> {
    let mut reasons: Vec<String> = Vec::new();
    if axes.model_changed {
        reasons.push("model changed".to_string());
    }
    if axes.system_changed {
        reasons.push("system prompt changed".to_string());
    }
    if axes.tools_changed {
        reasons.push(format!(
            "tool definitions changed{}",
            format_tool_diff(tools_added, tools_removed)
        ));
    }
    if axes.messages_changed {
        // Enrich with *where* the history diverged, and with what changed there
        // — the pieces the old "message payload changed" wording could never
        // answer, so every ordinary tail-append turn (which also changes the
        // aggregate hash) looked identical to an actual mid-history edit.
        let current_message_count = counts.current;
        let detail = match first_divergence_index {
            Some(index) => {
                let shape = diverged_message.map_or_else(String::new, |diverged| {
                    // Lead with the shift: "16.9kB -> 167B" reads as a rewrite
                    // even when it is one deletion sliding the tail under the
                    // index, and that misreading is what the shift exists to
                    // stop.
                    let shift = diverged
                        .shift
                        .as_deref()
                        .map_or_else(String::new, |shift| format!("{shift}, "));
                    format!(
                        " ({shift}{})",
                        WireMessageShape::describe_change(&diverged.previous, &diverged.current)
                    )
                });
                format!("history diverged at message {index}/{current_message_count}{shape}")
            }
            None if axes.messages_truncated => format!(
                "history truncated ({} -> {current_message_count} messages, shared prefix intact)",
                counts.previous
            ),
            None => "append-only, no earlier message changed".to_string(),
        };
        reasons.push(format!("message payload changed ({detail})"));
    }
    reasons
}

/// `model` is the wire model id of the request being recorded — the one thing
/// the tracked state cannot supply (it keeps only a hash of the model, so a
/// break row could name the axis that moved but never the model it moved on).
/// The event for a read drop on a PURE tail append (all invariants of that
/// branch hold by construction: only the messages axis changed, nothing
/// truncated or diverged, no tool diff). TTL when the gap explains it;
/// unexpected provider-side loss otherwise.
fn pure_append_break_event(
    config: &PromptCacheConfig,
    previous: &TrackedPromptState,
    current: &TrackedPromptState,
    token_drop: u32,
    elapsed: u64,
    model: &str,
) -> CacheBreakEvent {
    let (unexpected, reason) = if elapsed > config.prompt_ttl.as_secs() {
        (
            false,
            format!(
                "cache reads dropped on a pure append — possible prompt cache TTL expiry after {elapsed}s"
            ),
        )
    } else {
        (
            true,
            "cache reads dropped on a pure append (prefix unchanged) — provider-side miss or eviction"
                .to_string(),
        )
    };
    CacheBreakEvent {
        unexpected,
        reason,
        previous_cache_read_input_tokens: previous.cache_read_input_tokens,
        current_cache_read_input_tokens: current.cache_read_input_tokens,
        token_drop,
        model_changed: false,
        system_changed: false,
        tools_changed: false,
        messages_changed: true,
        messages_truncated: false,
        elapsed_secs: elapsed,
        diverged_message: None,
        tools_added: Vec::new(),
        tools_removed: Vec::new(),
        model: model.to_string(),
        provider: provider_family_for_model(model).to_string(),
    }
}

fn detect_cache_break(
    config: &PromptCacheConfig,
    previous: Option<&TrackedPromptState>,
    current: &TrackedPromptState,
    first_divergence_index: Option<usize>,
    current_message_count: usize,
    model: &str,
) -> Option<CacheBreakEvent> {
    let previous = previous?;
    let elapsed = current
        .observed_at_unix_secs
        .saturating_sub(previous.observed_at_unix_secs);
    if previous.fingerprint_version != current.fingerprint_version {
        return Some(fingerprint_bump_break(previous, current, elapsed, model));
    }
    let token_drop = previous
        .cache_read_input_tokens
        .saturating_sub(current.cache_read_input_tokens);
    if token_drop < config.cache_break_min_drop {
        return None;
    }

    let model_changed = previous.model_hash != current.model_hash;
    let system_changed = previous.system_hash != current.system_hash;
    let tools_changed = previous.tools_hash != current.tools_hash;
    let messages_changed = previous.messages_hash != current.messages_hash;
    // `first_divergence` compares only the shared prefix, so a history that
    // SHRANK with its remaining prefix intact (compaction, rewind, message
    // elision — all legitimate large cache breaks) returns `None` exactly like
    // an ordinary tail append. Distinguish it by count, or every truncation
    // masquerades as "append-only" — the mislabel that sent the cold-rewrite
    // investigation toward a phantom provider-side miss.
    let messages_truncated =
        messages_changed && current_message_count < previous.message_hashes.len();

    // Name the tools that came and went. A tool set that oscillates within one
    // conversation strands the entire prefix on every flip (the definitions sit
    // in front of the messages), and the axis flag alone never said which tools
    // — so the fix could only be guessed at.
    let (tools_added, tools_removed) = if tools_changed {
        tool_name_diff(&previous.tool_names, &current.tool_names)
    } else {
        (Vec::new(), Vec::new())
    };
    let diverged_message = first_divergence_index.and_then(|index| {
        Some(DivergedWireMessage {
            index,
            previous: previous.message_shapes.get(index)?.clone(),
            current: current.message_shapes.get(index)?.clone(),
            shift: divergence_shift(&previous.message_hashes, &current.message_hashes, index)
                .map(str::to_string),
        })
    });

    // A PURE tail append cannot legitimately drop reads: the previous request
    // is a byte-identical prefix of this one, and the provider's cache keys
    // on content, so everything it read last time is still there to read.
    // Reads dropping here is provider-side (miss, eviction, TTL) — yet the
    // reason machinery below files it under "message payload changed
    // (append-only…)" with `unexpected: false`, because an append does change
    // the aggregate messages hash. That misclassification buried 84M dropped
    // tokens across 577 ledger rows as self-inflicted-and-expected — the
    // wire-reminder silence pattern all over again. Classify it with the
    // fingerprint-stable case instead: TTL when the gap explains it,
    // unexpected otherwise.
    let pure_append = messages_changed
        && !messages_truncated
        && first_divergence_index.is_none()
        && current_message_count > previous.message_hashes.len()
        && !model_changed
        && !system_changed
        && !tools_changed;
    if pure_append {
        return Some(pure_append_break_event(
            config, previous, current, token_drop, elapsed, model,
        ));
    }

    let reasons = break_reasons(
        BreakAxes {
            model_changed,
            system_changed,
            tools_changed,
            messages_changed,
            messages_truncated,
        },
        &tools_added,
        &tools_removed,
        diverged_message.as_ref(),
        MessageCounts {
            previous: previous.message_hashes.len(),
            current: current_message_count,
        },
        first_divergence_index,
    );

    let (unexpected, reason) = if reasons.is_empty() {
        if elapsed > config.prompt_ttl.as_secs() {
            (
                false,
                format!("possible prompt cache TTL expiry after {elapsed}s"),
            )
        } else {
            (
                true,
                "cache read tokens dropped while prompt fingerprint remained stable".to_string(),
            )
        }
    } else {
        (false, reasons.join(", "))
    };

    Some(CacheBreakEvent {
        unexpected,
        reason,
        previous_cache_read_input_tokens: previous.cache_read_input_tokens,
        current_cache_read_input_tokens: current.cache_read_input_tokens,
        token_drop,
        model_changed,
        system_changed,
        tools_changed,
        messages_changed,
        messages_truncated,
        elapsed_secs: elapsed,
        diverged_message,
        tools_added,
        tools_removed,
        model: model.to_string(),
        provider: provider_family_for_model(model).to_string(),
    })
}

/// Names present in exactly one of the two advertised tool lists, as
/// `(added, removed)`. Set semantics on purpose: a pure reordering yields two
/// empty vectors, which is itself the finding (the names are the same, so
/// something else about the definitions moved).
fn tool_name_diff(previous: &[String], current: &[String]) -> (Vec<String>, Vec<String>) {
    let before: std::collections::BTreeSet<&str> =
        previous.iter().map(String::as_str).collect();
    let after: std::collections::BTreeSet<&str> = current.iter().map(String::as_str).collect();
    let added = after
        .difference(&before)
        .map(|name| (*name).to_string())
        .collect();
    let removed = before
        .difference(&after)
        .map(|name| (*name).to_string())
        .collect();
    (added, removed)
}

/// `" (-Agent, -Workflow)"` / `" (+ToolSearch)"` / `" (same names; order or
/// definition changed)"` — the parenthetical the break reason appends.
fn format_tool_diff(added: &[String], removed: &[String]) -> String {
    if added.is_empty() && removed.is_empty() {
        return " (same names; order or definition changed)".to_string();
    }
    let names: Vec<String> = removed
        .iter()
        .map(|name| format!("-{name}"))
        .chain(added.iter().map(|name| format!("+{name}")))
        .collect();
    format!(" ({})", names.join(", "))
}

/// Ratio-based cache-efficiency streak tracker (spec item B). Updates
/// `stats.low_cache_hit_streak` / `stats.total_low_cache_hit_requests` every
/// call, and returns `Some(message)` only on the edge transition where the
/// streak first reaches [`LOW_CACHE_HIT_STREAK_WARNING_THRESHOLD`] — not on
/// every request past it, so a long degraded stretch produces one warning
/// instead of spamming one per turn, and a recovery-then-relapse produces a
/// fresh warning rather than staying permanently silent.
fn record_low_cache_hit_streak(
    stats: &mut PromptCacheStats,
    usage: &Usage,
    first_divergence_index: Option<usize>,
    current_message_count: usize,
) -> Option<String> {
    let cache_read = u64::from(usage.cache_read_input_tokens);
    let rebilled = u64::from(usage.input_tokens) + u64::from(usage.cache_creation_input_tokens);
    let denom = (cache_read + rebilled).max(1);
    // ratio = cache_read / denom < 0.2  <=>  cache_read * 5 < denom (integer
    // comparison — avoids floating point for a value that only ever gates a
    // streak counter).
    let low_ratio = cache_read.saturating_mul(5) < denom;
    let low_hit_request = low_ratio && rebilled > LOW_CACHE_HIT_VOLUME_FLOOR;

    if !low_hit_request {
        stats.low_cache_hit_streak = 0;
        stats.low_cache_hit_streak_tokens = 0;
        return None;
    }

    stats.low_cache_hit_streak = stats.low_cache_hit_streak.saturating_add(1);
    stats.total_low_cache_hit_requests = stats.total_low_cache_hit_requests.saturating_add(1);
    stats.low_cache_hit_streak_tokens = stats.low_cache_hit_streak_tokens.saturating_add(rebilled);

    if stats.low_cache_hit_streak != LOW_CACHE_HIT_STREAK_WARNING_THRESHOLD {
        return None;
    }

    Some(format_low_cache_hit_warning(
        stats.low_cache_hit_streak,
        stats.low_cache_hit_streak_tokens,
        first_divergence_index,
        current_message_count,
    ))
}

fn format_low_cache_hit_warning(
    streak: u32,
    rebilled_tokens: u64,
    first_divergence_index: Option<usize>,
    current_message_count: usize,
) -> String {
    let tokens_k = rebilled_tokens / 1_000;
    match first_divergence_index {
        Some(index) => format!(
            "prompt cache degraded: {streak} consecutive requests re-billed ~{tokens_k}k tokens (history diverges at message #{index}/{current_message_count})"
        ),
        None => format!(
            "prompt cache degraded: {streak} consecutive requests re-billed ~{tokens_k}k tokens"
        ),
    }
}

/// Adopt a peer writer's persisted record before this request is added to it.
///
/// One session has more than one `PromptCache`. The Anthropic client holds one
/// for the whole session (`build_provider_client`), and every non-Anthropic
/// response builds a fresh one for the same session id
/// (`runtime::record_non_anthropic_prompt_cache_usage`). Both keep a full
/// in-memory copy of [`PromptCacheStats`] and [`persist_state`] writes the
/// whole struct, so without this the long-lived writer's next request
/// overwrote everything the short-lived one had counted — silently, because
/// the file was only ever written and never re-read.
///
/// That is not a cosmetic loss: `tracked_requests` is the denominator every
/// per-request figure in `docs/analysis/` divides by, and the same overwrite
/// took the token totals and the `breaks.jsonl` ordinal with it. Measured in
/// the live store before the fix, sessions that mixed providers held 37% of
/// the requests they had actually made
/// (`docs/analysis/cache-miss-anatomy-r29.md` §1).
///
/// The two writers share the file, not the process, so the file is the only
/// place they can meet. `tracked_requests` orders the two records: whoever
/// counted more requests holds the later one, and its token totals came with
/// it. The caller overwrites the `last_*` fields for this request regardless.
///
/// Cost is one read of a small file this function is about to write anyway.
fn adopt_peer_stats(inner: &mut PromptCacheInner) {
    if let Some(persisted) = read_json::<PromptCacheStats>(&inner.paths.stats_path) {
        if persisted.tracked_requests > inner.stats.tracked_requests {
            inner.stats = persisted;
        }
    }
}

/// Withdraw this session's pending trim credit and bank it in `stats`.
///
/// Up front, before anything else this request does: the trim fired before
/// THIS request regardless of whether a break row gets written below, and the
/// stats pair must count such firings too.
fn withdraw_trim_credit(inner: &mut PromptCacheInner) -> u64 {
    let trimmed_tokens = take_pending_context_trim(&inner.config.session_id);
    if trimmed_tokens > 0 {
        inner.stats.context_trims_noted += 1;
        inner.stats.context_trim_tokens_noted += trimmed_tokens;
    }
    trimmed_tokens
}

fn apply_usage_to_stats(
    stats: &mut PromptCacheStats,
    usage: &Usage,
    request_hash: &str,
    source: &str,
) {
    stats.total_cache_creation_input_tokens += u64::from(usage.cache_creation_input_tokens);
    stats.total_cache_read_input_tokens += u64::from(usage.cache_read_input_tokens);
    stats.total_input_tokens += u64::from(usage.input_tokens);
    stats.last_cache_creation_input_tokens = Some(usage.cache_creation_input_tokens);
    stats.last_cache_read_input_tokens = Some(usage.cache_read_input_tokens);
    stats.last_request_hash = Some(request_hash.to_string());
    stats.last_cache_source = Some(source.to_string());
}

fn persist_state(inner: &PromptCacheInner) {
    let _ = ensure_cache_dirs(&inner.paths);
    let _ = write_json(&inner.paths.stats_path, &inner.stats);
    if let Some(previous) = &inner.previous {
        let _ = write_json(&inner.paths.session_state_path, previous);
    }
}

fn write_completion_entry(
    paths: &PromptCachePaths,
    request_hash: &str,
    response: &MessageResponse,
) {
    let _ = ensure_cache_dirs(paths);
    // The completion store is created here, by its writer, and nowhere else.
    let _ = ensure_private_dir(&paths.completion_dir);
    let entry = CompletionCacheEntry {
        cached_at_unix_secs: now_unix_secs(),
        fingerprint_version: current_fingerprint_version(),
        response: response.clone(),
    };
    let _ = write_json(&paths.completion_entry_path(request_hash), &entry);
}

/// Create the directories every session needs. NOT the completion store: that
/// one is created by its only writer, so an empty `completions/` never appears
/// for a session that stored no completion.
///
/// It used to be created here, unconditionally, per session — and since the
/// live turn loop is streaming while the completion store is written only from
/// the non-streaming `send_message`, the result was 548 directories on this
/// machine, every one of them empty. A directory that exists only to be empty
/// makes a dead feature look provisioned.
fn ensure_cache_dirs(paths: &PromptCachePaths) -> std::io::Result<()> {
    ensure_private_dir(&paths.root)?;
    ensure_private_dir(&paths.session_dir)
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // Windows directory junctions are reparse points even when
        // `FileType::is_symlink` is false. Walk from the volume root with
        // retained no-follow handles, then apply the protected owner DACL.
        core_types::paths::ensure_windows_owner_only_dir_no_follow(path)
    }

    #[cfg(not(windows))]
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {},
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("prompt cache directory is not a directory: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The cache root lives two levels below the config home
            // (`<home>/cache/prompt-cache`), so the parent chain may not exist
            // yet. Create the ancestors best-effort with `create_dir_all` — we
            // deliberately do NOT tighten their permissions here: an ancestor
            // may be a shared, pre-existing directory (the config home, a temp
            // root) that this process does not own, and chmod-ing those would
            // fail with `EPERM`. Only the leaf cache directories (created here
            // and restricted below) are ours to make owner-only.
            if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                fs::create_dir_all(parent)?;
            }
            match fs::create_dir(path) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return ensure_private_dir(path);
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }
    #[cfg(not(windows))]
    core_types::paths::restrict_permissions_owner_only(path)
}

/// How long an idle per-session cache directory is retained before the sweep
/// removes it. Activity is judged by `stats.json` mtime (rewritten on every
/// tracked request), falling back to the directory's own mtime.
pub const PROMPT_CACHE_RETENTION_DAYS: u64 = 30;

/// Emergency cap on retained session directories: after the age sweep, the
/// oldest directories beyond this count are removed too. Sized as a backstop
/// well above real accumulation (measured: ~2,700 dirs over 41 days with
/// sub-agent fan-out) so the 30-day retention is what normally converges the
/// store — an earlier 512 cap did the exact opposite, deleting everything
/// but ~13 days of history on its first run.
pub const PROMPT_CACHE_MAX_SESSION_DIRS: usize = 8192;

/// Directories whose last activity is within this window are NEVER removed by
/// the count-cap branch (the age branch cannot reach them by definition). A
/// briefly-idle live session must not be collateral of a fan-out burst that
/// pushes the store past the cap.
pub const PROMPT_CACHE_CAP_TRIM_MIN_IDLE_DAYS: u64 = 7;

/// Root-level marker gating the sweep to roughly once per day; its mtime is
/// the last sweep time and its body records the last outcome
/// ([`SweepMarker`]) so `zo doctor` can attest what the janitor actually did
/// — a silent `let _ =` sweep left 2,194 deletions with zero written
/// evidence in adversarial review.
const SWEEP_MARKER_FILE: &str = ".last-sweep";

/// Rename-prefix applied to a directory before its recursive delete, so a
/// sweep interrupted mid-`remove_dir_all` leaves a clearly-marked husk that
/// the next sweep unconditionally removes — without it the half-deleted
/// directory's own mtime (bumped by the child deletions) made it look
/// freshly active and it squatted forever.
const SWEEPING_PREFIX: &str = ".sweeping-";

/// Persisted body of `SWEEP_MARKER_FILE`: when the last sweep ran and what
/// it did. `swept_at_unix_nanos` doubles as a content nonce so tests can
/// assert "the gate did not rewrite the marker" by bytes, not by mtime
/// (1-second-resolution filesystems can't distinguish a rewrite by mtime).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepMarker {
    pub swept_at_unix_nanos: u128,
    pub removed: usize,
    pub kept: usize,
}

/// Read the sweep marker's recorded outcome, if the body parses. A marker
/// from the touch-phase (empty) or an older build reads as `None`.
#[must_use]
pub fn read_sweep_marker(root: &Path) -> Option<SweepMarker> {
    let bytes = fs::read(root.join(SWEEP_MARKER_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Spawn the stale-session sweep on a background thread when the daily marker
/// says one is due. The marker is touched BEFORE spawning (losing the race
/// means an extra idempotent sweep; touching after would let every startup in
/// the window spawn) and rewritten AFTER the sweep with the outcome, so the
/// many fresh-instance constructions on the non-Anthropic path (one per
/// request) cost one `symlink_metadata` stat here and nothing more.
fn maybe_spawn_stale_session_sweep(paths: &PromptCachePaths) {
    let marker = paths.root.join(SWEEP_MARKER_FILE);
    if arm_sweep_marker(&marker) != SweepMarkerArming::Armed {
        return;
    }
    let root = paths.root.clone();
    let current_session_dir = paths.session_dir.clone();
    std::thread::spawn(move || {
        let outcome = sweep_stale_session_dirs(
            &root,
            &current_session_dir,
            SystemTime::now(),
            PROMPT_CACHE_MAX_SESSION_DIRS,
        );
        // Attest what happened (best-effort): the marker body is the only
        // durable record of the janitor's actions.
        if let Ok(outcome) = outcome {
            let marker_body = SweepMarker {
                swept_at_unix_nanos: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0),
                removed: outcome.removed,
                kept: outcome.kept,
            };
            if let Ok(json) = serde_json::to_vec(&marker_body) {
                let _ = core_types::paths::write_private_file(
                    &marker,
                    &json,
                    &core_types::paths::ParentDirPolicy::LeaveParent,
                );
            }
        }
    });
}

/// Outcome of [`arm_sweep_marker`]: whether this process won the right to run
/// today's sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepMarkerArming {
    /// Marker was missing or stale — this process re-armed it and must sweep.
    Armed,
    /// Someone swept within the last day (or the marker path is unusable) —
    /// do nothing.
    Declined,
}

/// Arm the daily sweep gate. A stale marker is re-armed by bumping its mtime
/// ONLY — truncating here erased the previous attestation, and a process
/// exiting before its detached sweep thread finished then left a permanent
/// zero-byte marker with a fresh mtime: no outcome on record and no retry
/// for 24 hours. A missing marker is created empty (there is no attestation
/// to preserve yet); anything that is not a regular file (a planted symlink
/// or directory) is refused outright.
fn arm_sweep_marker(marker: &Path) -> SweepMarkerArming {
    if let Ok(metadata) = fs::symlink_metadata(marker) {
        if !metadata.file_type().is_file() {
            return SweepMarkerArming::Declined;
        }
        let fresh = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed < Duration::from_secs(24 * 60 * 60));
        if fresh {
            return SweepMarkerArming::Declined;
        }
        let rearmed = fs::OpenOptions::new()
            .write(true)
            .open(marker)
            .and_then(|file| file.set_modified(SystemTime::now()));
        if rearmed.is_err() {
            return SweepMarkerArming::Declined;
        }
    } else if core_types::paths::write_private_file(
        marker,
        b"",
        &core_types::paths::ParentDirPolicy::LeaveParent,
    )
    .is_err()
    {
        // No root yet (first session on this machine — created by the first
        // persist) or unwritable — skip; the next construction sweeps.
        return SweepMarkerArming::Declined;
    }
    SweepMarkerArming::Armed
}

/// What one sweep pass did: directories removed vs. left in place. `kept`
/// counts every surviving live session directory INCLUDING the calling
/// session's own, so it matches what a subsequent store listing (e.g. the
/// doctor's directory count) sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SweepOutcome {
    removed: usize,
    kept: usize,
}

/// Rename-then-delete: move the directory to a `.sweeping-` husk name first
/// (atomic), then delete the husk. An interrupted delete leaves a husk the
/// next sweep removes unconditionally instead of a half-empty directory
/// whose bumped mtime reads as fresh activity. Falls back to a direct
/// delete when the rename fails (e.g. a name collision).
fn remove_session_dir(root: &Path, path: &Path) -> bool {
    let husk_name = format!(
        "{SWEEPING_PREFIX}{}",
        path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    );
    let husk = root.join(husk_name);
    let target = if fs::rename(path, &husk).is_ok() { husk } else { path.to_path_buf() };
    fs::remove_dir_all(&target).is_ok()
}

/// Remove idle per-session cache directories under `root`, in one directory
/// pass (entries handled in `read_dir` order): leftover `.sweeping-` husks
/// unconditionally, everything whose last activity (`stats.json` mtime, else
/// directory mtime) predates the retention window, then — as an emergency
/// backstop over the survivors — the oldest beyond `max_session_dirs`,
/// EXCEPT directories active within
/// [`PROMPT_CACHE_CAP_TRIM_MIN_IDLE_DAYS`], which the cap may never touch.
/// The calling session's own directory (compared case-insensitively by name:
/// APFS default volumes are case-insensitive, and `--session-id MyRun` vs
/// `myrun` land in one directory) and anything that is not a plain directory
/// (symlinks included — `remove_dir_all` must never follow a planted link)
/// are left untouched. Best-effort throughout: a single unremovable entry
/// never aborts the sweep.
///
/// Known race, accepted: a session idle past the retention window that
/// resumes at the exact moment of a sweep can lose its cache-diagnostic
/// state (stats counters, break ledger, divergence baseline — rebuilt from
/// scratch by the next request; conversation state lives elsewhere and is
/// unaffected).
fn sweep_stale_session_dirs(
    root: &Path,
    current_session_dir: &Path,
    now: SystemTime,
    max_session_dirs: usize,
) -> std::io::Result<SweepOutcome> {
    let cutoff = now - Duration::from_secs(PROMPT_CACHE_RETENTION_DAYS * 24 * 60 * 60);
    let cap_guard =
        now - Duration::from_secs(PROMPT_CACHE_CAP_TRIM_MIN_IDLE_DAYS * 24 * 60 * 60);
    let current_name = current_session_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase());
    let mut survivors: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut removed = 0usize;
    // The calling session's directory never enters `survivors` (it must not
    // be sort-fodder for the cap trim) but it IS a kept live directory.
    let mut kept_current = 0usize;
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let entry_name = entry.file_name().to_string_lossy().to_lowercase();
        if Some(&entry_name) == current_name.as_ref() {
            kept_current = 1;
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue; // files (the sweep marker) and symlinks stay
        }
        if entry_name.starts_with(SWEEPING_PREFIX) {
            // Husk from an interrupted earlier sweep — finish the job.
            if fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
            continue;
        }
        let activity = fs::metadata(path.join("stats.json"))
            .and_then(|stats| stats.modified())
            .or_else(|_| metadata.modified())
            .unwrap_or(now);
        if activity < cutoff {
            if remove_session_dir(root, &path) {
                removed += 1;
            }
            continue;
        }
        survivors.push((activity, path));
    }
    let mut kept = survivors.len() + kept_current;
    if survivors.len() > max_session_dirs {
        survivors.sort_by_key(|(activity, _)| *activity);
        let excess = survivors.len() - max_session_dirs;
        for (activity, path) in survivors.into_iter().take(excess) {
            // The cap is an emergency backstop against unbounded growth, not
            // a recency contest: anything active in the last week stays even
            // when the store is over the cap (a fan-out burst must not evict
            // a briefly-idle live session).
            if activity >= cap_guard {
                continue;
            }
            if remove_session_dir(root, &path) {
                removed += 1;
                kept -= 1;
            }
        }
    }
    Ok(SweepOutcome { removed, kept })
}

/// Cache-health snapshot for `zo doctor`: the most recently active session's
/// stats and break ledger, plus store-wide retention facts.
#[derive(Debug, Clone)]
pub struct PromptCacheDoctorSummary {
    /// Sanitized directory name of the most recently active session.
    pub session_dir_name: String,
    pub stats: PromptCacheStats,
    pub breaks: Vec<CacheBreakLedgerRow>,
    /// Session directories currently in the store.
    pub store_session_dirs: usize,
    /// Age in days of the least recently active session directory.
    pub store_oldest_days: Option<u64>,
    /// The janitor's last attested outcome, if a sweep has completed on this
    /// machine. `None` also covers a marker still in its touch-phase.
    pub last_sweep: Option<SweepMarker>,
}

/// Read-only doctor probe over the prompt-cache store: locate the most
/// recently active session (by `stats.json` mtime), load its stats and break
/// ledger, and count store directories. `None` when the store does not exist
/// or holds no session with stats. Touches nothing — safe for `--check`.
#[must_use]
pub fn doctor_cache_summary() -> Option<PromptCacheDoctorSummary> {
    let root = base_cache_root();
    let entries = fs::read_dir(&root).ok()?;
    let now = SystemTime::now();
    let mut store_session_dirs = 0usize;
    let mut oldest: Option<SystemTime> = None;
    let mut latest: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with(SWEEPING_PREFIX) {
            // A husk mid-delete is not a live session, and its mtime (bumped
            // by the child deletions) would drag `store_oldest_days` younger.
            continue;
        }
        store_session_dirs += 1;
        let activity = fs::metadata(path.join("stats.json"))
            .and_then(|stats| stats.modified())
            .or_else(|_| metadata.modified())
            .unwrap_or(now);
        if oldest.is_none_or(|current| activity < current) {
            oldest = Some(activity);
        }
        if path.join("stats.json").exists()
            && latest.as_ref().is_none_or(|(current, _)| activity > *current)
        {
            latest = Some((activity, path));
        }
    }
    let (_, session_dir) = latest?;
    // A torn/unparseable stats.json (the writer is a non-atomic truncate)
    // must not erase the store-wide findings with it — degrade to zeroed
    // session stats (rendered as "no cache telemetry") instead of `None`.
    let stats =
        read_json::<PromptCacheStats>(&session_dir.join("stats.json")).unwrap_or_default();
    Some(PromptCacheDoctorSummary {
        session_dir_name: session_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        stats,
        breaks: read_break_ledger(&session_dir.join("breaks.jsonl")),
        store_session_dirs,
        store_oldest_days: oldest.and_then(|activity| {
            now.duration_since(activity).ok().map(|age| age.as_secs() / 86_400)
        }),
        last_sweep: read_sweep_marker(&root),
    })
}

/// One line of the per-request ledger (`requests.jsonl`): what one recorded
/// provider response cost, when it happened, and under which cache policy.
///
/// Written for EVERY request, unlike [`CacheBreakLedgerRow`] — that is the
/// whole point. A ledger of breaks answers "what went wrong" and nothing else;
/// three separate measurement rounds needed "what does an ordinary request
/// cost" and had to reconstruct it from session-level totals divided by
/// `tracked_requests`, which cannot separate a period, a provider, or a TTL
/// policy inside one session. Worse, the gap distribution that priced the
/// 5-minute conversation TTL had to be imported wholesale from a Claude Code
/// corpus, because the only per-request timestamps zo owned were on broken
/// requests — a sample that skews long by construction
/// (`docs/analysis/cache-ttl-verdict-r21.md` §7).
///
/// Deliberately narrow. Every column here is a term in the per-request cost
/// identity (`cache_creation` × write multiplier + `cache_read` × 0.1 +
/// `input_uncached` × 1.0), a key to join on (`seq` → `breaks.jsonl`), or the
/// axis a period/provider/TTL split needs. Anything reconstructible from those
/// was left out: this file grows by one row per request forever, so a column
/// that is merely interesting costs ~24,700 copies of itself a month.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestLedgerRow {
    /// 1-based request ordinal within this session — `stats.tracked_requests`
    /// at record time, the same key [`CacheBreakLedgerRow::seq`] uses, so the
    /// two ledgers join on it. Carries that field's reset caveat: `stats.json`
    /// is a non-atomic overwrite, so order by `(seq, ts_unix_ms)`.
    pub seq: u64,
    /// Wall clock at record time, in **milliseconds** — not the seconds
    /// [`CacheBreakLedgerRow::ts_unix_secs`] uses. The question this ledger
    /// exists to answer is the inter-request gap distribution, whose measured
    /// median is single-digit seconds (4.7s on the proxy corpus); at
    /// second resolution a large share of consecutive gaps would round to 0
    /// or 1 and the `<=60s` bucket — the one that decided the TTL — would be
    /// unreadable from inside itself.
    pub ts_unix_ms: u64,
    /// Wire model id, verbatim, and its provider family — the same pair and
    /// the same derivation as [`CacheBreakLedgerRow::model`] /
    /// [`CacheBreakLedgerRow::provider`], for the same reason: cache-write
    /// pricing is provider-specific and one cache root holds every provider a
    /// machine touched. Previously this could only be recovered by inferring
    /// the provider from arithmetic artifacts of the numbers themselves.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    pub cache_creation: u32,
    pub cache_read: u32,
    /// `usage.input_tokens` — the fully re-billed remainder no cache covered.
    pub input_uncached: u32,
    /// `usage.output_tokens` — the generated half of the bill.
    ///
    /// The other three token columns are the input side of the cost identity,
    /// which is what this ledger was opened to explain. Output is here because
    /// the attempt join needs the WHOLE bill from one file: an attempt's
    /// output tokens otherwise exist only on a spawn's route-outcome record
    /// and in an agent manifest's `usageCost`, so a main-session turn — which
    /// has neither — could not be priced from disk at all
    /// (`docs/design/zo-attempt-key-contract-20260915.md` §2.2). It is read
    /// off the same `usage` the row's other counts come from, so it costs no
    /// threading and no second reader of the same fact.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub output: u32,
    pub message_count: usize,
    /// Whether this request also wrote a [`CacheBreakLedgerRow`]. Lets a
    /// break rate be computed against a real denominator without joining the
    /// two files, and makes a join that finds no partner a detectable defect
    /// rather than a silent one.
    pub broke: bool,
    /// The TTL the conversation breakpoints on THIS request asked for:
    /// `"5m"`, `"1h"`, `"5m+1h"` when the markers disagree, empty when the
    /// request carried no message-level marker at all (the non-Anthropic
    /// encoders never serialize one).
    ///
    /// Read straight off the request's own `cache_control` blocks — the value
    /// `runtime::convert_messages::mark_conversation_cache_breakpoints` chose
    /// is physically present in the `MessageRequest` this function is handed,
    /// so nothing had to be threaded through to record it. It is the column
    /// that makes a before/after comparison of a TTL change possible from the
    /// ledger alone, instead of from a deploy timestamp the data does not
    /// carry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ttl: String,
    /// Every `cache_control` marker this request carried: where it sat, which
    /// slot of the placement policy put it there, the wire role under it and
    /// the lifetime it asked for. See [`MarkerRecord`] for the compact wire
    /// form.
    ///
    /// This is the column r30 exists for. The same two facts were already
    /// recorded — as `CacheBreakLedgerRow::cache_breakpoints` and
    /// `marker_roles` — but on the WRONG file: a break row is written only
    /// when reads drop by at least `DEFAULT_BREAK_MIN_DROP`, so a request
    /// whose markers landed perfectly (the control group) and a run of
    /// failures after the first (which drop 0 → 0) both write nothing. Every
    /// break row since the fields landed carried them, 3 of 3 — the instrument
    /// was not conditional, its carrier was just almost never written. Measured
    /// on the live store the day this column was added: 3 marker records in
    /// 1,916 break rows, against 24,748 tracked requests
    /// (`docs/analysis/marker-instrument-r30.md` §1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<MarkerRecord>,
    /// How [`Self::markers`]' positions compare with the previous request's:
    /// `same` · `rolled` · `anchor_moved` · `added` · `removed`, empty when
    /// there is no basis. Defined by `marker_movement`.
    ///
    /// The positions alone cannot answer the question r29 §4 stopped at
    /// ("does zo lose reads because its markers moved?") without the reader
    /// re-deriving the comparison across rows — and the previous request's
    /// positions are already in hand here, on `TrackedPromptState`, at zero
    /// cost.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub moved: String,
    /// Tokens this request read FEWER than the previous one, 0 when reads held
    /// or grew.
    ///
    /// The same number as [`CacheBreakEvent::token_drop`] wherever a break row
    /// exists — pinned by `read_drop_agrees_with_the_break_rows_token_drop` —
    /// and the reason the column is here anyway is the drops below
    /// `DEFAULT_BREAK_MIN_DROP`, which no break row records. Without it a
    /// marker move that costs 1,900 tokens every request is invisible.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub read_drop: u32,
    /// The ATTEMPT this request was spent on — the one key that joins this
    /// ledger to `route-outcomes.jsonl` and `timings.jsonl`
    /// (`runtime::model_router::attempt`). `<sessionId>@<turnOrdinal>` for a
    /// main-session turn, `<agentId>#<runGeneration>` for a spawned agent's
    /// own requests. Opaque: every reader joins on equality and none parses
    /// it.
    ///
    /// Read from the process-wide slot [`note_attempt`] deposits, keyed by the
    /// prompt-cache session id — NOT from a field on this instance. The
    /// non-Anthropic recorder builds a FRESH [`PromptCache`] for every single
    /// request (`runtime::record_non_anthropic_prompt_cache_usage`), so an
    /// instance field would be empty on exactly the provider paths that write
    /// the most rows; and the slot costs one map lookup on a mutex this
    /// function already holds, where a per-request file read would not be
    /// affordable here at all. Empty when nothing deposited one (a bare
    /// harness, a request outside any turn).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attempt: String,
    /// The reasoning effort/thinking setting this request actually carried,
    /// in the catalog's own spelling (`EffortLevel::key`) or `budget:<tokens>`
    /// for a legacy thinking budget. Empty when the request asked for neither
    /// — including every provider that has no such concept.
    ///
    /// Same derivation discipline as [`Self::ttl`]: read straight off the
    /// `MessageRequest` in hand (`MessageRequest::reasoning_request`, the one
    /// existing provider-neutral projection), so nothing is threaded through
    /// to record it. It is the column that lets a cost-per-attempt join say
    /// which effort the tokens were spent at, instead of inferring it from a
    /// route record that only carries the router's RECOMMENDATION.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub effort: String,
}

/// `skip_serializing_if` for [`RequestLedgerRow::read_drop`]: most requests do
/// not drop, and 19 bytes on every row of a file kept forever is not free.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// One line of the per-request cache-break ledger (`breaks.jsonl`): which
/// request's cache reads DROPPED, on which fingerprint axis, and by how much.
/// `stats.json` only retains the LAST break's reason, so a session with
/// several breaks (the exact situation worth diagnosing) lost everything but
/// the final one.
///
/// Coverage contract — this is a ledger of break *transitions*, not of every
/// cold request: the session's first request (no previous to compare) and a
/// cache that STAYS cold (`token_drop` = 0) produce no row. Read it together
/// with `stats.json`'s `low_cache_hit_streak` counters, which do cover
/// sustained coldness. `seq` resets with `stats.json` (non-atomic overwrite;
/// a torn file re-zeroes `tracked_requests`), so treat `(seq, ts_unix_secs)`
/// as the ordering key, not `seq` alone.
///
/// Key signatures: `messages_truncated` = history shrank with its prefix
/// intact (compaction/rewind/elision — the common legitimate full-prefix
/// rewrite); all axis flags `false` with `unexpected: true` = our payload was
/// byte-stable yet reads dropped (provider-side miss / eviction).
///
/// `trimmed_tokens_estimate` is the trim-pricing column: the compaction
/// planner deposits each microcompact's cleared estimate through
/// [`note_context_trim`] (a session-keyed side channel — `runtime` depends on
/// this crate and cannot be called back into), and the same session's next
/// recorded request withdraws it here. Rows without a preceding trim carry
/// nothing.
// `Default` is the neutral row — no axis set, nothing lost. Aggregators and
// tests build a row by naming only the columns under discussion, which keeps
// an added column from rewriting every construction site.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
// Same rationale as `CacheBreakEvent`: the axis flags are independent
// fingerprint dimensions, not mutually exclusive states.
#[allow(clippy::struct_excessive_bools)]
pub struct CacheBreakLedgerRow {
    /// 1-based request ordinal within this session's tracking
    /// (`stats.tracked_requests` at record time; see the coverage contract
    /// above for its reset caveat).
    pub seq: u64,
    pub ts_unix_secs: u64,
    pub unexpected: bool,
    pub reason: String,
    pub model_changed: bool,
    pub system_changed: bool,
    pub tools_changed: bool,
    pub messages_changed: bool,
    /// History got shorter while the surviving prefix stayed identical —
    /// indistinguishable from an append by divergence index alone.
    #[serde(default)]
    pub messages_truncated: bool,
    pub first_divergence_index: Option<usize>,
    /// Leading messages byte-identical to the previous request.
    #[serde(default)]
    pub prefix_stable_messages: usize,
    pub prev_message_count: usize,
    pub message_count: usize,
    pub prev_cache_read: u32,
    pub cache_read: u32,
    pub cache_creation: u32,
    pub token_drop: u32,
    /// Seconds since the previous tracked request (TTL-expiry evidence).
    pub elapsed_secs: u64,
    /// What the diverging message looked like before and after — the field that
    /// makes a rewrite attributable without a re-investigation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diverged_message: Option<DivergedWireMessage>,
    /// Name-level tool diff for a `tools_changed` break; both empty means the
    /// names matched and the definitions themselves moved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_removed: Vec<String>,
    /// Wire model id this request was sent with, verbatim.
    ///
    /// WHY: a break's cost is provider-specific (cache-write premium, whether
    /// `cache_creation` is even reported), and the store holds rows from every
    /// provider a machine touched — foreground sessions and subagent scopes
    /// sit side by side under one root. Nothing in a row said which, so
    /// attributing cost meant inferring the provider from arithmetic
    /// artifacts. A real investigation did exactly that: it separated the
    /// populations by `cache_creation == 0` (the OpenAI-compat and Gemini
    /// paths hardcode it — see [`PromptCacheStats::total_input_tokens`]) and
    /// by `cache_read` being divisible by 128 (120/120 non-Anthropic rows
    /// were, 0/56 Anthropic rows were), and its headline median was quoted
    /// over the MIXED population before that split was found. Naming the
    /// model and family in the row ends that class of mistake.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Provider family for [`Self::model`], as
    /// [`crate::ProviderKind::rate_limit_key`] (`anthropic` / `openai` /
    /// `google` / `xai` / `ollama`) — the repo's existing stable telemetry
    /// namespace, deliberately not a second taxonomy invented here.
    ///
    /// Derived from the wire model id, which is all `PromptCache` is handed:
    /// a custom OpenAI-compatible provider serving a Claude-named model
    /// therefore reads as `anthropic`. Threading the routed `ProviderKind` in
    /// instead would add an argument to `record_usage`/`record_response` at
    /// every call site in `runtime`/`tools`/`zo-cli`, and the model id already
    /// answers the question the ledger gets asked (which family's cache
    /// semantics and pricing apply to this row). Empty when the id matches no
    /// known family, and empty on rows written by builds before this field —
    /// which is what `skip_serializing_if` is for: old rows still load, and a
    /// row costs no bytes for a field it cannot fill.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// Estimated tokens the microcompact(s) firing right before this request
    /// cleared — the price-of-trim pairing: `cache_creation` on this row is
    /// what the trim COST, this field is what it BOUGHT. `None` on rows not
    /// preceded by a trim, and on rows written by builds before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trimmed_tokens_estimate: Option<u64>,
    /// Message indices this request marked with `cache_control`, ascending,
    /// and the same for the previous request.
    ///
    /// A lost read tells you the prefix missed; these tell you WHERE the
    /// product asked the provider to look. `mark_breakpoints_with_ttl` writes
    /// an anchor on the previous request's tail plus one rolling marker, so a
    /// healthy pair reads as "my anchor is a position the last request already
    /// wrote". The 1,190 pure-append breaks that dominated this ledger were
    /// exactly the failure of that property — both markers had rolled onto
    /// content the request itself had just invented, so no position could ever
    /// be a hit — and nothing in a row said so; the cause took a
    /// re-investigation to find and would take another to confirm the fix.
    /// [`Self::anchor_was_previously_written`] is the one-call verdict.
    ///
    /// Empty on rows from builds before this field, which is what the two
    /// `Vec::is_empty` skips mean: an old row costs no bytes and simply
    /// answers `None`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_breakpoints: Vec<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prev_cache_breakpoints: Vec<usize>,
    /// Wire role at each [`Self::cache_breakpoints`] position, same order —
    /// what separates an anchor that landed on a previous request's tail from
    /// one that overshot onto content this request invented.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marker_roles: Vec<String>,
}

/// Cause of a break whose axis flags are all `false` (the request fingerprint
/// itself did not change). Lives next to `detect_cache_break` — the sole
/// producer of the `reason` strings it matches — so the classification and the
/// wording can only drift together, and the pairing is pinned by tests in this
/// file rather than by a `contains()` in a far-away consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoAxisBreakCause {
    /// Byte-stable request inside the TTL window, yet reads dropped — the
    /// only genuinely provider-side case (recorded as `unexpected`).
    ProviderSide,
    /// Our own fingerprint schema version changed (an upgrade, not a leak).
    FingerprintBump,
    /// The gap since the previous request exceeded the prompt TTL.
    TtlExpiry,
    /// A reason wording this build does not recognize (e.g. a row written by
    /// a newer binary).
    Unknown,
}

/// What a break row can say about the anchor breakpoint.
///
/// Exists so "no verdict" stops meaning two opposite things at once — see
/// [`CacheBreakLedgerRow::anchor_verdict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorVerdict {
    /// The anchor sat where the previous request had already written.
    Reused,
    /// No marker on this request pointed at previously written content.
    Missed,
    /// One side of the comparison carries no marker record — the row predates
    /// marker recording, or it is the first request after it started.
    Unrecorded,
    /// Both sides recorded, but this break had another cause (model, system
    /// prompt or tool block changed, or history diverged under the anchor), so
    /// no marker placement could have prevented it.
    Ineligible,
}

impl CacheBreakLedgerRow {
    /// Did this request's anchor breakpoint sit on a position the previous
    /// request had already written?
    ///
    /// `Some(true)` is the healthy shape the anchor+rolling fix produces:
    /// there was a cached entry at the anchor for the provider to find.
    /// `Some(false)` says every marker on this request pointed at content no
    /// earlier request had written — a guaranteed miss, and the defect that
    /// cost this ledger 149.9M tokens across 1,190 rows.
    ///
    /// `None` when the question does not apply, which is three cases:
    ///
    /// * either row carries no marker record (a pre-upgrade row, or a first
    ///   request);
    /// * the model, system prompt, or tool block changed. Those invalidate the
    ///   prefix wherever the markers sit, so the anchor neither caused the
    ///   break nor could have prevented it. Counting such a row as a "miss" is
    ///   what the FIRST row this ledger recorded did — a `/model` switch read
    ///   as 0% anchor reuse and made the metric say the fix had failed when
    ///   nothing about the fix was in play.
    #[must_use]
    pub fn anchor_was_previously_written(&self) -> Option<bool> {
        match self.anchor_verdict() {
            AnchorVerdict::Reused => Some(true),
            AnchorVerdict::Missed => Some(false),
            AnchorVerdict::Unrecorded | AnchorVerdict::Ineligible => None,
        }
    }

    /// [`Self::anchor_was_previously_written`] with its two `None`s told apart.
    ///
    /// "No verdict" hides two opposite states, and a reader who cannot tell
    /// them apart reads the wrong news:
    ///
    /// * `AnchorVerdict::Unrecorded` — one side of the comparison was never
    ///   written down, so the instrument has nothing to say yet. Genuinely
    ///   "waiting for data".
    /// * `AnchorVerdict::Ineligible` — both sides ARE recorded and the
    ///   instrument is working; this particular break simply had another cause.
    ///   A ledger that is all-ineligible is a ledger that is running, and that
    ///   is a different thing to know.
    ///
    /// Measured on the live store the day this split was added: 1,916 rows, of
    /// which 3 carried markers and all 3 were ineligible — and `zo --doctor`
    /// reported all 1,916 as "rows predate marker recording", which was untrue
    /// of three of them and would grow untruer with every eligible-looking row
    /// that arrived.
    #[must_use]
    pub fn anchor_verdict(&self) -> AnchorVerdict {
        let Some(&anchor) = self.cache_breakpoints.first() else {
            return AnchorVerdict::Unrecorded;
        };
        if self.prev_cache_breakpoints.is_empty() {
            return AnchorVerdict::Unrecorded;
        }
        if self.model_changed || self.system_changed || self.tools_changed {
            return AnchorVerdict::Ineligible;
        }
        // History that diverged AT or BEFORE the anchor takes the anchor's own
        // position with it: whatever was cached there is not what this request
        // sends, so no marker placement could have hit. Charging that to the
        // anchor is the same mistake as charging a model switch to it — and the
        // first eligible row this ledger recorded was exactly that, an anchor
        // at 27 against a divergence earlier in the history, read as 0% reuse.
        if self
            .first_divergence_index
            .is_some_and(|divergence| divergence <= anchor)
        {
            return AnchorVerdict::Ineligible;
        }
        if self.prev_cache_breakpoints.contains(&anchor) {
            AnchorVerdict::Reused
        } else {
            AnchorVerdict::Missed
        }
    }

    /// Whether this row's markers were recorded at all, whatever the verdict.
    /// The count of these is what "the instrument is alive" means.
    #[must_use]
    pub fn has_marker_record(&self) -> bool {
        !self.cache_breakpoints.is_empty()
    }

    /// Classify a no-axis break by its recorded cause; `None` when any axis
    /// flag is set (the axis flags themselves are the explanation).
    #[must_use]
    pub fn no_axis_cause(&self) -> Option<NoAxisBreakCause> {
        if self.model_changed || self.system_changed || self.tools_changed || self.messages_changed
        {
            return None;
        }
        Some(if self.unexpected {
            NoAxisBreakCause::ProviderSide
        } else if self.reason.contains("fingerprint version") {
            NoAxisBreakCause::FingerprintBump
        } else if self.reason.contains("TTL") {
            NoAxisBreakCause::TtlExpiry
        } else {
            NoAxisBreakCause::Unknown
        })
    }
}

/// Append one break row to the session's `breaks.jsonl`. Best-effort by
/// contract (the ledger is a diagnostic aid; a write failure must never
/// affect the request path). Size stays naturally bounded: at most one row
/// per tracked request, a few hundred bytes each, in a per-session directory.
/// The session directory already exists here: a break requires a previous
/// tracked request, whose `persist_state` created and restricted it. Writes
/// through the shared `append_private_file` so the ledger keeps the exact
/// symlink-rejecting `O_NOFOLLOW`/`0o600` policy of every other cache file
/// (a raw `OpenOptions::append` here followed a planted symlink and chmod-ed
/// its target — caught in adversarial review).
fn append_break_row(paths: &PromptCachePaths, row: &CacheBreakLedgerRow) {
    let Ok(mut line) = serde_json::to_string(row) else {
        return;
    };
    line.push('\n');
    let _ = core_types::paths::append_private_file(&paths.breaks_path, line.as_bytes());
}

/// Read a break ledger (`breaks.jsonl`), oldest first. Lossy: unparseable
/// lines are skipped, a missing file reads as empty.
#[must_use]
pub fn read_break_ledger(breaks_path: &Path) -> Vec<CacheBreakLedgerRow> {
    read_jsonl(breaks_path)
}

/// The three marker columns, built where both this request's markers and the
/// previous request's positions are in hand.
///
/// A struct rather than three more parameters on [`request_ledger_row`]: that
/// function is already at five, and the three belong together — they are one
/// reading of one request's markers, and `moved` is meaningless without the
/// positions `markers` carries.
struct MarkerColumns {
    markers: Vec<MarkerRecord>,
    moved: &'static str,
    read_drop: u32,
}

impl MarkerColumns {
    /// Read this request's markers against the previous request's state.
    fn new(
        fingerprints: &RequestFingerprints,
        previous: Option<&TrackedPromptState>,
        usage: &Usage,
    ) -> Self {
        Self {
            markers: fingerprints.markers.clone(),
            moved: marker_movement(
                previous.map(|state| state.cache_breakpoints.as_slice()),
                &fingerprints.cache_breakpoints,
            ),
            // The same subtraction `detect_cache_break` makes, deliberately
            // repeated rather than lifted off the event: the event exists only
            // above the break floor, and this column's whole point is the
            // drops below it.
            read_drop: previous.map_or(0, |state| {
                state
                    .cache_read_input_tokens
                    .saturating_sub(usage.cache_read_input_tokens)
            }),
        }
    }
}

/// Build one [`RequestLedgerRow`] from the values `record_usage_internal`
/// already holds. Split out of that function only to keep it under the
/// line lint; every field is copied, none is derived here except the two the
/// row's own docs explain (`provider` from the model id, `ttl` from the
/// request's own markers).
fn request_ledger_row(
    seq: u64,
    session_id: &str,
    request: &MessageRequest,
    usage: &Usage,
    message_count: usize,
    broke: bool,
    markers: MarkerColumns,
) -> RequestLedgerRow {
    RequestLedgerRow {
        seq,
        ts_unix_ms: now_unix_ms(),
        model: request.model.clone(),
        provider: provider_family_for_model(&request.model).to_string(),
        cache_creation: usage.cache_creation_input_tokens,
        cache_read: usage.cache_read_input_tokens,
        input_uncached: usage.input_tokens,
        output: usage.output_tokens,
        message_count,
        broke,
        ttl: conversation_marker_ttl(&request.messages),
        markers: markers.markers,
        moved: markers.moved.to_string(),
        read_drop: markers.read_drop,
        attempt: attempt_for_session(session_id),
        effort: request_effort_label(request),
    }
}

/// The effort/thinking this request carried, spelled the one way the catalog
/// spells it ([`EffortLevel::key`]) — never a second table of level names.
///
/// [`MessageRequest::reasoning_request`] is the existing provider-neutral
/// projection of the two fields a caller can set, so this adds no third
/// reading of the same request. A pre-populated Anthropic `output_config`
/// (the wire block, set without the neutral field) is the one thing that
/// projection does not look at, so it is the fallback here rather than a
/// parallel rule.
fn request_effort_label(request: &MessageRequest) -> String {
    match request.reasoning_request() {
        crate::types::ReasoningRequest::Effort(level) => level.key().to_string(),
        crate::types::ReasoningRequest::BudgetTokens(budget) => format!("budget:{budget}"),
        crate::types::ReasoningRequest::Auto => request
            .output_config
            .as_ref()
            .map(|config| config.effort.key().to_string())
            .unwrap_or_default(),
    }
}

/// Append one row, best-effort and silent.
///
/// Must be called AFTER `persist_state`, which is what creates the session
/// directory. Unlike [`append_break_row`] — which cannot fire before a second
/// request has been compared against a first, by which time the directory
/// exists — this runs on the session's FIRST request too, and a silent
/// appender against a missing directory would simply lose row 1 of every
/// session forever.
fn append_request_row(paths: &PromptCachePaths, row: &RequestLedgerRow) {
    let Ok(mut line) = serde_json::to_string(row) else {
        return;
    };
    line.push('\n');
    let _ = core_types::paths::append_private_file(&paths.requests_path, line.as_bytes());
}

/// Read a request ledger (`requests.jsonl`), oldest first. Lossy in the same
/// way as [`read_break_ledger`]: unparseable lines are skipped and a missing
/// file reads as empty — which is what every session recorded before this
/// ledger existed looks like.
#[must_use]
pub fn read_request_ledger(requests_path: &Path) -> Vec<RequestLedgerRow> {
    read_jsonl(requests_path)
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    // The cache files are as sensitive as credentials (they hold prompt text),
    // so they use the same owner-only, symlink-rejecting, creation-time-`0o600`
    // write policy — reuse the single shared implementation rather than keeping
    // a second copy here. `LeaveParent` preserves the prompt cache's existing
    // directory semantics: `ensure_private_dir` already created and restricted
    // the leaf cache dirs, and their ancestors may be shared, pre-existing
    // directories this process does not own (chmod-ing those would `EPERM`), so
    // the writer must not touch the parent.
    core_types::paths::write_private_file(
        path,
        &json,
        &core_types::paths::ParentDirPolicy::LeaveParent,
    )
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let primary_root = base_cache_root();
    let relative = path.strip_prefix(&primary_root).ok()?;
    // Cache snapshots contain cumulative counters and a single latest prompt;
    // they cannot be merged safely, so use the first valid high-to-low copy.
    // A later persist writes that selected state to the primary root.
    for root in cache_roots() {
        let Ok(bytes) = fs::read(root.join(relative)) else {
            continue;
        };
        if let Ok(value) = serde_json::from_slice(&bytes) {
            return Some(value);
        }
    }
    None
}

fn request_hash_hex(request: &MessageRequest) -> String {
    format!(
        "{REQUEST_FINGERPRINT_PREFIX}-{:016x}",
        hash_serializable(request)
    )
}

fn hash_serializable<T: Serialize>(value: &T) -> u64 {
    let json = serde_json::to_vec(value).unwrap_or_default();
    stable_hash_bytes(&json)
}

fn sanitize_path_segment(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return sanitized;
    }
    let suffix = format!("-{:x}", hash_string(value));
    format!(
        "{}{}",
        &sanitized[..MAX_SANITIZED_LENGTH.saturating_sub(suffix.len())],
        suffix
    )
}

fn hash_string(value: &str) -> u64 {
    stable_hash_bytes(value.as_bytes())
}

/// What the whole cache ledger on this machine says, aggregated.
///
/// `stats.json` keeps only the LAST break of a session, so every question
/// worth asking ("which axis costs most", "did the anchor fix take") has to
/// be answered from `breaks.jsonl` across ALL sessions. That aggregation used
/// to be an ad-hoc shell pipeline run by hand; each run re-derived the
/// taxonomy, and one of them mis-stated the headline by 2x because it read a
/// single session's `stats.json`. Making it a function makes the number
/// reproducible and the gate in `architecture-r15.md` §2.3 checkable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheLedgerSummary {
    pub sessions: usize,
    pub rows: usize,
    /// Cache reads lost, summed over rows (`token_drop`).
    pub tokens_lost: u64,
    /// `(axis, rows, tokens_lost)`, heaviest first.
    pub by_axis: Vec<(String, usize, u64)>,
    /// Rows whose anchor sat where the previous request wrote one.
    pub anchor_reused: usize,
    /// Rows where no marker pointed at previously written content — the
    /// defect `previous_request_tail` fixed.
    pub anchor_missed: usize,
    /// Rows with no marker record on one side — written before the field
    /// existed, or the first request after it started.
    pub anchor_unknown: usize,
    /// Rows that DO carry markers but whose break had another cause. Kept
    /// apart from [`Self::anchor_unknown`] because the two mean opposite
    /// things about the instrument: unknown says it has not recorded anything
    /// yet, ineligible says it is recording and this break was not the
    /// anchor's fault. Reporting their sum as "rows predate marker recording"
    /// is how a live instrument reads as a dead one.
    pub anchor_ineligible: usize,
    /// Sum of `cache_read` / (`cache_read` + `cache_creation` + uncached
    /// input) over every session's `stats.json` — the §2.3 headline.
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub uncached_input_tokens: u64,
}

impl CacheLedgerSummary {
    /// Share of billed input served from cache, in basis points (9,650 =
    /// 96.50%). Basis points, not a float: this number is compared against a
    /// gate and printed, and both want an exact integer.
    ///
    /// **Reads high on mixed machines.** `total_input_tokens` is the uncached
    /// input, and the OpenAI-compatible and Gemini paths hardcode it to zero
    /// (see [`PromptCacheStats::total_input_tokens`]) — so those sessions
    /// contribute cache reads to the numerator and nothing to the denominator.
    /// A ledger holding non-Anthropic sessions therefore reports a ceiling,
    /// not the true share, and the real figure is at or below what this
    /// returns. Failing a 96.5% gate on this number is meaningful; passing it
    /// on a mixed ledger is not.
    #[must_use]
    pub fn cache_read_basis_points(&self) -> Option<u64> {
        let total = self
            .cache_read_tokens
            .checked_add(self.cache_creation_tokens)?
            .checked_add(self.uncached_input_tokens)?;
        (total > 0).then(|| self.cache_read_tokens.saturating_mul(10_000) / total)
    }

    /// Share of rows whose anchor was reusable, in basis points. `None` while
    /// no row carries a marker record — every row predates the field.
    #[must_use]
    pub fn anchor_reuse_basis_points(&self) -> Option<u64> {
        let known = self.anchor_reused + self.anchor_missed;
        (known > 0).then(|| (self.anchor_reused as u64).saturating_mul(10_000) / known as u64)
    }
}

/// Which axis a break row belongs to, in the taxonomy the ranking uses.
///
/// A row can set several flags; the label names the FIRST that applies in
/// cost order, because a row is counted once and the tool plane is the axis
/// worth attributing when it moved.
#[must_use]
pub fn break_row_axis(row: &CacheBreakLedgerRow) -> String {
    if row.tools_changed {
        return "tools changed".to_string();
    }
    if row.system_changed {
        return "system changed".to_string();
    }
    if row.model_changed {
        return "model changed".to_string();
    }
    if row.messages_changed {
        return if row.messages_truncated {
            "history truncated".to_string()
        } else if row.first_divergence_index.is_some() {
            "history diverged".to_string()
        } else {
            "history appended".to_string()
        };
    }
    match row.no_axis_cause() {
        Some(NoAxisBreakCause::ProviderSide) => "provider side".to_string(),
        Some(NoAxisBreakCause::FingerprintBump) => "fingerprint bump".to_string(),
        Some(NoAxisBreakCause::TtlExpiry) => "TTL expiry".to_string(),
        Some(NoAxisBreakCause::Unknown) | None => "unclassified".to_string(),
    }
}

/// Aggregate every session's `breaks.jsonl` and `stats.json` under the
/// configured cache roots.
///
/// Best-effort throughout: an unreadable session, a truncated final line, a
/// row from a newer schema — each is skipped, never fatal. A diagnostic that
/// refuses to run because one line is malformed is a diagnostic nobody runs.
#[must_use]
pub fn summarize_cache_ledger() -> CacheLedgerSummary {
    let mut summary = CacheLedgerSummary::default();
    let mut axes: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for root in cache_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let session = entry.path();
            if !session.is_dir() {
                continue;
            }
            summary.sessions += 1;
            if let Some(stats) = read_json::<PromptCacheStats>(&session.join("stats.json")) {
                summary.cache_read_tokens = summary
                    .cache_read_tokens
                    .saturating_add(stats.total_cache_read_input_tokens);
                summary.cache_creation_tokens = summary
                    .cache_creation_tokens
                    .saturating_add(stats.total_cache_creation_input_tokens);
                summary.uncached_input_tokens = summary
                    .uncached_input_tokens
                    .saturating_add(stats.total_input_tokens);
            }
            let Ok(text) = std::fs::read_to_string(session.join("breaks.jsonl")) else {
                continue;
            };
            for line in text.lines() {
                let Ok(row) = serde_json::from_str::<CacheBreakLedgerRow>(line) else {
                    continue;
                };
                summary.rows += 1;
                summary.tokens_lost = summary.tokens_lost.saturating_add(u64::from(row.token_drop));
                let slot = axes.entry(break_row_axis(&row)).or_default();
                slot.0 += 1;
                slot.1 = slot.1.saturating_add(u64::from(row.token_drop));
                match row.anchor_verdict() {
                    AnchorVerdict::Reused => summary.anchor_reused += 1,
                    AnchorVerdict::Missed => summary.anchor_missed += 1,
                    AnchorVerdict::Unrecorded => summary.anchor_unknown += 1,
                    AnchorVerdict::Ineligible => summary.anchor_ineligible += 1,
                }
            }
        }
    }
    summary.by_axis = axes
        .into_iter()
        .map(|(axis, (rows, tokens))| (axis, rows, tokens))
        .collect();
    summary
        .by_axis
        .sort_by(|left, right| right.2.cmp(&left.2).then_with(|| right.1.cmp(&left.1)));
    summary
}

fn cache_roots() -> Vec<PathBuf> {
    let homes = core_types::paths::zo_global_config_roots();
    let homes = if homes.is_empty() {
        vec![core_types::paths::default_config_home()]
    } else {
        homes
    };
    homes
        .into_iter()
        .map(|home| home.join("cache").join("prompt-cache"))
        .collect()
}

fn base_cache_root() -> PathBuf {
    cache_roots()
        .into_iter()
        .next()
        .expect("cache_roots always includes the primary config home")
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Millisecond wall clock for [`RequestLedgerRow::ts_unix_ms`]. See that
/// field for why the request ledger cannot use second resolution.
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

const fn current_fingerprint_version() -> u32 {
    REQUEST_FINGERPRINT_VERSION
}

fn stable_hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{
        base_cache_root, detect_cache_break, ensure_private_dir, first_divergence, read_json,
        request_hash_hex, sanitize_path_segment, write_json, CacheBreakLedgerRow, PromptCache,
        PromptCacheConfig, PromptCachePaths, TrackedPromptState, REQUEST_FINGERPRINT_PREFIX,
    };
    // Tests here mutate the process-wide ZO_CONFIG_HOME env var, so they must
    // serialize through the single crate-wide env lock rather than a private one;
    // two independent locks would let parallel tests race on the same env var.
    use crate::test_env_lock;
    use crate::types::{InputMessage, MessageRequest, MessageResponse, OutputContentBlock, Usage};

    #[test]
    fn path_builder_sanitizes_session_identifier() {
        let paths = PromptCachePaths::for_session("session:/with spaces");
        let session_dir = paths
            .session_dir
            .file_name()
            .and_then(|value| value.to_str())
            .expect("session dir name");
        assert_eq!(session_dir, "session--with-spaces");
        assert!(paths.completion_dir.ends_with("completions"));
        assert!(paths.stats_path.ends_with("stats.json"));
        assert!(paths.session_state_path.ends_with("session-state.json"));
    }

    #[test]
    fn request_fingerprint_drives_unexpected_break_detection() {
        let request = sample_request("same");
        let previous = TrackedPromptState::from_usage(
            &request,
            &Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 6_000,
                output_tokens: 0,
                output_tokens_details: None,
            },
        );
        let current = TrackedPromptState::from_usage(
            &request,
            &Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 1_000,
                output_tokens: 0,
                output_tokens_details: None,
            },
        );
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&previous),
            &current,
            None,
            1,
            TEST_MODEL,
        )
        .expect("break should be detected");
        assert!(event.unexpected);
        assert!(event.reason.contains("stable"));
    }

    /// A cache root of our own plus a session id no earlier run can have used.
    ///
    /// BOTH halves are load-bearing, and the second one is the half a
    /// "hermetic" test usually forgets. `ZO_CONFIG_HOME` PREPENDS a root; it
    /// does not replace the set (`core_types::paths::zo_global_config_roots`),
    /// so `read_json` still falls back to the developer's real `~/.zo`. A test
    /// that reuses a fixed session id therefore reads whatever a previous
    /// build left there — which is exactly how
    /// `every_request_stamps_what_it_ran_on` stayed green for a stamp the
    /// shipped code never wrote.
    struct CacheHome {
        home: std::path::PathBuf,
        session: String,
    }

    impl CacheHome {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let unique = format!("{label}-{}-{nonce}", std::process::id());
            let home = std::env::temp_dir().join(format!("zo-{unique}"));
            let _ = std::fs::remove_dir_all(&home);
            std::env::set_var("ZO_CONFIG_HOME", &home);
            Self {
                home,
                session: unique,
            }
        }

        fn requests(&self) -> Vec<super::RequestLedgerRow> {
            super::read_request_ledger(&PromptCachePaths::for_session(&self.session).requests_path)
        }
    }

    impl Drop for CacheHome {
        fn drop(&mut self) {
            std::env::remove_var("ZO_CONFIG_HOME");
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    fn plain_usage(cache_creation: u32, cache_read: u32, input: u32) -> Usage {
        Usage {
            input_tokens: input,
            cache_creation_input_tokens: cache_creation,
            cache_read_input_tokens: cache_read,
            output_tokens: 7,
            output_tokens_details: None,
        }
    }

    /// The ordinary request — the one `breaks.jsonl` never records — leaves a
    /// row, and that row can be read back with its clock, its model, and the
    /// TTL policy that priced it.
    ///
    /// Without this every per-request number in `docs/analysis/` had to be a
    /// session total divided by `tracked_requests`, which cannot be split by
    /// period, provider, or policy inside a session — and the inter-request
    /// gap distribution, which is what actually decided the conversation TTL,
    /// could not be measured from zo's own data at all.
    #[test]
    fn an_ordinary_request_appends_one_request_ledger_row() {
        let _env = test_env_lock();
        let home = CacheHome::new("request-ledger-row");
        let cache = PromptCache::new(home.session.clone());

        let before = super::now_unix_ms();
        let _ = cache.record_usage(&sample_request("hello"), &plain_usage(8_334, 51_317, 2));

        let rows = home.requests();
        assert_eq!(rows.len(), 1, "one ordinary request, one row: {rows:?}");
        let row = &rows[0];
        assert_eq!(row.seq, 1);
        assert!(
            row.ts_unix_ms >= before && row.ts_unix_ms <= super::now_unix_ms(),
            "the row must carry its own wall clock, got {}",
            row.ts_unix_ms
        );
        assert_eq!(row.model, TEST_MODEL);
        assert_eq!(row.provider, "anthropic");
        assert_eq!(row.cache_creation, 8_334);
        assert_eq!(row.cache_read, 51_317);
        assert_eq!(row.input_uncached, 2);
        assert_eq!(row.message_count, 1);
        assert!(!row.broke, "a first request has nothing to break from");
    }

    /// Two writers share one session, and the ledger must not lose either.
    ///
    /// The Anthropic client holds ONE `PromptCache` for the life of the
    /// session (`build_provider_client`), while every non-Anthropic response
    /// builds a FRESH one for the same session id
    /// (`runtime::record_non_anthropic_prompt_cache_usage`). Both own a full
    /// in-memory copy of `stats`, and `persist_state` writes the whole struct
    /// — so the long-lived writer's next request used to overwrite everything
    /// the short-lived one had counted.
    ///
    /// That is not a cosmetic loss. `tracked_requests` is the denominator
    /// every per-request figure in `docs/analysis/` divides by (r22 §1 rule 1),
    /// and the same overwrite takes the token totals with it. Measured in the
    /// live store before the fix: sessions that only ever ran Anthropic
    /// reproduced their transcript's request count 77 times out of 81; sessions
    /// that mixed providers, 0 times out of 34, holding 32% of the requests
    /// those sessions actually made.
    ///
    /// A model switch used to erase the previous model's numbers: one
    /// `previous` slot, overwritten. The warm-prefix table keeps every
    /// model's last observed prefix, carries it across a switch and across a
    /// fresh instance (the non-Anthropic path), and reads back by session id
    /// alone — which is how the turn host prices a stay against a switch.
    #[test]
    fn warm_prefixes_survive_a_model_switch_and_a_fresh_instance() {
        let _env = test_env_lock();
        let home = CacheHome::new("warm-prefixes");
        let cache = PromptCache::new(home.session.clone());

        // A: read 17,000 + wrote 3,000 under 5m markers.
        let _ = cache.record_usage(&marked_request("a"), &plain_usage(3_000, 17_000, 2));
        // B: a different model, cold — writes the whole prefix.
        let mut to_b = marked_request("b");
        to_b.model = "other-model".to_string();
        let _ = cache.record_usage(&to_b, &plain_usage(20_000, 0, 2));

        let warm = super::warm_prefixes_for_session(&home.session);
        assert_eq!(warm.len(), 2, "both models keep their own row: {warm:?}");
        assert_eq!(warm[TEST_MODEL].prefix_tokens, 20_000, "A's prefix survives the switch to B");
        assert_eq!(warm[TEST_MODEL].ttl_secs, 300, "5m markers");
        assert_eq!(warm["other-model"].prefix_tokens, 20_000);
        assert!(warm["other-model"].observed_at_unix_secs >= warm[TEST_MODEL].observed_at_unix_secs);

        // A fresh instance (the per-request path) carries the table forward,
        // and a request with no marker takes the default lifetime.
        let fresh = PromptCache::new(home.session.clone());
        let mut to_c = sample_request("c");
        to_c.model = "third-model".to_string();
        let _ = fresh.record_usage(&to_c, &plain_usage(0, 0, 5));
        let warm = super::warm_prefixes_for_session(&home.session);
        assert_eq!(warm.len(), 3, "{warm:?}");
        assert_eq!(warm["third-model"].ttl_secs, super::DEFAULT_WARM_TTL_SECS);
        assert_eq!(warm[TEST_MODEL].prefix_tokens, 20_000, "untouched rows are carried, not dropped");

        // A session that never recorded reads back empty, not an error.
        assert!(super::warm_prefixes_for_session("no-such-session").is_empty());
    }

    /// The plan-shape slot: deposited by attempt, withdrawn by attempt,
    /// overwritten in place, bounded so a long session cannot grow it, and
    /// silent on empty input.
    #[test]
    fn plan_shape_slot_remembers_by_attempt_and_forgets_the_oldest() {
        let _env = test_env_lock();
        let tag = format!("shape-slot-{}-{}", std::process::id(), super::now_unix_ms());
        assert_eq!(super::plan_shape_for_attempt(&format!("{tag}@1")), None);
        super::note_plan_shape(&format!("{tag}@1"), "solo");
        super::note_plan_shape(&format!("{tag}@1"), "host-prelude:3");
        assert_eq!(
            super::plan_shape_for_attempt(&format!("{tag}@1")).as_deref(),
            Some("host-prelude:3"),
            "the latest deposit for an attempt wins"
        );
        super::note_plan_shape("", "solo");
        super::note_plan_shape(&format!("{tag}@2"), "");
        assert_eq!(super::plan_shape_for_attempt(&format!("{tag}@2")), None);
        // Fill past the bound: the earliest deposit falls off, the rest stay.
        for turn in 3..(3 + super::PLAN_SHAPE_SLOTS) {
            super::note_plan_shape(&format!("{tag}@{turn}"), "solo");
        }
        assert_eq!(super::plan_shape_for_attempt(&format!("{tag}@1")), None);
        assert_eq!(
            super::plan_shape_for_attempt(&format!("{tag}@{}", 2 + super::PLAN_SHAPE_SLOTS)).as_deref(),
            Some("solo")
        );
        // The TTL mapping: the shortest marker wins, no marker means the default.
        assert_eq!(super::warm_ttl_secs("1h"), 3600);
        assert_eq!(super::warm_ttl_secs("5m+1h"), 300);
        assert_eq!(super::warm_ttl_secs(""), super::DEFAULT_WARM_TTL_SECS);
    }

    /// `requests.jsonl` is append-only from both writers, so its row count is
    /// the honest one — and this asserts the two agree, which is the same
    /// invariant `request_ledger_row_count_equals_tracked_requests` states for
    /// a single writer.
    #[test]
    fn two_writers_on_one_session_do_not_lose_each_others_requests() {
        let _env = test_env_lock();
        let home = CacheHome::new("two-writers-one-session");

        let long_lived = PromptCache::new(home.session.clone());
        let _ = long_lived.record_usage(&marked_request("a"), &plain_usage(1_000, 0, 2));

        let _ = PromptCache::new(home.session.clone())
            .record_usage(&marked_request("b"), &plain_usage(0, 2_000, 3));

        let _ = long_lived.record_usage(&marked_request("c"), &plain_usage(500, 900, 2));

        let persisted: super::PromptCacheStats =
            read_json(&PromptCachePaths::for_session(&home.session).stats_path).expect("stats");
        let rows = home.requests();
        assert_eq!(
            (persisted.tracked_requests, rows.len() as u64),
            (3, 3),
            "three requests on one session; stats said {} and the request ledger {}",
            persisted.tracked_requests,
            rows.len()
        );
        assert_eq!(
            rows.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "seq is the ordinal `breaks.jsonl` stamps; two writers must not repeat one"
        );
        assert_eq!(
            (
                persisted.total_cache_creation_input_tokens,
                persisted.total_cache_read_input_tokens,
                persisted.total_input_tokens,
            ),
            (1_500, 2_900, 7),
            "the token totals are additive across writers too"
        );
        // The marker columns must come out of the SHORT-LIVED writer too. That
        // instance is constructed fresh for one request, so it has no
        // in-memory history at all — every column that compares against a
        // predecessor rides `session-state.json`, the same field the
        // divergence detector rides for the same reason.
        assert_eq!(
            rows.iter()
                .map(|row| (row.markers.len(), row.moved.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, ""), (1, "same"), (1, "same")],
            "one marker record per row from both writers; the first has no predecessor"
        );
    }

    /// Each writer's `moved` and `read_drop` compare against ITS OWN previous
    /// request, not against whatever the peer wrote in between.
    ///
    /// This falls out of `adopt_peer_stats` adopting `stats` and NOT
    /// `previous`, and it is the right shape rather than a second half of that
    /// fix: the interleaved request went to a different provider, so it neither
    /// wrote nor read the Anthropic prefix cache these columns describe, and
    /// comparing across it would price a transition that never happened.
    /// r29 §4 could only ask what an interleave costs because the Anthropic
    /// client keeps looking at the last ANTHROPIC request.
    ///
    /// Stated here because the asymmetry is real and asymmetric: the
    /// short-lived writer, having no memory, does pick up whatever is on disk.
    /// A reader of `moved` needs to know which of the two wrote the row.
    #[test]
    fn each_writer_compares_its_markers_against_its_own_predecessor() {
        let _env = test_env_lock();
        let home = CacheHome::new("markers-two-writers-predecessor");

        let long_lived = PromptCache::new(home.session.clone());
        let _ = long_lived.record_usage(&marked_request("a"), &plain_usage(0, 60_000, 1));
        let _ = PromptCache::new(home.session.clone())
            .record_usage(&marked_request("b"), &plain_usage(0, 20_000, 1));
        let _ = long_lived.record_usage(&marked_request("c"), &plain_usage(0, 50_000, 1));

        assert_eq!(
            home.requests().iter().map(|row| row.read_drop).collect::<Vec<_>>(),
            vec![0, 40_000, 10_000],
            "the peer dropped against the long-lived writer's 60,000 (on disk); the \
             long-lived writer then dropped against its OWN 60,000, not the peer's 20,000"
        );
    }

    /// Row count == `tracked_requests`, breaks included.
    ///
    /// This is the property that makes the file a denominator rather than
    /// another biased sample. `breaks.jsonl` holds only break transitions, and
    /// every rate computed from it needs a count of ALL requests to divide by;
    /// if these two numbers can drift, the per-request figures every analysis
    /// doc quotes are wrong by an unknown factor.
    /// The attempt the runtime deposited is on every row it wrote — the join
    /// key that lets `requests.jsonl` meet `route-outcomes.jsonl`.
    #[test]
    fn request_row_carries_the_attempt_the_runtime_deposited() {
        let _env = test_env_lock();
        let home = CacheHome::new("request-row-attempt");
        let cache = PromptCache::new(home.session.clone());
        super::note_attempt(&home.session, "session-abc@3");

        let _ = cache.record_usage(&sample_request_with_messages(&["a"]), &plain_usage(0, 0, 1));

        let rows = home.requests();
        assert_eq!(
            rows.iter().map(|row| row.attempt.as_str()).collect::<Vec<_>>(),
            vec!["session-abc@3"],
            "the row must carry the attempt verbatim: {rows:?}"
        );
        super::note_attempt(&home.session, "");
    }

    /// The provider path that writes the most rows builds a FRESH
    /// `PromptCache` for every single response
    /// (`runtime::record_non_anthropic_prompt_cache_usage`). The attempt must
    /// survive that lifetime, or the column is right only on Anthropic.
    #[test]
    fn a_fresh_prompt_cache_per_request_still_carries_the_attempt() {
        let _env = test_env_lock();
        let home = CacheHome::new("request-row-attempt-fresh");
        super::note_attempt(&home.session, "agent-42#2");

        for text in ["a", "b"] {
            let _ = PromptCache::new(home.session.clone()).record_usage(
                &sample_request_with_messages(&[text]),
                &plain_usage(0, 0, 1),
            );
        }

        let rows = home.requests();
        assert_eq!(
            rows.iter().map(|row| row.attempt.as_str()).collect::<Vec<_>>(),
            vec!["agent-42#2", "agent-42#2"],
            "a per-request instance must read the same deposited attempt: {rows:?}"
        );
        super::note_attempt(&home.session, "");
    }

    /// One session's attempt is never another's, and clearing it stops the
    /// stamp rather than freezing a stale key.
    #[test]
    fn a_deposited_attempt_is_scoped_to_its_own_session() {
        let _env = test_env_lock();
        super::note_attempt("scope-a", "scope-a@1");
        super::note_attempt("scope-b", "scope-b@9");
        assert_eq!(super::attempt_for_session("scope-a"), "scope-a@1");
        assert_eq!(super::attempt_for_session("scope-b"), "scope-b@9");
        assert_eq!(super::attempt_for_session("scope-c"), "");
        super::note_attempt("scope-a", "");
        assert_eq!(super::attempt_for_session("scope-a"), "");
        super::note_attempt("scope-b", "");
    }

    /// The generated half of the bill. Without it an attempt's total billed
    /// tokens cannot be computed from this file, which is the whole point of
    /// joining on the attempt — a main-session turn has no other ledger that
    /// counts its output.
    #[test]
    fn request_row_carries_the_output_tokens_it_billed() {
        let _env = test_env_lock();
        let home = CacheHome::new("request-row-output");
        let cache = PromptCache::new(home.session.clone());

        let mut usage = plain_usage(11, 22, 33);
        usage.output_tokens = 4_096;
        let _ = cache.record_usage(&sample_request_with_messages(&["a"]), &usage);

        let rows = home.requests();
        let row = rows.first().expect("one row");
        assert_eq!(
            (row.cache_creation, row.cache_read, row.input_uncached, row.output),
            (11, 22, 33, 4_096),
            "all four billed terms come off the same usage: {rows:?}"
        );
    }

    /// The effort column is read off the request in hand, in the catalog's own
    /// spelling — explicit level, legacy budget, or nothing at all.
    #[test]
    fn request_row_carries_the_effort_the_request_asked_for() {
        let _env = test_env_lock();
        let home = CacheHome::new("request-row-effort");
        let cache = PromptCache::new(home.session.clone());

        let mut with_level = sample_request_with_messages(&["a"]);
        with_level.effort = Some(crate::types::EffortLevel::Xhigh);
        let mut with_budget = sample_request_with_messages(&["b"]);
        with_budget.thinking = Some(crate::types::ThinkingConfig::enabled(16_000));
        let plain = sample_request_with_messages(&["c"]);

        for request in [&with_level, &with_budget, &plain] {
            let _ = cache.record_usage(request, &plain_usage(0, 0, 1));
        }

        let rows = home.requests();
        assert_eq!(
            rows.iter().map(|row| row.effort.as_str()).collect::<Vec<_>>(),
            vec!["xhigh", "budget:16000", ""],
            "effort must be the request's own setting: {rows:?}"
        );
    }

    #[test]
    fn request_ledger_row_count_equals_tracked_requests() {
        let _env = test_env_lock();
        let home = CacheHome::new("request-ledger-denominator");
        let cache = PromptCache::new(home.session.clone());

        // A plain append, then a mid-history rewrite (which also writes a
        // break row), then another append: the row count must count all three.
        let _ = cache.record_usage(
            &sample_request_with_messages(&["a", "b"]),
            &plain_usage(0, 50_000, 1),
        );
        let _ = cache.record_usage(
            &sample_request_with_messages(&["a", "REWRITTEN", "c"]),
            &plain_usage(0, 100, 1),
        );
        let _ = cache.record_usage(
            &sample_request_with_messages(&["a", "REWRITTEN", "c", "d"]),
            &plain_usage(0, 40_000, 1),
        );

        let rows = home.requests();
        let stats = cache.stats();
        assert_eq!(stats.tracked_requests, 3);
        assert_eq!(
            rows.len() as u64,
            stats.tracked_requests,
            "row count and tracked_requests are the same event: {rows:?}"
        );
        assert_eq!(
            rows.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "seq is the join key into breaks.jsonl and must not skip"
        );
        let broke: Vec<u64> = rows.iter().filter(|row| row.broke).map(|row| row.seq).collect();
        let break_rows =
            super::read_break_ledger(&PromptCachePaths::for_session(&home.session).breaks_path);
        assert_eq!(
            broke,
            break_rows.iter().map(|row| row.seq).collect::<Vec<_>>(),
            "`broke` must agree with the file it claims to point at"
        );
        assert_eq!(broke, vec![2], "only the rewrite broke");
    }

    /// The TTL that `runtime::mark_conversation_cache_breakpoints` stamped on
    /// the request is what the row records — and "no marker" stays
    /// distinguishable from "the provider default".
    ///
    /// This column is why a TTL change can be judged from the ledger instead
    /// of from a deploy timestamp the data does not carry: rows written before
    /// and after a policy change label themselves.
    #[test]
    fn a_request_row_records_the_ttl_its_markers_asked_for() {
        let _env = test_env_lock();
        let home = CacheHome::new("request-ledger-ttl");
        let cache = PromptCache::new(home.session.clone());

        let marked = |text: &str, control: Option<crate::types::CacheControl>| {
            let mut request = sample_request(text);
            request.messages = vec![InputMessage {
                role: "user".to_string(),
                content: vec![crate::types::InputContentBlock::Text {
                    text: text.to_string(),
                    cache_control: control,
                }],
                thought_signature: None,
                reasoning_replay: None,
            }];
            request
        };

        let _ = cache.record_usage(&marked("unmarked", None), &plain_usage(0, 0, 5));
        let _ = cache.record_usage(
            &marked("five", Some(crate::types::CacheControl::ephemeral())),
            &plain_usage(0, 0, 5),
        );
        let _ = cache.record_usage(
            &marked("hour", Some(crate::types::CacheControl::ephemeral_1h())),
            &plain_usage(0, 0, 5),
        );

        let rows = home.requests();
        let ttls: Vec<&str> = rows.iter().map(|row| row.ttl.as_str()).collect();
        assert_eq!(
            ttls,
            vec!["", "5m", "1h"],
            "an absent marker is not the same fact as the 5-minute default"
        );
    }

    /// One row must stay small enough that keeping every one of them forever
    /// is cheaper than any rotation policy.
    ///
    /// This file has no cap and no rotation, and that is a decision this
    /// number backs: measured against the store's real shapes (the busiest
    /// model id here, `message_count` at the observed p90 of 635, six-figure
    /// token counts), a row is under the ceiling below and the machine records
    /// ~24,700 requests a month. Single-digit megabytes a month, for the only
    /// copy of data that cannot be reconstructed afterwards, does not earn a
    /// rotation mechanism — and a rotation would silently delete exactly the
    /// history a longitudinal comparison needs.
    ///
    /// The ceiling is what makes that argument keep holding: a column added
    /// later has to fit, or has to move this number and re-do the arithmetic
    /// in `docs/analysis/cache-remeasure-r22.md`. It last moved on 2026-09-15,
    /// when the attempt/effort join columns took the worst case from 360 to
    /// 404 bytes; the arithmetic is re-done in that document's §5.
    #[test]
    fn a_request_ledger_row_stays_small_enough_to_keep_forever() {
        const CEILING_BYTES: usize = 420;

        let marker = |index: i32, slot: &str, role: &str, ttl: &str| super::MarkerRecord {
            index,
            slot: slot.to_string(),
            role: role.to_string(),
            ttl: ttl.to_string(),
        };
        let row = super::RequestLedgerRow {
            seq: 24_710,
            ts_unix_ms: 1_788_055_407_582,
            model: "claude-opus-5".to_string(),
            provider: "anthropic".to_string(),
            cache_creation: 999_999,
            cache_read: 999_999,
            input_uncached: 999_999,
            output: 999_999,
            message_count: 635,
            broke: true,
            ttl: "5m".to_string(),
            // The full four-breakpoint Anthropic shape, which is also the
            // provider's ceiling: two 1h system blocks
            // (`runtime::split_system_with_identity` writes at most two) and
            // the anchor+rolling pair. The longest slot and role names are
            // deliberate — this is a ceiling, not an average.
            markers: vec![
                marker(super::NON_MESSAGE_MARKER_INDEX, super::MARKER_SLOT_SYSTEM, "", "1h"),
                marker(super::NON_MESSAGE_MARKER_INDEX, super::MARKER_SLOT_SYSTEM, "", "1h"),
                marker(633, super::MARKER_SLOT_ANCHOR, "assistant", "5m"),
                marker(634, super::MARKER_SLOT_ROLLING, "assistant", "5m"),
            ],
            moved: "anchor_moved".to_string(),
            read_drop: 999_999,
            // A spawn attempt is the longer of the two shapes: an agent id is
            // wall-clock NANOseconds (19 digits) against a session id's
            // milliseconds.
            attempt: "agent-1787013448552488000#12".to_string(),
            // A legacy thinking budget is the longer of the two spellings; a
            // level is at most `medium`.
            effort: "budget:32000".to_string(),
        };
        let line = serde_json::to_string(&row).expect("row serializes") + "\n";
        println!("request ledger row: {} bytes — {line}", line.len());
        assert!(
            line.len() <= CEILING_BYTES,
            "a row grew to {} bytes (ceiling {CEILING_BYTES}); \
             re-do the retention arithmetic in cache-remeasure-r22.md before raising it",
            line.len()
        );
    }

    /// The marker columns are recorded on EVERY request, not only on the ones
    /// that broke — the whole of r30.
    ///
    /// The same two facts (`cache_breakpoints`, `marker_roles`) were already
    /// being written to `breaks.jsonl`, and were written faithfully: every
    /// break row since the fields landed carried them, 3 of 3. But a break row
    /// exists only when reads DROP by at least `DEFAULT_BREAK_MIN_DROP`, so
    /// the control group — a request whose markers landed and whose cache held
    /// — could never appear, and neither could a run of failures after the
    /// first. Three marker records in 1,916 rows was the result.
    ///
    /// This asserts the shape that was missing: three requests, none of them
    /// breaking, three request rows all carrying markers, and `breaks.jsonl`
    /// empty beside them. Delete the `markers` line from `request_ledger_row`
    /// and only this test and its siblings below go red.
    #[test]
    fn markers_are_recorded_on_every_request_not_only_on_breaks() {
        let _env = test_env_lock();
        let home = CacheHome::new("markers-every-request");
        let cache = PromptCache::new(home.session.clone());

        for turn in 1..=3 {
            let mut request = sample_request_with_messages(&["a", "b", "c"]);
            // Mark the last two, the way `mark_conversation_cache_breakpoints`
            // does; reads hold steady so nothing breaks.
            for index in [1, 2] {
                request.messages[index].content = vec![crate::types::InputContentBlock::Text {
                    text: format!("m{index}"),
                    cache_control: Some(crate::types::CacheControl::ephemeral()),
                }];
            }
            let _ = cache.record_usage(&request, &plain_usage(100, 50_000, turn));
        }

        let rows = home.requests();
        assert_eq!(rows.len(), 3, "one row per request: {rows:?}");
        assert!(
            super::read_break_ledger(
                &PromptCachePaths::for_session(&home.session).breaks_path
            )
            .is_empty(),
            "no request broke, so the file that used to be the only carrier is empty"
        );
        for row in &rows {
            assert_eq!(
                row.markers
                    .iter()
                    .map(|marker| (marker.index, marker.slot.as_str(), marker.ttl.as_str()))
                    .collect::<Vec<_>>(),
                vec![(1, super::MARKER_SLOT_ANCHOR, "5m"), (2, super::MARKER_SLOT_ROLLING, "5m")],
                "every request records where its markers sat: {row:?}"
            );
        }
        assert_eq!(
            rows.iter().map(|row| row.moved.as_str()).collect::<Vec<_>>(),
            vec!["", "same", "same"],
            "the first request has no basis for comparison; the rest did not move"
        );
    }

    /// A system block's marker is recorded too, with `-1` for a position it
    /// does not have, and its own 1h lifetime.
    ///
    /// Anthropic allows four breakpoints per request and
    /// `runtime::split_system_with_identity` spends up to two of them on the
    /// system prefix. A row that recorded only the conversation pair could not
    /// say whether a request was at the provider's ceiling — which is a
    /// candidate explanation for the provider axis that r29 §4 could not test.
    #[test]
    fn a_system_block_marker_is_recorded_with_its_own_lifetime() {
        let _env = test_env_lock();
        let home = CacheHome::new("markers-system-block");
        let cache = PromptCache::new(home.session.clone());

        let mut request = sample_request("body");
        request.system = Some(vec![
            crate::types::SystemBlock::text("identity"),
            crate::types::SystemBlock::Text {
                text: "static".to_string(),
                cache_control: Some(crate::types::CacheControl::ephemeral_1h()),
            },
            crate::types::SystemBlock::Text {
                text: "dynamic".to_string(),
                cache_control: Some(crate::types::CacheControl::ephemeral_1h()),
            },
        ]);
        request.messages[0].content = vec![crate::types::InputContentBlock::Text {
            text: "body".to_string(),
            cache_control: Some(crate::types::CacheControl::ephemeral()),
        }];
        let _ = cache.record_usage(&request, &plain_usage(100, 0, 1));

        let rows = home.requests();
        assert_eq!(
            rows[0]
                .markers
                .iter()
                .map(|marker| (marker.index, marker.slot.as_str(), marker.role.as_str(), marker.ttl.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (super::NON_MESSAGE_MARKER_INDEX, super::MARKER_SLOT_SYSTEM, "", "1h"),
                (super::NON_MESSAGE_MARKER_INDEX, super::MARKER_SLOT_SYSTEM, "", "1h"),
                (0, super::MARKER_SLOT_ROLLING, "user", "5m"),
            ],
            "the uncached identity block is not a marker; the two cached ones are"
        );
        assert_eq!(
            rows[0].ttl, "5m",
            "the `ttl` column stays the CONVERSATION policy — the system prefix has always \
             had its own lifetime, and folding the two would silently rewrite r21's column"
        );
    }

    /// `moved` names the five shapes, and stays silent when it has no basis.
    ///
    /// Silence is a distinct reading, not a sixth shape: r29 §4 lost a round
    /// to a field whose two kinds of nothing looked alike.
    #[test]
    fn marker_movement_names_the_shape_of_the_change() {
        use super::marker_movement as moved;
        assert_eq!(moved(None, &[4, 6]), "", "a first request has no predecessor");
        assert_eq!(moved(Some(&[]), &[]), "", "neither side carried a marker");
        assert_eq!(moved(Some(&[4, 6]), &[4, 6]), "same");
        assert_eq!(
            moved(Some(&[4, 6]), &[6, 8]),
            "rolled",
            "the anchor took the position the last request's rolling marker wrote"
        );
        assert_eq!(
            moved(Some(&[4, 6]), &[8, 10]),
            "anchor_moved",
            "neither marker points at anything an earlier request wrote"
        );
        assert_eq!(moved(Some(&[6]), &[4, 6]), "added");
        assert_eq!(moved(Some(&[4, 6]), &[6]), "removed");
    }

    /// `read_drop` and the break row's `token_drop` are the same subtraction,
    /// and `read_drop` also covers the drops that are too small to write a
    /// break row at all.
    ///
    /// Two numbers that claim to be one number must be pinned together, and
    /// the sub-threshold half is why this column exists: a marker move that
    /// costs 1,900 tokens on every request writes nothing anywhere else.
    #[test]
    fn read_drop_agrees_with_the_break_rows_token_drop() {
        let _env = test_env_lock();
        let home = CacheHome::new("markers-read-drop");
        let cache = PromptCache::new(home.session.clone());

        let _ = cache.record_usage(&sample_request("a"), &plain_usage(0, 50_000, 1));
        // Below `DEFAULT_BREAK_MIN_DROP`: no break row exists to hold this.
        let _ = cache.record_usage(&sample_request("a"), &plain_usage(0, 49_000, 1));
        // Above it: a break row exists, and must agree.
        let _ = cache.record_usage(&sample_request("a"), &plain_usage(0, 0, 1));

        let rows = home.requests();
        assert_eq!(
            rows.iter().map(|row| row.read_drop).collect::<Vec<_>>(),
            vec![0, 1_000, 49_000],
            "the first request has no predecessor to have dropped from"
        );
        assert_eq!(
            rows.iter().map(|row| row.broke).collect::<Vec<_>>(),
            vec![false, false, true],
            "only the third drop clears the break floor"
        );
        let breaks =
            super::read_break_ledger(&PromptCachePaths::for_session(&home.session).breaks_path);
        assert_eq!(
            breaks.iter().map(|row| (row.seq, row.token_drop)).collect::<Vec<_>>(),
            vec![(3, 49_000)],
            "the one break row must carry the same number the request row does"
        );
    }

    /// A tool block cannot carry a marker, so [`super::request_markers`] does
    /// not look for one — and this is the pin that makes that safe.
    ///
    /// `MarkerRecord`'s vocabulary has a `tools` slot in the analysis tool
    /// because Anthropic's wire format allows a breakpoint there; ours does
    /// not have the field, and scanning the whole tool array per request to
    /// prove a negative is not free. The day the field appears, this fails and
    /// points at the recorder rather than the ledger quietly under-counting
    /// the slots a request spent.
    #[test]
    fn a_tool_definition_cannot_carry_a_marker() {
        let tool = crate::types::ToolDefinition {
            name: "Read".to_string(),
            description: Some("read a file".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let wire = serde_json::to_string(&tool).expect("tool serializes");
        assert!(
            !wire.contains("cache_control"),
            "a tool block gained a marker field; teach `request_markers` about it: {wire}"
        );
    }

    /// The typed marker read finds exactly what the recursive JSON scan it
    /// replaced found.
    ///
    /// The scan was correct and cost a full `serde_json::to_value` of the whole
    /// history, three times per request. Recording markers per-request would
    /// have made it four. This keeps the old definition as the oracle rather
    /// than as production code: `cache_control` lives on exactly five block
    /// variants, and `InputMessage`'s two other fields are `#[serde(skip)]`,
    /// so the two reads cannot disagree.
    #[test]
    fn the_typed_marker_read_finds_what_the_json_scan_found() {
        fn scan(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::Object(map) => {
                    map.contains_key("cache_control") || map.values().any(scan)
                }
                serde_json::Value::Array(items) => items.iter().any(scan),
                _ => false,
            }
        }

        let marked = |control: Option<crate::types::CacheControl>| crate::types::InputContentBlock::ToolResult {
            tool_use_id: "toolu_1".to_string(),
            content: vec![crate::types::ToolResultContentBlock::Text {
                text: "out".to_string(),
            }],
            is_error: false,
            cache_control: control,
        };
        let messages = vec![
            InputMessage {
                role: "assistant".to_string(),
                content: vec![
                    crate::types::InputContentBlock::Thinking {
                        thinking: "…".to_string(),
                        signature: "sig".to_string(),
                    },
                    crate::types::InputContentBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "Read".to_string(),
                        input: serde_json::json!({"path": "x"}),
                        cache_control: None,
                    },
                ],
                thought_signature: Some("gemini".to_string()),
                reasoning_replay: Some(serde_json::json!({"cache_control": "decoy"})),
            },
            InputMessage {
                role: "user".to_string(),
                content: vec![marked(None), marked(Some(crate::types::CacheControl::ephemeral_1h()))],
                thought_signature: None,
                reasoning_replay: None,
            },
        ];

        let by_scan: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                scan(&serde_json::to_value(message).expect("message serializes"))
            })
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            super::cache_breakpoint_indices(&messages),
            by_scan,
            "the typed read and the JSON scan must see the same markers — and the \
             `serde(skip)` decoy in `reasoning_replay` must be invisible to both"
        );
        assert_eq!(by_scan, vec![1]);
        assert_eq!(
            super::conversation_marker_ttl(&messages),
            "1h",
            "a marker on a tool_result block is a marker"
        );
    }

    /// Disagreeing markers are reported as both, not as whichever came first.
    ///
    /// One call marks the anchor and the rolling slot today, so this cannot
    /// happen yet — and that is the point: splitting the two TTLs is the
    /// obvious next move on this axis (r20 §4 proposed exactly it), and a
    /// column that silently reported half of the policy would make the round
    /// that tried it unreadable.
    #[test]
    fn a_request_row_reports_disagreeing_marker_ttls_as_both() {
        let mut anchor = InputMessage::user_text("anchor");
        anchor.content = vec![crate::types::InputContentBlock::Text {
            text: "anchor".to_string(),
            cache_control: Some(crate::types::CacheControl::ephemeral_1h()),
        }];
        let mut rolling = InputMessage::user_text("rolling");
        rolling.content = vec![crate::types::InputContentBlock::Text {
            text: "rolling".to_string(),
            cache_control: Some(crate::types::CacheControl::ephemeral()),
        }];
        assert_eq!(
            super::conversation_marker_ttl(&[anchor, rolling]),
            "1h+5m",
            "a mixed policy must be visible as mixed"
        );
    }

    /// A marker survives the compact wire form unchanged, and a row written
    /// before the column existed still loads.
    ///
    /// `MarkerRecord` is a string on disk, not an object, for the byte budget
    /// `a_request_ledger_row_stays_small_enough_to_keep_forever` states — and
    /// a hand-rolled codec that cannot round-trip is a ledger that lies. The
    /// second half is the `#[serde(default)]` discipline every column of this
    /// file follows: 38 rows on the live store were written before this one.
    #[test]
    fn a_marker_round_trips_through_its_compact_wire_form() {
        let markers = vec![
            super::MarkerRecord {
                index: super::NON_MESSAGE_MARKER_INDEX,
                slot: super::MARKER_SLOT_SYSTEM.to_string(),
                role: String::new(),
                ttl: "1h".to_string(),
            },
            super::MarkerRecord {
                index: 274,
                slot: super::MARKER_SLOT_ROLLING.to_string(),
                role: "user".to_string(),
                ttl: "5m".to_string(),
            },
        ];
        let wire = serde_json::to_string(&markers).expect("markers serialize");
        assert_eq!(wire, r#"["-1:system::1h","274:rolling:user:5m"]"#);
        assert_eq!(
            serde_json::from_str::<Vec<super::MarkerRecord>>(&wire).expect("markers parse"),
            markers
        );

        let legacy: super::RequestLedgerRow = serde_json::from_str(
            r#"{"seq":1,"ts_unix_ms":1788055407582,"cache_creation":0,"cache_read":0,
                "input_uncached":0,"message_count":1,"broke":false}"#,
        )
        .expect("a row written before the marker columns still loads");
        assert_eq!(
            (legacy.markers.len(), legacy.moved.as_str(), legacy.read_drop),
            (0, "", 0),
            "an older row reads as `no marker record`, which is what it is"
        );
    }

    /// A session directory written before this ledger existed — no
    /// `requests.jsonl`, and a `PromptCachePaths` serialized without the field
    /// — must load and read as empty, never fail.
    ///
    /// Same `#[serde(default)]` discipline `TrackedPromptState` follows: the
    /// store on this machine holds 720 session directories, every one of them
    /// written before this file existed.
    #[test]
    fn a_session_without_a_request_ledger_reads_empty_and_still_loads() {
        let missing = std::env::temp_dir()
            .join("zo-no-such-session-dir")
            .join("requests.jsonl");
        assert!(super::read_request_ledger(&missing).is_empty());

        let legacy = serde_json::json!({
            "root": "/tmp/zo-legacy",
            "session_dir": "/tmp/zo-legacy/s",
            "completion_dir": "/tmp/zo-legacy/s/completions",
            "session_state_path": "/tmp/zo-legacy/s/session-state.json",
            "stats_path": "/tmp/zo-legacy/s/stats.json",
        });
        let paths: PromptCachePaths =
            serde_json::from_value(legacy).expect("a pre-ledger paths record must still load");
        assert_eq!(paths.requests_path, std::path::PathBuf::new());
        // And the appender no-ops on that empty path rather than panicking.
        super::append_request_row(&paths, &super::RequestLedgerRow::default());
    }

    /// The trim-pricing pair: a deposited microcompact credit rides the SAME
    /// session's next break row as `trimmed_tokens_estimate`, is consumed
    /// exactly once, and never crosses sessions.
    #[test]
    fn a_context_trim_credit_stamps_the_next_break_row_once_for_its_own_session() {
        use super::{note_context_trim, read_break_ledger, take_pending_context_trim};
        let _env = test_env_lock();
        let home = std::env::temp_dir().join(format!("zo-trim-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let session = format!("trim-pair-{}", std::process::id());
        let cache = PromptCache::new(session.clone());
        let usage_with_read = |cache_read: u32| Usage {
            input_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: cache_read,
            output_tokens: 0,
            output_tokens_details: None,
        };
        let _ = cache.record_usage(&sample_request_with_messages(&["a", "b"]), &usage_with_read(50_000));
        note_context_trim(&session, 12_345);
        note_context_trim("someone-else", 999);
        // The trim rewrote history mid-prefix → divergence → break row.
        let _ = cache.record_usage(
            &sample_request_with_messages(&["a", "REWRITTEN", "c"]),
            &usage_with_read(30_000),
        );
        let stats = cache.stats();
        assert_eq!(stats.context_trims_noted, 1);
        assert_eq!(stats.context_trim_tokens_noted, 12_345);
        let rows = read_break_ledger(&PromptCachePaths::for_session(&session).breaks_path);
        let row = rows.last().expect("break row written");
        assert_eq!(row.trimmed_tokens_estimate, Some(12_345), "{row:?}");
        // Consumed once: the next break carries nothing.
        let _ = cache.record_usage(
            &sample_request_with_messages(&["a", "DIFFERENT", "c"]),
            &usage_with_read(500),
        );
        let rows = read_break_ledger(&PromptCachePaths::for_session(&session).breaks_path);
        assert_eq!(rows.last().expect("second row").trimmed_tokens_estimate, None);
        // The other session's deposit is still waiting for ITS next request.
        assert_eq!(take_pending_context_trim("someone-else"), 999);
        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A pure tail append cannot legitimately drop cache reads — the previous
    /// request is a byte-identical prefix, and the provider caches on
    /// content. A drop there is provider-side, and it must be UNEXPECTED, not
    /// filed under "message payload changed (expected)": that misclass buried
    /// 84M dropped tokens across 577 production rows as self-inflicted.
    #[test]
    fn a_read_drop_on_a_pure_append_is_unexpected_not_self_inflicted() {
        let read = |request: &MessageRequest, cache_read: u32| {
            TrackedPromptState::from_usage(
                request,
                &Usage {
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: cache_read,
                    output_tokens: 0,
                    output_tokens_details: None,
                },
            )
        };
        let previous = read(&sample_request_with_messages(&["a", "b"]), 50_000);
        let appended = sample_request_with_messages(&["a", "b", "c"]);
        // Within the TTL: the gap cannot explain the drop — provider-side.
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&previous),
            &read(&appended, 1_000),
            None, // shared prefix identical: no divergence index, like a real append
            3,
            TEST_MODEL,
        )
        .expect("drop on append must produce a row");
        assert!(event.unexpected, "{}", event.reason);
        assert!(event.reason.contains("pure append"), "{}", event.reason);
        assert!(
            event.reason.contains("provider-side"),
            "{}",
            event.reason
        );
        // A drop AFTER the TTL window is the one self-explaining append case.
        let mut stale_previous = read(&sample_request_with_messages(&["a", "b"]), 50_000);
        stale_previous.observed_at_unix_secs =
            stale_previous.observed_at_unix_secs.saturating_sub(24 * 60 * 60);
        let ttl_event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&stale_previous),
            &read(&appended, 1_000),
            None,
            3,
            TEST_MODEL,
        )
        .expect("row still written");
        assert!(!ttl_event.unexpected, "{}", ttl_event.reason);
        assert!(ttl_event.reason.contains("TTL"), "{}", ttl_event.reason);
    }

    /// A `tools_changed` break must name the tools. Without this the ledger says
    /// only "tool definitions changed", which is where a real investigation had
    /// to start over from measurement: 41% of one day's cache-write came from
    /// this axis with no record of which tools moved.
    #[test]
    fn a_tools_changed_break_names_the_tools_that_came_and_went() {
        let with = |names: &[&str]| {
            let mut request = sample_request("same prompt");
            request.tools = Some(
                names
                    .iter()
                    .map(|name| crate::types::ToolDefinition {
                        name: (*name).to_string(),
                        description: None,
                        input_schema: serde_json::json!({"type": "object"}),
                    })
                    .collect(),
            );
            request
        };
        let state = |request: &MessageRequest, cache_read: u32| {
            TrackedPromptState::from_usage(
                request,
                &Usage {
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: cache_read,
                    output_tokens: 0,
                    output_tokens_details: None,
                },
            )
        };

        // A deep leg dropping its delegation tools, then the main lane restoring
        // them — the exact oscillation that stranded whole prefixes.
        let full = state(&with(&["read_file", "Agent", "Workflow"]), 200_000);
        let leg = state(&with(&["read_file"]), 0);
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&full),
            &leg,
            None,
            1,
            TEST_MODEL,
        )
        .expect("break");
        assert!(event.tools_changed);
        assert_eq!(event.tools_removed, vec!["Agent", "Workflow"]);
        assert!(event.tools_added.is_empty());
        assert!(
            event.reason.contains("(-Agent, -Workflow)"),
            "reason must name them: {}",
            event.reason
        );

        // An identical tool set is not a tools_changed break at all — whatever
        // else the request did, this axis stays quiet and names nothing.
        let back = state(&with(&["read_file", "Agent", "Workflow"]), 0);
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&full),
            &back,
            None,
            1,
            TEST_MODEL,
        )
        .expect("the token drop alone is still a break");
        assert!(!event.tools_changed);
        assert!(event.tools_added.is_empty() && event.tools_removed.is_empty());

        // Same names in a different order: both diffs empty, and the reason says
        // so rather than implying a membership change.
        let reordered = state(&with(&["Agent", "Workflow", "read_file"]), 0);
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&full),
            &reordered,
            None,
            1,
            TEST_MODEL,
        )
        .expect("break");
        assert!(event.tools_changed);
        assert!(event.tools_added.is_empty() && event.tools_removed.is_empty());
        assert!(
            event.reason.contains("same names; order or definition changed"),
            "{}",
            event.reason
        );
    }

    /// A mid-prefix rewrite must say WHAT changed at the divergence, not only
    /// where. Five identical "diverged at message 275/1142" rows could not tell
    /// an in-memory context trim from a tool-history rewrite; a role, the block
    /// kinds and a byte count do.
    #[test]
    fn a_mid_prefix_rewrite_describes_the_message_that_changed() {
        let long_body = "x".repeat(4_000);
        let previous_request = sample_request_with_messages(&[&long_body, "keep going"]);
        let current_request = sample_request_with_messages(&["[context trimmed]", "keep going"]);
        let state = |request: &MessageRequest, cache_read: u32| {
            TrackedPromptState::from_usage(
                request,
                &Usage {
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: cache_read,
                    output_tokens: 0,
                    output_tokens_details: None,
                },
            )
        };
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&state(&previous_request, 200_000)),
            &state(&current_request, 0),
            Some(0),
            2,
            TEST_MODEL,
        )
        .expect("break");

        let diverged = event.diverged_message.as_ref().expect("shape recorded");
        assert_eq!(diverged.index, 0);
        assert_eq!(diverged.previous.role, "user");
        assert_eq!(diverged.previous.kinds, "text");
        assert!(
            diverged.previous.bytes > 4_000 && diverged.current.bytes < 100,
            "the shrink is the whole signal: {diverged:?}"
        );
        assert_eq!(
            diverged.shift.as_deref(),
            Some("edited"),
            "this fixture rewrites message 0 in place, so nothing slid"
        );
        assert!(
            event
                .reason
                .contains("history diverged at message 0/2 (edited, user [text] "),
            "{}",
            event.reason
        );
        assert!(
            event.reason.contains("kB -> ") && event.reason.contains('B'),
            "the reason must carry both sizes: {}",
            event.reason
        );
    }

    /// The compact wire form must round-trip, including a `kinds` field that
    /// itself carries separators (`tool_use:Agent,text`) and the `+N` tail.
    #[test]
    fn a_message_shape_round_trips_through_its_compact_string_form() {
        let shape = super::WireMessageShape {
            role: "user".to_string(),
            kinds: "tool_use:Agent,text,+12".to_string(),
            bytes: 12_431,
        };
        let json = serde_json::to_string(&shape).expect("serialize");
        assert_eq!(json, "\"user|tool_use:Agent,text,+12|12431\"");
        assert_eq!(
            serde_json::from_str::<super::WireMessageShape>(&json).expect("deserialize"),
            shape
        );
        // A malformed entry must fail loudly rather than deserialize to zeros.
        assert!(serde_json::from_str::<super::WireMessageShape>("\"user|text\"").is_err());
        assert!(serde_json::from_str::<super::WireMessageShape>("\"user|text|huge\"").is_err());
    }

    /// The shapes must survive the same upgrade path the hashes did: a state
    /// file written before the field existed still loads, and the break it
    /// produces simply carries no shape rather than failing.
    #[test]
    fn a_state_file_without_shapes_still_loads_and_breaks_without_one() {
        let old_json = serde_json::json!({
            "observed_at_unix_secs": 1_000_000,
            "fingerprint_version": super::current_fingerprint_version(),
            "model_hash": 1,
            "system_hash": 2,
            "tools_hash": 3,
            "messages_hash": 4,
            "cache_read_input_tokens": 200_000,
            "message_hashes": [11, 22],
        })
        .to_string();
        let previous: TrackedPromptState =
            serde_json::from_str(&old_json).expect("pre-shape state must still deserialize");
        assert!(previous.message_shapes.is_empty());

        let current = TrackedPromptState::from_usage(
            &sample_request_with_messages(&["changed", "tail"]),
            &Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: 0,
                output_tokens_details: None,
            },
        );
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&previous),
            &current,
            Some(0),
            2,
            TEST_MODEL,
        )
        .expect("break");
        assert!(event.diverged_message.is_none());
        assert!(
            event.reason.contains("history diverged at message 0/2"),
            "{}",
            event.reason
        );
    }

    #[test]
    fn changed_prompt_marks_break_as_expected() {
        let previous_request = sample_request("first");
        let current_request = sample_request("second");
        let previous = TrackedPromptState::from_usage(
            &previous_request,
            &Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 6_000,
                output_tokens: 0,
                output_tokens_details: None,
            },
        );
        let current = TrackedPromptState::from_usage(
            &current_request,
            &Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 1_000,
                output_tokens: 0,
                output_tokens_details: None,
            },
        );
        // The two single-message requests differ at message index 0, matching
        // what `first_divergence` would compute for this exact pair — passed
        // explicitly here since this test drives `detect_cache_break` directly
        // rather than through `PromptCache::record_usage`.
        let event = detect_cache_break(
            &PromptCacheConfig::default(),
            Some(&previous),
            &current,
            Some(0),
            1,
            TEST_MODEL,
        )
        .expect("break should be detected");
        assert!(!event.unexpected);
        assert!(event.reason.contains("message payload changed"));
        assert!(event.reason.contains("history diverged at message 0/1"));
    }

    /// End-to-end break ledger: every break-bearing request appends one row
    /// whose axis flags are structured (no string parsing), non-break requests
    /// append nothing, and the provider-side-miss signature (all axes stable,
    /// `unexpected: true`) is distinguishable from a tools-axis break — the
    /// exact per-request evidence `stats.json`'s single `last_break_reason`
    /// kept losing.
    #[test]
    fn break_ledger_records_each_break_with_axis_flags() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-breaks-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let cache = PromptCache::new("break-ledger-session");
        let warm = |cr: u32| Usage {
            input_tokens: 10,
            cache_creation_input_tokens: 100,
            cache_read_input_tokens: cr,
            output_tokens: 5,
            output_tokens_details: None,
        };

        // req1: baseline (no previous → no break).
        let base = sample_request("stable message");
        let record = cache.record_usage(&base, &warm(6_000));
        assert!(record.cache_break.is_none());

        // req2: tools axis changes, everything else identical → expected
        // break attributed to tools only.
        let mut with_tool = base.clone();
        with_tool.tools = Some(vec![crate::types::ToolDefinition {
            name: "probe".into(),
            description: Some("probe tool".into()),
            input_schema: serde_json::json!({"type": "object"}),
        }]);
        let record = cache.record_usage(&with_tool, &warm(0));
        let event = record.cache_break.expect("tools change must break");
        assert!(event.tools_changed && !event.model_changed && !event.system_changed);

        // req3: identical request repeated, cache reads recover → no break,
        // no ledger row.
        let record = cache.record_usage(&with_tool, &warm(6_000));
        assert!(record.cache_break.is_none());

        // req4: byte-identical payload yet reads collapse inside the TTL —
        // the provider-side-miss signature.
        let record = cache.record_usage(&with_tool, &warm(0));
        let event = record.cache_break.expect("stable-fingerprint drop must break");
        assert!(event.unexpected);
        assert!(
            !event.model_changed
                && !event.system_changed
                && !event.tools_changed
                && !event.messages_changed
        );

        // req5/req6: grow the history, then truncate it with the surviving
        // prefix intact (the compaction/rewind shape). The divergence index is
        // `None` both ways — the truncation flag and label must tell them
        // apart, or a compaction cold-rewrite reads as "append-only".
        let mut grown = with_tool.clone();
        grown.messages = vec![
            InputMessage::user_text("stable message"),
            InputMessage::user_text("second"),
            InputMessage::user_text("third"),
        ];
        let _ = cache.record_usage(&grown, &warm(6_000));
        let mut truncated = with_tool.clone();
        truncated.messages = vec![InputMessage::user_text("stable message")];
        let record = cache.record_usage(&truncated, &warm(0));
        let event = record.cache_break.expect("truncation must break");
        assert!(event.messages_changed && event.messages_truncated);
        assert!(
            event.reason.contains("history truncated (3 -> 1 messages"),
            "truncation must be labeled, got: {}",
            event.reason
        );

        let rows = super::read_break_ledger(&cache.paths().breaks_path);
        assert_eq!(rows.len(), 3, "one row per break, none for quiet requests: {rows:?}");
        assert_eq!(rows[0].seq, 2);
        assert!(rows[0].tools_changed && !rows[0].unexpected);
        assert!(rows[0].reason.contains("tool definitions changed"));
        assert_eq!(rows[0].prev_cache_read, 6_000);
        assert_eq!(rows[0].cache_read, 0);
        assert_eq!(rows[0].cache_creation, 100);
        assert_eq!(rows[1].seq, 4);
        assert!(rows[1].unexpected, "all-axes-stable drop is the provider-side-miss row");
        assert!(
            !rows[1].model_changed
                && !rows[1].system_changed
                && !rows[1].tools_changed
                && !rows[1].messages_changed
        );
        assert!(!rows[1].messages_truncated);
        assert_eq!(rows[2].seq, 6);
        assert!(rows[2].messages_truncated, "truncation row carries the flag: {rows:?}");
        assert_eq!(rows[2].prev_message_count, 3);
        assert_eq!(rows[2].message_count, 1);
        assert_eq!(rows[2].prefix_stable_messages, 1);

        // Reader is lossy: a corrupt line is skipped, not fatal.
        {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&cache.paths().breaks_path)
                .expect("ledger file exists");
            writeln!(file, "not json").expect("append corrupt line");
        }
        assert_eq!(super::read_break_ledger(&cache.paths().breaks_path).len(), 3);

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    /// A row must name the model the break happened on and that model's
    /// provider family. One store holds rows from every provider a machine
    /// touched, and until the fields existed the family could only be INFERRED
    /// from arithmetic artifacts (`cache_creation == 0`, `cache_read` divisible
    /// by 128) — an inference a real cost investigation got wrong, quoting a
    /// median over the mixed population before it found the split. Two
    /// providers inside one scope here, because that is the case that used to
    /// be unreadable.
    #[test]
    fn a_break_row_names_the_wire_model_and_provider_family_that_broke() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-attribution-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let cache = PromptCache::new("break-attribution-session");
        let warm = |cr: u32| Usage {
            input_tokens: 10,
            cache_creation_input_tokens: 100,
            cache_read_input_tokens: cr,
            output_tokens: 5,
            output_tokens_details: None,
        };

        // Anthropic leg: a warm request, then a byte-stable read collapse.
        let claude = sample_request("attribute me");
        assert!(cache.record_usage(&claude, &warm(6_000)).cache_break.is_none());
        let event = cache
            .record_usage(&claude, &warm(0))
            .cache_break
            .expect("a byte-stable read collapse is a break");
        assert_eq!(event.model, TEST_MODEL);
        assert_eq!(event.provider, "anthropic");

        // Same scope, other provider. The model switch itself is not the break
        // under test — it recovers cache reads, so the row lands on the next
        // collapse, exactly as a real provider hand-off would.
        let mut gemini = claude.clone();
        gemini.model = "gemini-2.5-pro".to_string();
        assert!(cache.record_usage(&gemini, &warm(6_000)).cache_break.is_none());
        let event = cache
            .record_usage(&gemini, &warm(0))
            .cache_break
            .expect("second collapse is a break");
        assert_eq!(event.model, "gemini-2.5-pro");
        assert_eq!(event.provider, "google");

        let rows = super::read_break_ledger(&cache.paths().breaks_path);
        assert_eq!(rows.len(), 2, "one row per break: {rows:?}");
        assert_eq!(
            (rows[0].model.as_str(), rows[0].provider.as_str()),
            (TEST_MODEL, "anthropic")
        );
        assert_eq!(
            (rows[1].model.as_str(), rows[1].provider.as_str()),
            ("gemini-2.5-pro", "google"),
            "a mixed-provider scope must read without inference: {rows:?}"
        );

        std::env::remove_var("ZO_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    /// Attribution is additive in both directions: a row written before the
    /// fields existed still loads, and a row that cannot fill them does not pay
    /// bytes for them (the ledger gains a line per break for the life of every
    /// session, so an always-present empty key is a permanent tax).
    #[test]
    fn a_row_written_before_attribution_still_loads_and_empty_fields_stay_off_the_wire() {
        let old_line = serde_json::json!({
            "seq": 7,
            "ts_unix_secs": 1_000,
            "unexpected": false,
            "reason": "tool definitions changed",
            "model_changed": false,
            "system_changed": false,
            "tools_changed": true,
            "messages_changed": false,
            "first_divergence_index": serde_json::Value::Null,
            "prev_message_count": 40,
            "message_count": 41,
            "prev_cache_read": 700_000,
            "cache_read": 24_000,
            "cache_creation": 690_000,
            "token_drop": 676_000,
            "elapsed_secs": 12,
        })
        .to_string();
        let dir = tempfile::TempDir::new().expect("mkdir ledger dir");
        let path = dir.path().join("breaks.jsonl");
        std::fs::write(&path, format!("{old_line}\n")).expect("write pre-attribution ledger");

        let rows = super::read_break_ledger(&path);
        assert_eq!(rows.len(), 1, "an old row must still parse: {rows:?}");
        assert!(rows[0].tools_changed, "the rest of the row must survive too");
        assert!(
            rows[0].model.is_empty() && rows[0].provider.is_empty(),
            "an old row carries no attribution rather than failing to load"
        );

        let line = serde_json::to_string(&rows[0]).expect("serialize unfilled row");
        assert!(!line.contains("\"model\":"), "unfilled model must be omitted: {line}");
        assert!(!line.contains("\"provider\":"), "unfilled provider must be omitted: {line}");

        // The two assertions above are only meaningful if a FILLED row does
        // write the keys — otherwise they would pass on a field that never
        // serializes at all.
        let mut filled = rows[0].clone();
        filled.model = TEST_MODEL.to_string();
        filled.provider = "anthropic".to_string();
        let line = serde_json::to_string(&filled).expect("serialize filled row");
        assert!(line.contains("\"model\":\"claude-3-7-sonnet-latest\""), "{line}");
        assert!(line.contains("\"provider\":\"anthropic\""), "{line}");
    }

    /// The family string is the provider registry's own bucket key, not a
    /// second taxonomy invented next to it — if the two disagree the ledger
    /// misattributes cost, which is the whole failure the field exists to stop.
    #[test]
    fn the_recorded_provider_family_is_the_registrys_own_key() {
        let _guard = test_env_lock();
        assert_eq!(
            super::provider_family_for_model(TEST_MODEL),
            crate::ProviderKind::Anthropic.rate_limit_key()
        );
        assert_eq!(super::provider_family_for_model("gpt-5.5"), "openai");
        assert_eq!(super::provider_family_for_model("gemini-2.5-pro"), "google");

        // An id no family claims records nothing rather than guessing, so the
        // row omits the key. (`OLLAMA_BASE_URL` in the environment makes the
        // REGISTRY claim unknown ids for Ollama — that is its documented
        // behavior, not this helper's, so allow it instead of pretending.)
        let unknown = super::provider_family_for_model("not-a-real-model-family");
        assert!(
            unknown.is_empty() || unknown == "ollama",
            "an unknown id must not be attributed to a first-party family: {unknown}"
        );
    }

    /// A fresh private store root that cleans itself up on drop — including
    /// when an assertion unwinds (the previous hand-rolled temp path leaked
    /// its tree on every failing run).
    fn sweep_test_root() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("mkdir root")
    }

    fn mk_session(root: &Path, name: &str) -> std::path::PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir session");
        std::fs::write(dir.join("stats.json"), "{}").expect("stats");
        dir
    }

    /// Age branch: idle directories past retention go; the calling session
    /// (even matched case-insensitively — APFS default volumes fold case, so
    /// `--session-id MyRun` and `myrun` share one directory), plain files,
    /// and symlinked directories all survive, and a planted symlink's target
    /// is never followed.
    #[test]
    fn stale_session_sweep_ages_out_idle_dirs_but_never_current_or_symlinks() {
        let store = sweep_test_root();
        let root = store.path();
        let current = mk_session(root, "Current-Session");
        let idle_a = mk_session(root, "idle-a");
        let idle_b = mk_session(root, "idle-b");
        // A planted symlink to a victim directory OUTSIDE the store root.
        let victim_home = sweep_test_root();
        let victim = victim_home.path();
        std::fs::write(victim.join("keep.txt"), "keep").expect("victim file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(victim, root.join("planted-link")).expect("symlink");

        // A `now` far in the future ages every real mtime past retention —
        // idle sessions go; the current session is exempt regardless of age,
        // including when the caller spells its name in a different case.
        let future = SystemTime::now()
            + Duration::from_secs((super::PROMPT_CACHE_RETENTION_DAYS + 10) * 86_400);
        let outcome = super::sweep_stale_session_dirs(
            root,
            &root.join("current-session"),
            future,
            super::PROMPT_CACHE_MAX_SESSION_DIRS,
        )
        .expect("sweep");
        assert_eq!(outcome.removed, 2, "both idle sessions age out");
        assert_eq!(
            outcome.kept, 1,
            "the spared current session counts as kept (symlinks are not live dirs)"
        );
        assert!(
            current.exists(),
            "the calling session must never be swept (case-insensitive name match)"
        );
        assert!(!idle_a.exists() && !idle_b.exists());
        assert!(
            victim.join("keep.txt").exists(),
            "a planted symlink must not delete its target"
        );
    }

    /// Cap branch — the code path that actually fired in production (the age
    /// branch rarely finds anything on an active machine): oldest-first trim
    /// down to the cap, but NEVER a directory active within the cap-trim
    /// guard window, and never the current session. Exercised with an
    /// injected cap because the production constant cannot be reached with
    /// test-sized fixtures.
    #[test]
    fn stale_session_sweep_cap_trims_oldest_but_spares_recently_active() {
        let store = sweep_test_root();
        let root = store.path();
        let current = mk_session(root, "current");
        for index in 0..4 {
            mk_session(root, &format!("s{index}"));
        }
        // All five non-current dirs share "now" mtimes: within retention AND
        // within the cap-trim guard. Over-cap trimming must remove NOTHING.
        let now = SystemTime::now();
        let outcome =
            super::sweep_stale_session_dirs(root, &current, now, 2).expect("guarded sweep");
        assert_eq!(
            outcome.removed, 0,
            "recently-active dirs are never cap-trimmed even over the cap"
        );
        assert_eq!(outcome.kept, 5, "4 survivors + the calling session");

        // Push `now` past the guard window (but inside retention): the cap
        // may now trim oldest-first down to the injected cap of 2.
        let past_guard = now
            + Duration::from_secs(
                (super::PROMPT_CACHE_CAP_TRIM_MIN_IDLE_DAYS + 1) * 86_400,
            );
        let outcome =
            super::sweep_stale_session_dirs(root, &current, past_guard, 2).expect("cap sweep");
        assert_eq!(outcome.removed, 2, "trim down to the cap, oldest first");
        assert_eq!(outcome.kept, 3, "2 capped survivors + the calling session");
        assert!(current.exists(), "current survives the cap branch too");
    }

    /// An interrupted delete leaves a `.sweeping-` husk whose mtime looks
    /// fresh (child deletions bump it); the next sweep must remove husks
    /// unconditionally instead of letting them squat under the fresh mtime.
    #[test]
    fn stale_session_sweep_finishes_interrupted_husks() {
        let store = sweep_test_root();
        let root = store.path();
        let current = mk_session(root, "current");
        let husk = root.join(format!("{}old-session", super::SWEEPING_PREFIX));
        std::fs::create_dir_all(&husk).expect("husk");
        std::fs::write(husk.join("leftover.json"), "{}").expect("leftover");

        let outcome = super::sweep_stale_session_dirs(
            root,
            &current,
            SystemTime::now(),
            super::PROMPT_CACHE_MAX_SESSION_DIRS,
        )
        .expect("sweep");
        assert_eq!(outcome.removed, 1, "husks are finished regardless of mtime");
        assert!(!husk.exists());
        assert!(current.exists());
    }

    /// The daily marker gates the sweep: a missing marker arms one (touched
    /// BEFORE spawning, then rewritten with the outcome attestation), and a
    /// fresh marker short-circuits without rewriting. Gate behavior is
    /// asserted on marker BYTES (the attestation carries a nanosecond nonce),
    /// not mtime — a 1-second-resolution filesystem cannot distinguish a
    /// rewrite by mtime.
    #[test]
    fn sweep_marker_gates_to_once_per_day() {
        let store = sweep_test_root();
        let root = store.path().to_path_buf();
        let paths = PromptCachePaths {
            session_dir: root.join("current"),
            completion_dir: root.join("current").join("completions"),
            session_state_path: root.join("current").join("session-state.json"),
            stats_path: root.join("current").join("stats.json"),
            breaks_path: root.join("current").join("breaks.jsonl"),
            requests_path: root.join("current").join("requests.jsonl"),
            root: root.clone(),
        };
        let marker = root.join(super::SWEEP_MARKER_FILE);
        assert!(!marker.exists());
        super::maybe_spawn_stale_session_sweep(&paths);
        assert!(marker.exists(), "an armed sweep must leave the daily marker");
        // Wait for the detached sweep thread to write its attestation.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let attested = loop {
            if let Some(outcome) = super::read_sweep_marker(&root) {
                break outcome;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "sweep thread must attest its outcome in the marker"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(attested.removed, 0, "an empty store sweeps nothing");
        let first_bytes = std::fs::read(&marker).expect("marker bytes");

        // Fresh marker → the gate short-circuits: neither a new touch nor a
        // new attestation may change the marker bytes.
        super::maybe_spawn_stale_session_sweep(&paths);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second_bytes = std::fs::read(&marker).expect("marker bytes");
        assert_eq!(
            first_bytes, second_bytes,
            "a fresh marker must gate without rewriting"
        );
    }

    /// Re-arming a STALE marker must bump its mtime without touching its
    /// bytes: the body is the last completed sweep's attestation, and the
    /// old truncate-on-arm left a permanent zero-byte marker whenever the
    /// process exited before its detached sweep thread finished.
    #[test]
    fn arm_sweep_marker_rearms_stale_marker_without_destroying_attestation() {
        let store = sweep_test_root();
        let root = store.path();
        let marker = root.join(super::SWEEP_MARKER_FILE);

        // Missing marker: created empty (nothing to preserve) and armed.
        assert_eq!(super::arm_sweep_marker(&marker), super::SweepMarkerArming::Armed);
        assert_eq!(std::fs::read(&marker).expect("marker").len(), 0);

        // Fresh marker (just created): the gate declines.
        assert_eq!(super::arm_sweep_marker(&marker), super::SweepMarkerArming::Declined);

        // Stale marker carrying a previous attestation: re-armed, bytes kept.
        let attestation = serde_json::to_vec(&super::SweepMarker {
            swept_at_unix_nanos: 42,
            removed: 7,
            kept: 3,
        })
        .expect("attestation json");
        std::fs::write(&marker, &attestation).expect("seed attestation");
        let stale = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&marker)
            .and_then(|file| file.set_modified(stale))
            .expect("age the marker");
        assert_eq!(super::arm_sweep_marker(&marker), super::SweepMarkerArming::Armed);
        assert_eq!(
            std::fs::read(&marker).expect("marker"),
            attestation,
            "re-arming must not destroy the previous attestation"
        );
        let rearmed_mtime =
            std::fs::metadata(&marker).and_then(|meta| meta.modified()).expect("mtime");
        assert!(
            rearmed_mtime > stale + Duration::from_secs(60),
            "re-arming must bump the gate mtime"
        );
        // And the bumped mtime now gates again.
        assert_eq!(super::arm_sweep_marker(&marker), super::SweepMarkerArming::Declined);

        // A planted symlink where the marker belongs is refused outright.
        #[cfg(unix)]
        {
            let linked = root.join("elsewhere");
            std::fs::write(&linked, b"target").expect("target");
            let link = root.join("link-marker");
            std::os::unix::fs::symlink(&linked, &link).expect("symlink");
            assert_eq!(super::arm_sweep_marker(&link), super::SweepMarkerArming::Declined);
            assert_eq!(std::fs::read(&linked).expect("target"), b"target");
        }
    }

    /// The marker positions must be read off the RAW request: `cache_control`
    /// rides a content BLOCK, not the message object, so a top-level-only scan
    /// would record an empty vector on every request and the anchor verdict
    /// would silently answer `None` forever.
    #[test]
    fn breakpoint_indices_see_markers_nested_in_content_blocks() {
        let marked: InputMessage = serde_json::from_value(serde_json::json!({
            "role": "user",
            "content": [{
                "type": "text",
                "text": "anchored",
                "cache_control": {"type": "ephemeral"}
            }]
        }))
        .expect("marked message");
        let request = MessageRequest {
            model: TEST_MODEL.to_string(),
            max_tokens: 64,
            messages: vec![
                InputMessage::user_text("plain"),
                marked,
                InputMessage::user_text("tail"),
            ],
            system: Some(crate::types::system_from_string("system")),
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        };
        assert_eq!(super::cache_breakpoint_indices(&request.messages), vec![1]);
        let fingerprints = super::RequestFingerprints::from_request(&request);
        assert_eq!(fingerprints.cache_breakpoints, vec![1]);
        // The roles ride alongside, one per index and in the same order: the
        // indices say WHERE the markers landed, the roles say whether that
        // placement is the one the anchor policy intends.
        assert_eq!(fingerprints.marker_roles, vec!["user".to_string()]);
    }

    /// Roles stay aligned with indices when several markers land — the pairing
    /// is the whole value, and a filter that walked the messages twice could
    /// drift between them.
    #[test]
    fn marker_roles_line_up_one_per_breakpoint() {
        let marked = |role: &str| -> InputMessage {
            serde_json::from_value(serde_json::json!({
                "role": role,
                "content": [{
                    "type": "text",
                    "text": "marked",
                    "cache_control": {"type": "ephemeral"}
                }]
            }))
            .expect("marked message")
        };
        let messages = vec![
            InputMessage::user_text("plain"),
            marked("user"),
            InputMessage::user_text("between"),
            marked("assistant"),
        ];
        assert_eq!(super::cache_breakpoint_indices(&messages), vec![1, 3]);
        assert_eq!(
            super::marker_roles(&messages),
            vec!["user".to_string(), "assistant".to_string()]
        );
    }

    /// The verdict the whole marker recording exists for: an anchor sitting on
    /// a position the previous request wrote is the healthy shape; an anchor
    /// past everything the previous request marked is the 1,190-row defect.
    #[test]
    fn the_anchor_verdict_separates_a_reused_anchor_from_a_rolled_one() {
        let row = |current: Vec<usize>, previous: Vec<usize>| CacheBreakLedgerRow {
            cache_breakpoints: current,
            prev_cache_breakpoints: previous,
            ..CacheBreakLedgerRow::default()
        };
        assert_eq!(
            row(vec![14, 18], vec![10, 14]).anchor_was_previously_written(),
            Some(true),
            "the anchor is the position the last request rolled onto"
        );
        // A prefix invalidated by the model, the system prompt, or the tool
        // block would have broken wherever the markers sat, so the anchor is
        // not on trial. The first row this ledger ever recorded was exactly
        // that — a `/model` switch — and counting it made the metric read 0%.
        for out_of_scope in [
            CacheBreakLedgerRow {
                model_changed: true,
                cache_breakpoints: vec![6, 7],
                prev_cache_breakpoints: vec![2, 3],
                ..CacheBreakLedgerRow::default()
            },
            CacheBreakLedgerRow {
                system_changed: true,
                cache_breakpoints: vec![6, 7],
                prev_cache_breakpoints: vec![2, 3],
                ..CacheBreakLedgerRow::default()
            },
            CacheBreakLedgerRow {
                tools_changed: true,
                cache_breakpoints: vec![6, 7],
                prev_cache_breakpoints: vec![2, 3],
                ..CacheBreakLedgerRow::default()
            },
        ] {
            assert_eq!(
                out_of_scope.anchor_was_previously_written(),
                None,
                "a break with another cause must not be charged to the anchor"
            );
        }
        assert_eq!(
            row(vec![16, 18], vec![10, 14]).anchor_was_previously_written(),
            Some(false),
            "both markers rolled past anything previously written"
        );
        assert_eq!(
            row(vec![14, 18], Vec::new()).anchor_was_previously_written(),
            None,
            "a row with no previous record cannot answer"
        );
        // A divergence at or before the anchor moved the ground it stood on.
        assert_eq!(
            CacheBreakLedgerRow {
                cache_breakpoints: vec![27, 28],
                prev_cache_breakpoints: vec![23, 25],
                messages_changed: true,
                first_divergence_index: Some(9),
                ..CacheBreakLedgerRow::default()
            }
            .anchor_was_previously_written(),
            None,
            "a divergence under the anchor is not an anchor failure"
        );
        // A divergence ABOVE the anchor leaves it standing, and that is the
        // append case the fix targets — a real verdict.
        assert_eq!(
            CacheBreakLedgerRow {
                cache_breakpoints: vec![14, 18],
                prev_cache_breakpoints: vec![10, 14],
                messages_changed: true,
                first_divergence_index: Some(16),
                ..CacheBreakLedgerRow::default()
            }
            .anchor_was_previously_written(),
            Some(true),
            "a divergence above the anchor still lets the anchor be judged"
        );
        assert_eq!(
            row(Vec::new(), vec![10]).anchor_was_previously_written(),
            None,
            "a row written before markers were recorded cannot answer"
        );
    }

    /// The instrument itself, end to end: markers put on a real request reach
    /// the persisted `breaks.jsonl` row, and that row answers the anchor
    /// question.
    ///
    /// Everything above this test asserts on rows built by hand, so all of it
    /// stays green if the three assignments in `record_usage_internal` that
    /// COPY the markers onto the row are deleted — `zo --doctor` would then
    /// report "not observed yet" forever and read as good news. That is the
    /// exact shape r22 found in the attribution stamp: fields, doc comments and
    /// tests all present, no assignment, and a green suite for the whole life
    /// of the defect. Deleting `cache_breakpoints`, `prev_cache_breakpoints` or
    /// `marker_roles` from the row construction fails this test and only this
    /// test.
    ///
    /// The break is forced the way a real one arrives: a pure tail append whose
    /// cache read collapses. No axis flag is set, nothing diverges under the
    /// anchor, so the row is ELIGIBLE — which is the other half of the pin,
    /// because a recorded marker that never produces a verdict is still a dead
    /// instrument.
    #[test]
    fn a_real_break_row_carries_the_markers_and_answers_the_anchor_question() {
        let _env = test_env_lock();

        // `count` messages, with `cache_control` on exactly the given indices.
        let marked_request = |count: usize, markers: &[usize]| -> MessageRequest {
            let messages = (0..count)
                .map(|index| {
                    let role = if index % 2 == 0 { "user" } else { "assistant" };
                    serde_json::from_value(serde_json::json!({
                        "role": role,
                        "content": [{
                            "type": "text",
                            "text": format!("message {index}"),
                            "cache_control": markers.contains(&index)
                                .then(|| serde_json::json!({"type": "ephemeral"})),
                        }],
                    }))
                    .expect("marked message")
                })
                .collect();
            MessageRequest {
                messages,
                ..sample_request("unused")
            }
        };

        // Rolling shape: the new anchor sits on a position the previous request
        // already marked, so the prefix under it was written and can be read.
        let home = CacheHome::new("anchor-instrument-alive");
        let cache = PromptCache::new(home.session.clone());
        let _ = cache.record_usage(&marked_request(5, &[1, 4]), &plain_usage(0, 50_000, 1));
        let _ = cache.record_usage(&marked_request(7, &[4, 5]), &plain_usage(9_000, 0, 1));

        let rows =
            super::read_break_ledger(&PromptCachePaths::for_session(&home.session).breaks_path);
        assert_eq!(rows.len(), 1, "the read collapse is one break: {rows:?}");
        let row = &rows[0];
        assert_eq!(
            row.cache_breakpoints,
            vec![4, 5],
            "the row must carry where THIS request's markers landed"
        );
        assert_eq!(
            row.prev_cache_breakpoints,
            vec![1, 4],
            "and where the PREVIOUS request's did — the comparison is the metric"
        );
        // The two markers deliberately sit on DIFFERENT roles: a filter that
        // walked the messages twice, or reused the index list, would still line
        // up if both markers happened to land on the same role.
        assert_eq!(
            row.marker_roles,
            vec!["user".to_string(), "assistant".to_string()],
            "roles ride along one per index, in the same order"
        );
        assert!(
            !row.model_changed && !row.system_changed && !row.tools_changed,
            "the fixture must not smuggle in another cause: {row:?}"
        );
        assert_eq!(
            row.anchor_was_previously_written(),
            Some(true),
            "an anchor on a previously written position is the healthy verdict"
        );

        // Same append, but both markers roll past everything the previous
        // request wrote. This is the 1,190-row defect's shape, and the ledger
        // has to be able to say so out loud.
        let rolled = CacheHome::new("anchor-instrument-rolled");
        let cache = PromptCache::new(rolled.session.clone());
        let _ = cache.record_usage(&marked_request(5, &[1, 4]), &plain_usage(0, 50_000, 1));
        let _ = cache.record_usage(&marked_request(9, &[6, 7]), &plain_usage(9_000, 0, 1));
        let rolled_rows =
            super::read_break_ledger(&PromptCachePaths::for_session(&rolled.session).breaks_path);
        assert_eq!(rolled_rows.len(), 1, "{rolled_rows:?}");
        assert_eq!(
            rolled_rows[0].anchor_was_previously_written(),
            Some(false),
            "an anchor past every previously written position is the defect"
        );
    }

    /// "No verdict" must keep saying WHICH no.
    ///
    /// A row that never recorded its markers and a row that recorded them and
    /// found another cause both answer `None`, and the doctor collapsed the
    /// two into "N rows predate marker recording". On the live store that
    /// sentence was already false for 3 of 1,916 rows — the instrument had
    /// started and was reporting itself as not yet started, which is the one
    /// failure mode a health check must not have. Merging the two arms of
    /// `anchor_verdict` back together fails here.
    #[test]
    fn a_recorded_but_ineligible_anchor_is_not_a_row_that_predates_recording() {
        let unrecorded = CacheBreakLedgerRow::default();
        assert_eq!(unrecorded.anchor_verdict(), super::AnchorVerdict::Unrecorded);
        assert!(!unrecorded.has_marker_record());

        // Markers on this request, none on the previous: the comparison has
        // only one side, so it still cannot be made.
        let half = CacheBreakLedgerRow {
            cache_breakpoints: vec![4, 6],
            ..CacheBreakLedgerRow::default()
        };
        assert_eq!(half.anchor_verdict(), super::AnchorVerdict::Unrecorded);
        assert!(
            half.has_marker_record(),
            "the markers ARE recorded even though the verdict cannot be made"
        );

        // Both sides recorded; the break had another cause. The instrument is
        // working and this row is evidence of that, not of its absence.
        for (label, ineligible) in [
            (
                "a tool block change invalidates the prefix wherever markers sit",
                CacheBreakLedgerRow {
                    tools_changed: true,
                    cache_breakpoints: vec![4, 6],
                    prev_cache_breakpoints: vec![2, 4],
                    ..CacheBreakLedgerRow::default()
                },
            ),
            (
                "a divergence under the anchor moved the ground it stood on",
                CacheBreakLedgerRow {
                    messages_changed: true,
                    first_divergence_index: Some(1),
                    cache_breakpoints: vec![4, 6],
                    prev_cache_breakpoints: vec![2, 4],
                    ..CacheBreakLedgerRow::default()
                },
            ),
        ] {
            assert_eq!(
                ineligible.anchor_verdict(),
                super::AnchorVerdict::Ineligible,
                "{label}"
            );
            assert!(ineligible.has_marker_record(), "{label}");
            assert_eq!(
                ineligible.anchor_was_previously_written(),
                None,
                "the Option form still says 'no verdict' — {label}"
            );
        }
    }

    /// The axis taxonomy is what a ranking is built on, so a row that sets
    /// several flags must land in exactly one bucket, and the same one every
    /// time. An append and a divergence differ only by the divergence index —
    /// a distinction a hand-written pipeline got wrong once already.
    #[test]
    fn every_row_lands_in_exactly_one_axis() {
        let row = CacheBreakLedgerRow::default();
        assert_eq!(
            super::break_row_axis(&CacheBreakLedgerRow {
                tools_changed: true,
                messages_changed: true,
                ..row.clone()
            }),
            "tools changed",
            "the tool plane wins attribution when it moved"
        );
        assert_eq!(
            super::break_row_axis(&CacheBreakLedgerRow {
                messages_changed: true,
                ..row.clone()
            }),
            "history appended"
        );
        assert_eq!(
            super::break_row_axis(&CacheBreakLedgerRow {
                messages_changed: true,
                first_divergence_index: Some(3),
                ..row.clone()
            }),
            "history diverged"
        );
        assert_eq!(
            super::break_row_axis(&CacheBreakLedgerRow {
                messages_changed: true,
                messages_truncated: true,
                first_divergence_index: Some(3),
                ..row.clone()
            }),
            "history truncated"
        );
        assert_eq!(
            super::break_row_axis(&CacheBreakLedgerRow {
                unexpected: true,
                ..row
            }),
            "provider side"
        );
    }

    /// The share is basis points over an integer denominator, and it must not
    /// divide by zero on a ledger that has recorded nothing.
    #[test]
    fn the_cache_share_is_exact_and_survives_an_empty_ledger() {
        let summary = super::CacheLedgerSummary {
            cache_read_tokens: 9_000,
            cache_creation_tokens: 500,
            uncached_input_tokens: 500,
            anchor_reused: 3,
            anchor_missed: 1,
            ..super::CacheLedgerSummary::default()
        };
        assert_eq!(summary.cache_read_basis_points(), Some(9_000));
        assert_eq!(summary.anchor_reuse_basis_points(), Some(7_500));
        let empty = super::CacheLedgerSummary::default();
        assert_eq!(empty.cache_read_basis_points(), None);
        assert_eq!(empty.anchor_reuse_basis_points(), None);
    }

    /// Pins the pairing between [`detect_cache_break`]'s reason wording and
    /// [`CacheBreakLedgerRow::no_axis_cause`], with rows built from REAL
    /// detector output rather than hand-written strings — a rewording that
    /// strands the classifier turns this red instead of silently downgrading
    /// every no-axis break to `Unknown` in the doctor.
    /// Every session's numbers must be attributable, break or no break.
    ///
    /// Measured on this machine: a sub-agent session with 13 requests and
    /// **1.22M** fully-billed `total_input_tokens` had zero cache read and zero
    /// creation — and no `breaks.jsonl` at all, because nothing ever broke. So
    /// the one session whose cost most needed explaining was the one with no
    /// record of what it ran on. `cache_creation: 0` is structural on the
    /// OpenAI-compat path (see `total_input_tokens`' own doc) and a defect on
    /// the Anthropic path; without the model there is no way to tell which.
    ///
    /// Ran green for its whole life against code that never wrote the stamp.
    /// It used the fixed session id `attribution-session` and no cache root of
    /// its own, so `PromptCache::new` loaded that directory out of the
    /// developer's real `~/.zo` — a file some earlier build had left with the
    /// model in it — and asserted on that instead of on this run. Measured the
    /// day it was caught: 720 of 720 real session directories on this machine
    /// had `last_model: null`. [`CacheHome`] is the fix: a private root AND a
    /// session id no previous run can have written.
    #[test]
    fn every_request_stamps_what_it_ran_on() {
        let _env = test_env_lock();
        let home = CacheHome::new("attribution");
        let cache = PromptCache::new(home.session.clone());
        let request = sample_request("hello");
        let _ = cache.record_response(&request, &sample_response(2, 2, "x"));

        let stats = cache.stats();
        assert_eq!(
            stats.last_model.as_deref(),
            Some(TEST_MODEL),
            "the model must be stamped on an ordinary request, not only on a break"
        );
        assert_eq!(
            stats.last_provider.as_deref(),
            Some(super::provider_family_for_model(TEST_MODEL)),
            "and the provider family derived from it"
        );
    }

    #[test]
    fn no_axis_cause_classifies_real_detector_reasons() {
        let config = PromptCacheConfig::new("no-axis-cause");
        let base = TrackedPromptState {
            observed_at_unix_secs: 1_000_000,
            fingerprint_version: super::current_fingerprint_version(),
            model_hash: 1,
            system_hash: 2,
            tools_hash: 3,
            messages_hash: 4,
            cache_read_input_tokens: 50_000,
            message_hashes: vec![11, 22],
            message_shapes: Vec::new(),
            tool_names: Vec::new(),
            cache_breakpoints: Vec::new(),
            marker_roles: Vec::new(),
            warm_prefixes: std::collections::BTreeMap::new(),
        };
        let row_for = |event: &super::CacheBreakEvent| CacheBreakLedgerRow {
            seq: 1,
            ts_unix_secs: 0,
            unexpected: event.unexpected,
            reason: event.reason.clone(),
            model_changed: event.model_changed,
            system_changed: event.system_changed,
            tools_changed: event.tools_changed,
            messages_changed: event.messages_changed,
            messages_truncated: event.messages_truncated,
            first_divergence_index: None,
            prefix_stable_messages: 0,
            prev_message_count: 0,
            message_count: 0,
            prev_cache_read: event.previous_cache_read_input_tokens,
            cache_read: event.current_cache_read_input_tokens,
            cache_creation: 0,
            token_drop: event.token_drop,
            elapsed_secs: event.elapsed_secs,
            diverged_message: event.diverged_message.clone(),
            tools_added: event.tools_added.clone(),
            tools_removed: event.tools_removed.clone(),
            model: event.model.clone(),
            provider: event.provider.clone(),
            trimmed_tokens_estimate: None,
            cache_breakpoints: Vec::new(),
            marker_roles: Vec::new(),
            prev_cache_breakpoints: Vec::new(),
        };

        // Provider-side: byte-stable fingerprint, inside the TTL, reads drop.
        let mut cold = base.clone();
        cold.observed_at_unix_secs += 10;
        cold.cache_read_input_tokens = 0;
        let event = super::detect_cache_break(&config, Some(&base), &cold, None, 2, TEST_MODEL)
            .expect("provider-side break");
        assert!(event.unexpected);
        assert_eq!(
            row_for(&event).no_axis_cause(),
            Some(super::NoAxisBreakCause::ProviderSide)
        );

        // TTL expiry: same fingerprint, but the gap exceeds the prompt TTL.
        let mut expired = cold.clone();
        expired.observed_at_unix_secs = base.observed_at_unix_secs
            + config.prompt_ttl.as_secs()
            + 60;
        let event = super::detect_cache_break(&config, Some(&base), &expired, None, 2, TEST_MODEL)
            .expect("ttl break");
        assert!(!event.unexpected);
        assert_eq!(
            row_for(&event).no_axis_cause(),
            Some(super::NoAxisBreakCause::TtlExpiry)
        );

        // Fingerprint bump: our own schema version changed.
        let mut bumped = cold.clone();
        bumped.fingerprint_version = base.fingerprint_version + 1;
        let event = super::detect_cache_break(&config, Some(&base), &bumped, None, 2, TEST_MODEL)
            .expect("fingerprint break");
        assert!(!event.unexpected);
        assert_eq!(
            row_for(&event).no_axis_cause(),
            Some(super::NoAxisBreakCause::FingerprintBump)
        );

        // An axis break carries its own explanation — no cause classification.
        let mut retooled = cold.clone();
        retooled.tools_hash = 99;
        let event = super::detect_cache_break(&config, Some(&base), &retooled, None, 2, TEST_MODEL)
            .expect("tools break");
        assert!(event.tools_changed);
        assert_eq!(row_for(&event).no_axis_cause(), None);
    }

    /// A session that stores no completion must leave no completion directory.
    ///
    /// The store is written only from the non-streaming `send_message`, while
    /// the live turn loop is streaming — so creating the directory per session
    /// produced 548 permanently-empty directories on this machine and made a
    /// feature that never ran look provisioned. Emptiness is the signal; do not
    /// manufacture it.
    #[test]
    fn a_session_that_stores_no_completion_leaves_no_completion_dir() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-emptydir-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);

        let cache = PromptCache::new("no-completion-session");
        let request = sample_request("stream me");
        // The streaming path's recording call: usage only, no response body.
        let _ = cache.record_usage(&request, &sample_response(1, 1, "x").usage);
        let paths = PromptCachePaths::for_session("no-completion-session");
        assert!(
            !paths.completion_dir.exists(),
            "no completion was stored, so nothing should have created its directory"
        );

        // …and the writer still creates it when there IS something to store.
        let _ = cache.record_response(&request, &sample_response(2, 2, "y"));
        assert!(paths.completion_dir.exists());
    }

    #[test]
    fn completion_cache_round_trip_persists_recent_response() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let cache = PromptCache::new("unit-test-session");
        let request = sample_request("cache me");
        let response = sample_response(42, 12, "cached");

        assert!(cache.lookup_completion(&request).is_none());
        let record = cache.record_response(&request, &response);
        assert!(record.cache_break.is_none());

        let cached = cache
            .lookup_completion(&request)
            .expect("cached response should load");
        assert_eq!(cached.content, response.content);

        let stats = cache.stats();
        assert_eq!(stats.completion_cache_hits, 1);
        assert_eq!(stats.completion_cache_misses, 1);
        assert_eq!(stats.completion_cache_writes, 1);

        let persisted = read_json::<super::PromptCacheStats>(&cache.paths().stats_path)
            .expect("stats should persist");
        assert_eq!(persisted.completion_cache_hits, 1);

        std::fs::remove_dir_all(temp_root).expect("cleanup temp root");
        std::env::remove_var("ZO_CONFIG_HOME");
    }

    #[test]
    fn distinct_requests_do_not_collide_in_completion_cache() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-distinct-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let cache = PromptCache::new("distinct-request-session");
        let first_request = sample_request("first");
        let second_request = sample_request("second");

        let response = sample_response(42, 12, "cached");
        let _ = cache.record_response(&first_request, &response);

        assert!(cache.lookup_completion(&second_request).is_none());

        std::fs::remove_dir_all(temp_root).expect("cleanup temp root");
        std::env::remove_var("ZO_CONFIG_HOME");
    }

    #[test]
    fn expired_completion_entries_are_not_reused() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-expired-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let cache = PromptCache::with_config(PromptCacheConfig {
            session_id: "expired-session".to_string(),
            completion_ttl: Duration::ZERO,
            ..PromptCacheConfig::default()
        });
        let request = sample_request("expire me");
        let response = sample_response(7, 3, "stale");

        let _ = cache.record_response(&request, &response);

        assert!(cache.lookup_completion(&request).is_none());
        let stats = cache.stats();
        assert_eq!(stats.completion_cache_hits, 0);
        assert_eq!(stats.completion_cache_misses, 1);

        std::fs::remove_dir_all(temp_root).expect("cleanup temp root");
        std::env::remove_var("ZO_CONFIG_HOME");
    }

    #[test]
    fn distinct_zo_homes_do_not_share_cache_dir() {
        // Regression: base_cache_root() previously ignored ZO_CONFIG_HOME, so
        // two Zo homes sharing one HOME silently shared a legacy cache root.
        // The root must track ZO_CONFIG_HOME so the homes remain isolated.
        let _guard = test_env_lock();
        let prior = std::env::var_os("ZO_CONFIG_HOME");

        std::env::set_var("ZO_CONFIG_HOME", "/tmp/zo-home-a");
        let root_a = base_cache_root();

        std::env::set_var("ZO_CONFIG_HOME", "/tmp/zo-home-b");
        let root_b = base_cache_root();

        assert_ne!(
            root_a, root_b,
            "distinct ZO_CONFIG_HOME values must not share a cache root"
        );
        assert!(
            root_a.starts_with("/tmp/zo-home-a"),
            "cache root must live under ZO_CONFIG_HOME, got {}",
            root_a.display()
        );
        assert!(
            root_b.ends_with("cache/prompt-cache"),
            "cache root must keep its prompt-cache suffix, got {}",
            root_b.display()
        );

        match prior {
            Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
            None => std::env::remove_var("ZO_CONFIG_HOME"),
        }
    }

    #[test]
    fn sanitize_path_caps_long_values() {
        let long_value = "x".repeat(200);
        let sanitized = sanitize_path_segment(&long_value);
        assert!(sanitized.len() <= 80);
    }

    #[test]
    fn request_hashes_are_versioned_and_stable() {
        let request = sample_request("stable");
        let first = request_hash_hex(&request);
        let second = request_hash_hex(&request);
        assert_eq!(first, second);
        assert!(first.starts_with(REQUEST_FINGERPRINT_PREFIX));
    }

    #[test]
    fn env_guarded_tests_use_the_shared_crate_lock() {
        // Regression: previously this module owned a private env lock that did not
        // serialize against crate::test_env_lock, so tests in both could mutate
        // ZO_CONFIG_HOME concurrently. Acquiring the lock here must trip the
        // shared lock's side effect (set on first init), proving we route through
        // the single crate-wide lock instead of a separate private one.
        let _guard = test_env_lock();
        assert_eq!(
            std::env::var("ZO_DISABLE_EXTERNAL_CREDENTIALS").as_deref(),
            Ok("1"),
            "prompt_cache env-guarded tests must hold the shared crate::test_env_lock"
        );
    }

    /// A deletion ahead of the index and an in-place rewrite produce the SAME
    /// `previous[i]` vs `current[i]` comparison, and the ledger read both as a
    /// rewrite. These pin the three answers apart.
    mod divergence_shift {
        use super::super::divergence_shift;

        #[test]
        fn one_message_rewritten_where_it_stands_is_edited() {
            let previous = [1, 2, 3, 4, 5];
            let current = [1, 2, 99, 4, 5];
            assert_eq!(divergence_shift(&previous, &current, 2), Some("edited"));
        }

        #[test]
        fn a_deletion_ahead_of_the_index_reads_as_removed_not_edited() {
            // `current[2]` now holds what `previous[3]` held: a big tool_result
            // "becoming" a small one, with nothing rewritten at all.
            let previous = [1, 2, 3, 4, 5];
            let current = [1, 2, 4, 5];
            assert_eq!(divergence_shift(&previous, &current, 2), Some("removed:1"));
        }

        #[test]
        fn an_insertion_ahead_of_the_index_reads_as_inserted() {
            let previous = [1, 2, 3, 4, 5];
            let current = [1, 2, 99, 3, 4, 5];
            assert_eq!(divergence_shift(&previous, &current, 2), Some("inserted:1"));
        }

        #[test]
        fn two_removed_at_once_is_still_named_exactly() {
            let previous = [1, 2, 3, 4, 5, 6];
            let current = [1, 2, 5, 6];
            assert_eq!(divergence_shift(&previous, &current, 2), Some("removed:2"));
        }

        #[test]
        fn tails_that_share_nothing_stay_unexplained() {
            let previous = [1, 2, 3, 4, 5];
            let current = [1, 2, 91, 92, 93];
            assert_eq!(divergence_shift(&previous, &current, 2), None);
        }

        /// A divergence on the last message has no tail to re-align, so the
        /// only honest reading is that the message itself changed — not a
        /// vacuous "everything after it matches".
        #[test]
        fn a_divergence_at_the_very_end_is_reported_as_edited() {
            let previous = [1, 2, 3];
            let current = [1, 2, 9];
            assert_eq!(divergence_shift(&previous, &current, 2), Some("edited"));
        }

        #[test]
        fn an_index_past_either_history_is_not_guessed_at() {
            assert_eq!(divergence_shift(&[1, 2], &[1, 2, 3], 5), None);
        }
    }

    // --- Prompt-cache forensics: per-message first-divergence index (spec A) ---

    /// The conversation cache-breakpoint markers move to the newest messages
    /// on every request BY DESIGN; the provider prefix cache keys on content,
    /// not markers. The fingerprints must therefore ignore `cache_control`
    /// entirely: two requests whose only difference is marker position hash
    /// identically — no bogus "history diverged at message N" and no break
    /// misfiled as "message payload changed".
    #[test]
    fn moving_cache_breakpoints_do_not_register_as_divergence() {
        with_temp_cache("marker-movement", |cache| {
            let marked = |marker_on: usize| {
                let mut request = sample_request_with_messages(&["one", "two", "three"]);
                let crate::types::InputContentBlock::Text { cache_control, .. } =
                    &mut request.messages[marker_on].content[0]
                else {
                    panic!("expected a Text block");
                };
                *cache_control = Some(crate::types::CacheControl::ephemeral_1h());
                request
            };

            let _ = cache.record_usage(&marked(1), &low_hit_usage());
            // Same content, marker advanced from message 1 to message 2 — the
            // exact shape every follow-up turn produces.
            let record = cache.record_usage(&marked(2), &low_hit_usage());

            assert_eq!(
                record.stats.last_first_divergence_index, None,
                "a moved marker must not read as a mid-history edit"
            );
            assert_eq!(record.stats.last_prefix_stable_messages, 3);
            // And a token drop under a stable content fingerprint must stay
            // classified as UNEXPECTED, not swallowed by "message payload
            // changed" at the marker position.
            if let Some(cache_break) = record.cache_break {
                assert!(
                    !cache_break.reason.contains("message payload changed"),
                    "marker movement leaked into break classification: {}",
                    cache_break.reason
                );
            }
        });
    }

    #[test]
    fn first_divergence_helper_handles_no_previous_and_pure_append() {
        // No previous vector at all (fresh process / first request): nothing
        // to compare, so no divergence is reported.
        assert_eq!(first_divergence(None, &[1, 2, 3]), (None, 0));
        // Pure prefix-preserving extension (ordinary turn growth): the
        // aggregate messages hash would differ, but per-message comparison
        // must still say "no divergence".
        assert_eq!(first_divergence(Some(&[1, 2]), &[1, 2, 3]), (None, 2));
        // A message inside the shared prefix changed.
        assert_eq!(first_divergence(Some(&[1, 2, 3]), &[1, 9, 3]), (Some(1), 1));
    }

    #[test]
    fn divergence_index_is_none_for_pure_append() {
        with_temp_cache("divergence-append", |cache| {
            let first = sample_request_with_messages(&["hello"]);
            let _ = cache.record_usage(&first, &low_hit_usage());

            let second = sample_request_with_messages(&["hello", "world"]);
            let record = cache.record_usage(&second, &low_hit_usage());

            assert_eq!(record.stats.last_first_divergence_index, None);
            assert_eq!(record.stats.last_prefix_stable_messages, 1);
            assert_eq!(record.stats.last_prev_message_count, 1);
            assert_eq!(record.stats.last_message_count, 2);
        });
    }

    #[test]
    fn divergence_index_flags_change_at_message_zero() {
        with_temp_cache("divergence-first", |cache| {
            let first = sample_request_with_messages(&["hello", "world"]);
            let _ = cache.record_usage(&first, &low_hit_usage());

            let second = sample_request_with_messages(&["goodbye", "world"]);
            let record = cache.record_usage(&second, &low_hit_usage());

            assert_eq!(record.stats.last_first_divergence_index, Some(0));
            assert_eq!(record.stats.last_prefix_stable_messages, 0);
        });
    }

    #[test]
    fn divergence_index_flags_change_in_the_middle() {
        with_temp_cache("divergence-middle", |cache| {
            let first = sample_request_with_messages(&["a", "b", "c"]);
            let _ = cache.record_usage(&first, &low_hit_usage());

            let second = sample_request_with_messages(&["a", "changed", "c"]);
            let record = cache.record_usage(&second, &low_hit_usage());

            assert_eq!(record.stats.last_first_divergence_index, Some(1));
            assert_eq!(record.stats.last_prefix_stable_messages, 1);
            assert_eq!(record.stats.last_prev_message_count, 3);
            assert_eq!(record.stats.last_message_count, 3);
        });
    }

    /// Regression: divergence tracking must not depend on a `PromptCache`
    /// instance staying alive between calls. The Anthropic client holds one
    /// long-lived `PromptCache` per session, but the non-Anthropic path
    /// (`record_non_anthropic_prompt_cache_usage` in `crates/runtime`)
    /// constructs a *fresh* `PromptCache::new(session_id)` on every single
    /// call. An earlier version of this instrumentation kept the per-message
    /// hash vector in a process-memory-only field on `PromptCacheInner`,
    /// which silently discarded it between such calls — divergence detection
    /// degraded to always-`None` on that path, and worse, mislabeled a real
    /// mid-history edit as "append-only, no earlier message changed". This
    /// test drives two independent `PromptCache::new()` instances against the
    /// same session id, exactly as the non-Anthropic seam does.
    #[test]
    fn divergence_index_survives_across_fresh_cache_instances() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-fresh-instance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let session_id = "fresh-instance-divergence";

        let first_request = sample_request_with_messages(&["a", "b"]);
        let _ = PromptCache::new(session_id).record_usage(&first_request, &low_hit_usage());

        // A brand-new instance for the second call — never touches the first
        // instance's in-process state, only what it persisted to disk.
        let second_request = sample_request_with_messages(&["a", "changed"]);
        let record = PromptCache::new(session_id).record_usage(&second_request, &low_hit_usage());

        assert_eq!(
            record.stats.last_first_divergence_index,
            Some(1),
            "divergence must be detected purely from persisted state, \
             independent of whether the previous PromptCache instance is still alive"
        );

        std::fs::remove_dir_all(temp_root).expect("cleanup temp root");
        std::env::remove_var("ZO_CONFIG_HOME");
    }

    // --- Prompt-cache forensics: low-cache-hit-ratio streak warning (spec B) ---

    #[test]
    fn low_cache_hit_streak_warns_once_then_resets_on_recovery() {
        with_temp_cache("low-hit-streak", |cache| {
            let request = sample_request_with_messages(&["hello"]);

            let r1 = cache.record_usage(&request, &low_hit_usage());
            assert!(r1.low_cache_hit_warning.is_none());
            assert_eq!(r1.stats.low_cache_hit_streak, 1);

            let r2 = cache.record_usage(&request, &low_hit_usage());
            assert!(r2.low_cache_hit_warning.is_none());
            assert_eq!(r2.stats.low_cache_hit_streak, 2);

            let r3 = cache.record_usage(&request, &low_hit_usage());
            let warning = r3.low_cache_hit_warning.expect("streak of 3 should warn");
            assert!(warning.contains("3 consecutive requests"), "{warning}");
            assert_eq!(r3.stats.low_cache_hit_streak, 3);

            // 4th consecutive low-hit request: the streak keeps counting but
            // must NOT re-warn — an edge trigger, not a level trigger.
            let r4 = cache.record_usage(&request, &low_hit_usage());
            assert!(r4.low_cache_hit_warning.is_none());
            assert_eq!(r4.stats.low_cache_hit_streak, 4);

            // Recovery (a healthy-ratio request) clears the streak.
            let recovered = cache.record_usage(&request, &high_hit_usage());
            assert!(recovered.low_cache_hit_warning.is_none());
            assert_eq!(recovered.stats.low_cache_hit_streak, 0);

            // Relapse after recovery must warn again, not stay permanently
            // silent because the streak already fired once.
            let _ = cache.record_usage(&request, &low_hit_usage());
            let _ = cache.record_usage(&request, &low_hit_usage());
            let relapse = cache.record_usage(&request, &low_hit_usage());
            assert!(
                relapse.low_cache_hit_warning.is_some(),
                "warning should fire again after a recovery + relapse"
            );

            let stats = cache.stats();
            assert_eq!(stats.total_low_cache_hit_requests, 7);
        });
    }

    #[test]
    fn low_cache_hit_streak_ignores_small_requests() {
        // A poor ratio on a small request (below LOW_CACHE_HIT_VOLUME_FLOOR)
        // must not count toward the streak — otherwise short exchanges with
        // naturally little to read from cache would falsely alarm.
        with_temp_cache("low-hit-small", |cache| {
            let request = sample_request_with_messages(&["hi"]);
            let tiny_usage = Usage {
                input_tokens: 100,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 1,
                output_tokens: 5,
                output_tokens_details: None,
            };
            for _ in 0..5 {
                let record = cache.record_usage(&request, &tiny_usage);
                assert!(record.low_cache_hit_warning.is_none());
                assert_eq!(record.stats.low_cache_hit_streak, 0);
            }
        });
    }

    /// Regression, mirroring `divergence_index_survives_across_fresh_cache_instances`:
    /// the streak counters AND the accumulated re-billed-token figure behind
    /// the warning message must survive across independent `PromptCache`
    /// instances, since that is exactly how the non-Anthropic seam calls in
    /// (a fresh instance per request).
    #[test]
    fn low_cache_hit_streak_survives_across_fresh_cache_instances() {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-fresh-instance-streak-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let session_id = "fresh-instance-streak";
        let request = sample_request_with_messages(&["hello"]);

        let r1 = PromptCache::new(session_id).record_usage(&request, &low_hit_usage());
        assert!(r1.low_cache_hit_warning.is_none());
        let r2 = PromptCache::new(session_id).record_usage(&request, &low_hit_usage());
        assert!(r2.low_cache_hit_warning.is_none());
        let r3 = PromptCache::new(session_id).record_usage(&request, &low_hit_usage());
        let warning = r3
            .low_cache_hit_warning
            .expect("streak of 3 should warn even across fresh instances");
        // With a fresh instance per call (the real non-Anthropic pattern), the
        // accumulated token figure must reflect all 3 requests, not just the
        // last one — proving the accumulator is disk-backed, not discarded
        // between calls.
        assert!(
            warning.contains("re-billed ~180k tokens"),
            "expected the cumulative 3x60k re-billed figure, got: {warning}"
        );

        std::fs::remove_dir_all(temp_root).expect("cleanup temp root");
        std::env::remove_var("ZO_CONFIG_HOME");
    }

    // --- Backward compatibility: old stats.json must still deserialize ---

    #[test]
    fn stats_deserializes_from_pre_instrumentation_json() {
        let old_json = r#"{
            "tracked_requests": 10,
            "completion_cache_hits": 2,
            "completion_cache_misses": 3,
            "completion_cache_writes": 4,
            "expected_invalidations": 1,
            "unexpected_cache_breaks": 1,
            "total_cache_creation_input_tokens": 100,
            "total_cache_read_input_tokens": 200,
            "last_cache_creation_input_tokens": 5,
            "last_cache_read_input_tokens": 6,
            "last_request_hash": "v1-deadbeef",
            "last_completion_cache_key": "v1-deadbeef",
            "last_break_reason": "model changed",
            "last_cache_source": "api-response"
        }"#;
        let stats: super::PromptCacheStats = serde_json::from_str(old_json)
            .expect("old-format stats.json (pre-instrumentation) must still deserialize");
        assert_eq!(stats.tracked_requests, 10);
        assert_eq!(stats.last_first_divergence_index, None);
        assert_eq!(stats.last_prefix_stable_messages, 0);
        assert_eq!(stats.last_prev_message_count, 0);
        assert_eq!(stats.last_message_count, 0);
        assert_eq!(stats.low_cache_hit_streak, 0);
        assert_eq!(stats.total_low_cache_hit_requests, 0);
        assert_eq!(stats.low_cache_hit_streak_tokens, 0);
    }

    /// `TrackedPromptState` also gained a field (`message_hashes`) and is
    /// mirrored to `session-state.json` — a resumed session whose
    /// `session-state.json` predates this instrumentation must still load
    /// (as "no basis for comparison yet" rather than failing to deserialize).
    #[test]
    fn tracked_prompt_state_deserializes_from_pre_instrumentation_json() {
        let old_json = r#"{
            "observed_at_unix_secs": 1700000000,
            "fingerprint_version": 1,
            "model_hash": 1,
            "system_hash": 2,
            "tools_hash": 3,
            "messages_hash": 4,
            "cache_read_input_tokens": 5000
        }"#;
        let state: TrackedPromptState = serde_json::from_str(old_json)
            .expect("old-format session-state.json (pre-instrumentation) must still deserialize");
        assert_eq!(state.message_hashes, Vec::<u64>::new());
    }

    // --- Owner-only persistence: fail safely on path-type / symlink surprises ---

    /// A freshly created cache directory and entry file must be owner-only
    /// (`0o700` / `0o600`) so other local users cannot read cached prompts and
    /// responses.
    #[cfg(unix)]
    #[test]
    fn persisted_cache_dir_and_file_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = unique_temp_path("owner-only-dir");
        ensure_private_dir(&dir).expect("create private dir");
        let dir_mode = std::fs::metadata(&dir).expect("dir metadata").permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "cache dir must be owner-only, got {dir_mode:o}");

        let file = dir.join("entry.json");
        write_json(&file, &serde_json::json!({ "k": "v" })).expect("write entry");
        let file_mode =
            std::fs::metadata(&file).expect("file metadata").permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "cache file must be owner-only, got {file_mode:o}");

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A symlink planted where a cache entry belongs must NOT be followed:
    /// `write_json` must fail and leave the symlink's target untouched, so a
    /// hostile link cannot redirect a cache write onto an arbitrary file.
    #[cfg(unix)]
    #[test]
    fn write_json_refuses_to_follow_a_symlink() {
        let dir = unique_temp_path("symlink-guard");
        std::fs::create_dir_all(&dir).expect("create dir");
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, "untouched\n").expect("write victim");
        let link = dir.join("entry.json");
        std::os::unix::fs::symlink(&victim, &link).expect("create symlink");

        let result = write_json(&link, &serde_json::json!({ "k": "v" }));
        assert!(result.is_err(), "write through a symlink must fail, not follow the link");
        assert_eq!(
            std::fs::read_to_string(&victim).expect("read victim"),
            "untouched\n",
            "the symlink target must be left byte-for-byte untouched"
        );

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// If a directory already occupies the cache-entry path, `write_json` must
    /// surface a clear error rather than clobbering or panicking.
    #[test]
    fn write_json_refuses_a_non_file_path() {
        let dir = unique_temp_path("non-file");
        let occupied = dir.join("entry.json");
        std::fs::create_dir_all(&occupied).expect("create dir at entry path");

        let result = write_json(&occupied, &serde_json::json!({ "k": "v" }));
        assert!(result.is_err(), "writing onto a directory path must fail");

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// If a non-directory (here, a plain file) sits where a cache directory
    /// belongs, `ensure_private_dir` must fail rather than treat it as usable.
    #[test]
    fn ensure_private_dir_refuses_a_non_directory() {
        let base = unique_temp_path("non-dir");
        std::fs::create_dir_all(&base).expect("create base");
        let occupied = base.join("cache");
        std::fs::write(&occupied, "not a dir\n").expect("write file at dir path");

        let result = ensure_private_dir(&occupied);
        assert!(result.is_err(), "a file where a directory belongs must fail");

        std::fs::remove_dir_all(&base).expect("cleanup");
    }

    fn unique_temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "prompt-cache-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn with_temp_cache(session_id: &str, body: impl FnOnce(&PromptCache)) {
        let _guard = test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "prompt-cache-{session_id}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);
        let cache = PromptCache::new(session_id);
        body(&cache);
        std::fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        std::env::remove_var("ZO_CONFIG_HOME");
    }

    /// Model id every fixture in this module sends. Shared with the direct
    /// [`detect_cache_break`] calls so the attribution fields under test always
    /// describe the same request the fixtures build.
    const TEST_MODEL: &str = "claude-3-7-sonnet-latest";

    fn sample_request_with_messages(texts: &[&str]) -> MessageRequest {
        MessageRequest {
            model: TEST_MODEL.to_string(),
            max_tokens: 64,
            messages: texts.iter().map(|text| InputMessage::user_text(*text)).collect(),
            system: Some(crate::types::system_from_string("system")),
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        }
    }

    fn low_hit_usage() -> Usage {
        Usage {
            input_tokens: 60_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 100,
            output_tokens: 10,
            output_tokens_details: None,
        }
    }

    fn high_hit_usage() -> Usage {
        Usage {
            input_tokens: 1_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 60_000,
            output_tokens: 10,
            output_tokens_details: None,
        }
    }

    fn sample_request(text: &str) -> MessageRequest {
        MessageRequest {
            model: TEST_MODEL.to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_text(text)],
            system: Some(crate::types::system_from_string("system")),
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        }
    }

    /// [`sample_request`] with a `cache_control` marker on its one message —
    /// the shape `mark_conversation_cache_breakpoints` leaves behind, for the
    /// tests that are about the marker columns rather than about the tokens.
    fn marked_request(text: &str) -> MessageRequest {
        let mut request = sample_request(text);
        request.messages = vec![InputMessage {
            role: "user".to_string(),
            content: vec![crate::types::InputContentBlock::Text {
                text: text.to_string(),
                cache_control: Some(crate::types::CacheControl::ephemeral()),
            }],
            thought_signature: None,
            reasoning_replay: None,
        }];
        request
    }

    fn sample_response(
        cache_read_input_tokens: u32,
        output_tokens: u32,
        text: &str,
    ) -> MessageResponse {
        MessageResponse {
            id: "msg_test".to_string(),
            kind: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputContentBlock::Text {
                text: text.to_string(),
            }],
            model: "claude-3-7-sonnet-latest".to_string(),
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            usage: Usage {
                input_tokens: 10,
                cache_creation_input_tokens: 5,
                cache_read_input_tokens,
                output_tokens,
                output_tokens_details: None,
            },
            request_id: Some("req_test".to_string()),
            thought_signature: None,
            reasoning_replay: None,
            context_management: None,
        }
    }
}
