//! Codex's hook trust contract.
//!
//! Codex is the one agent in this family that will not run a hook just because
//! the hook is in its config. `~/.codex/hooks.json` says what to run;
//! `~/.codex/config.toml` says which of those the user has agreed to. A hook
//! present in the first and absent from the second makes Codex stop and ask
//! ("hooks need review", "press t to trust") — Orca's own terminal-wait detector
//! carries that as a blocked-wait sentinel with the reason
//! `codex-hooks-review-prompt` (out/main/index.js:193169-193190, 1.4.169).
//!
//! **That is why this module exists before the installer does.** Writing a Codex
//! hook without its trust entry does not fail — it stalls the agent on a prompt
//! in a pane nobody is looking at. So the trust state has to be readable and the
//! trusted hash computable before there is any question of writing the hook.
//!
//! ## What is measured here
//!
//! From `codex-app-server-client-DseSy6-0.js` and
//! `managed-agent-hook-controls-Hy3KYvcp.js` (Orca **1.4.169** — the 1.4.164
//! extraction does not carry these chunks, and the version is part of the
//! citation):
//!
//! - The trust key: `<normalized source path>:<event label>:<group>:<handler>`
//!   (`computeTrustKey`, :482-484).
//! - The trusted hash: `sha256:` over canonical JSON of the hook's identity
//!   (`computeTrustedHash`, :465-481). Byte-for-byte, because Codex computes the
//!   same string and compares — an approximation is not a weaker check, it is a
//!   hash that never matches.
//! - Where trust lives: `[hooks.state."<key>"]` tables carrying `trusted_hash`
//!   and `enabled` (`parseHookStateHeaderKey`, :705-708; `readHookTrustBlockState`,
//!   :894-904).
//! - Which events take a matcher, and which must not have one at all
//!   (`matcherPatternForEvent`, :451-463) — the matcher is part of the hashed
//!   identity, so getting this wrong changes the hash.
//!
//! ## What is NOT here, and why
//!
//! **Granting trust.** Orca does not write a trust entry itself. It *removes*
//! any self-computed one (`removeSelfComputedTrustBeforeGrant`, :3751-3758) and
//! asks `codex app-server` over RPC to grant it, then verifies, then caches the
//! grant in a ledger keyed by the Codex binary's stamp. If the grant is
//! unavailable it **rolls the hooks.json entry back** and falls to a mirrored
//! `CODEX_HOME` of its own. That subsystem — an RPC client for a vendor surface,
//! a capability cache, a rollback, and a mirrored home — is ledgered
//! (docs/reverse/orca-ui-inventory.md 1-be) rather than half-built, for the
//! reason above: a Codex hook we cannot trust is worse than no Codex hook.
//!
//! So nothing in this module writes. It reads, and it computes what a writer
//! would need.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The events Orca puts a managed hook on (`CODEX_EVENTS`,
/// managed-agent-hook-controls-Hy3KYvcp.js:4066-4075), with the label each one
/// carries inside the trust identity (`CODEX_HOOK_EVENT_LABEL`, :3029-3040).
///
/// The label, not the event name, is what the hash sees — so this pairing is
/// part of the contract and not a display convenience.
pub const CODEX_EVENTS: [(&str, &str); 8] = [
    ("SessionStart", "session_start"),
    ("UserPromptSubmit", "user_prompt_submit"),
    ("PreToolUse", "pre_tool_use"),
    ("PermissionRequest", "permission_request"),
    ("PostToolUse", "post_tool_use"),
    ("SubagentStart", "subagent_start"),
    ("SubagentStop", "subagent_stop"),
    ("Stop", "stop"),
];

/// Codex's default timeout when a hook does not name one (`computeTrustedHash`
/// uses `entry.timeoutSec ?? 600`). Present so the hash is right for an entry
/// somebody else wrote without a timeout.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 600;

/// One hook handler, as trust identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEntry {
    /// The `hooks.json` this handler is written in.
    pub source_path: PathBuf,
    /// The label — `pre_tool_use`, not `PreToolUse`.
    pub event_label: String,
    /// Which definition in the event's array, and which handler inside it.
    pub group_index: usize,
    pub handler_index: usize,
    pub command: String,
    pub timeout_seconds: Option<u64>,
    pub async_hook: bool,
    /// Only some events carry one, and only when the config sets it.
    pub matcher: Option<String>,
    pub status_message: Option<String>,
}

/// Does this event's identity include a matcher at all?
///
/// `user_prompt_submit` and `stop` return undefined from
/// `matcherPatternForEvent` — for them the key is absent from the hashed object
/// rather than null, which is a different string and therefore a different hash.
pub fn event_takes_matcher(event_label: &str) -> bool {
    !matches!(event_label, "user_prompt_submit" | "stop")
}

/// Normalise a hook file's path the way trust does (`normalizeCodexHookSourcePath`,
/// codex-app-server-client-DseSy6-0.js:493-501).
///
/// POSIX only here: the Windows branch folds device prefixes and separators, and
/// this window does not run there yet. Absolutise, normalise `.`/`..`, and drop
/// trailing separators except on the root — so `/a/b/../c/` and `/a/c` are one
/// key, which they must be or the same hook gets two trust entries.
pub fn normalize_source_path(path: &Path) -> String {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    let absolute = path.is_absolute();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // A `..` above the root has nowhere to go and stays consumed,
                // which is what `path.posix.normalize` does.
                parts.pop();
            }
            std::path::Component::Normal(name) => parts.push(name),
            std::path::Component::RootDir => parts.clear(),
            std::path::Component::Prefix(_) => {}
        }
    }
    let joined = parts
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// The key a trust table is headed by.
pub fn trust_key(entry: &TrustEntry) -> String {
    format!(
        "{}:{}:{}:{}",
        normalize_source_path(&entry.source_path),
        entry.event_label,
        entry.group_index,
        entry.handler_index
    )
}

/// The canonical JSON of a hook's identity, exactly as Codex serialises it.
///
/// Written by hand rather than through serde because the shape is not a Rust
/// struct's — it is `canonicalize()` output: **keys sorted at every level**, and
/// optional keys ABSENT rather than null. `serde_json` with `preserve_order`
/// (which this crate enables, for the installers) would emit insertion order, so
/// building a `Value` and serialising it would produce a different string and
/// therefore a hash that never matches.
///
/// The sorted order for the identity object is `event_name`, `hooks`, `matcher`;
/// for a handler it is `async`, `command`, `statusMessage`, `timeout`, `type`.
fn canonical_identity(entry: &TrustEntry) -> String {
    let mut handler = String::from("{");
    handler.push_str(&format!("\"async\":{},", entry.async_hook));
    handler.push_str(&format!("\"command\":{},", json_string(&entry.command)));
    if let Some(message) = &entry.status_message {
        handler.push_str(&format!("\"statusMessage\":{},", json_string(message)));
    }
    // `Math.max(1, timeoutSec ?? 600)` — a zero or negative timeout in somebody
    // else's config still hashes as 1, so a hook we did not write is recognised.
    let timeout = entry
        .timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .max(1);
    handler.push_str(&format!("\"timeout\":{timeout},"));
    handler.push_str("\"type\":\"command\"}");

    let mut identity = String::from("{");
    identity.push_str(&format!(
        "\"event_name\":{},",
        json_string(&entry.event_label)
    ));
    identity.push_str(&format!("\"hooks\":[{handler}]"));
    if event_takes_matcher(&entry.event_label)
        && let Some(matcher) = &entry.matcher
    {
        identity.push_str(&format!(",\"matcher\":{}", json_string(matcher)));
    }
    identity.push('}');
    identity
}

/// A JSON string literal, with the escapes `JSON.stringify` uses.
///
/// Hand-written for the same reason as the object above: this has to match
/// JavaScript's output byte for byte, including that it escapes the control
/// range as `\u00XX` and does NOT escape non-ASCII.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for glyph in value.chars() {
        match glyph {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The hash a trust entry must carry for this hook to be trusted.
pub fn trusted_hash(entry: &TrustEntry) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_identity(entry).as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// What `config.toml` records about one hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustState {
    /// The hashes this key is trusted for. More than one because a config may
    /// carry several `trusted_hash` lines in the same table — Codex reads them
    /// as a set (`readHookTrustBlockState` collects into `trustedHashes`).
    pub trusted_hashes: Vec<String>,
    /// `enabled = false` turns a hook off even when it is trusted, so trust
    /// alone is not permission to expect it to run.
    pub enabled: Option<bool>,
}

/// Read the trust tables out of a `config.toml`.
///
/// A line-wise read of `[hooks.state."<key>"]` blocks rather than a TOML parse,
/// which is the same choice this repo made for `zerocode.yaml` and for the same
/// reason: a strict reader of the measured shape can only fail to FIND trust,
/// while a general parser that is subtly wrong about a user's file can report
/// trust that is not there — and that failure looks like a working hook until
/// the agent stops on a prompt.
///
/// A missing file is no trust, not an error: most machines have never trusted a
/// hook and that is the normal state.
pub fn read_trust_states(config_path: &Path) -> BTreeMap<String, TrustState> {
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return BTreeMap::new();
    };
    read_trust_states_from(&text)
}

/// Is this line a `[hooks.state."…"]` header, and what key does it name?
///
/// The prefix tolerates spacing inside the brackets, which TOML allows and
/// Orca's own regex accepts (`/^\[[ \t]*hooks[ \t]*\.[ \t]*state[ \t]*\.[ \t]*/`).
pub fn hook_state_header(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let rest = rest.trim_start().strip_prefix("hooks")?;
    let rest = rest.trim_start().strip_prefix('.')?;
    let rest = rest.trim_start().strip_prefix("state")?;
    let rest = rest.trim_start().strip_prefix('.')?;
    let rest = rest.trim_start();
    // The key is a quoted string, or a bare key when it has no special
    // characters. Ours always quote (a path contains `/` and `:`), but somebody
    // else's file may not.
    let (key, after) = if let Some(quoted) = rest.strip_prefix('"') {
        let mut key = String::new();
        let mut chars = quoted.char_indices();
        let mut closed = None;
        while let Some((at, glyph)) = chars.next() {
            match glyph {
                '\\' => {
                    // TOML's basic-string escapes. Only the ones a path or an
                    // event label can contain are unescaped; anything else is
                    // carried through so an unknown escape cannot silently
                    // become a different key.
                    match chars.next().map(|(_, next)| next) {
                        Some('"') => key.push('"'),
                        Some('\\') => key.push('\\'),
                        Some(other) => {
                            key.push('\\');
                            key.push(other);
                        }
                        None => return None,
                    }
                }
                '"' => {
                    closed = Some(at);
                    break;
                }
                other => key.push(other),
            }
        }
        (key, &quoted[closed? + 1..])
    } else {
        let end = rest.find(']')?;
        (rest[..end].trim().to_string(), &rest[end..])
    };
    // The header has to actually close, or this is some other line that happens
    // to start the same way.
    after.trim_start().strip_prefix(']')?;
    Some(key)
}

/// One `key = "value"` line, when the key is the one asked for.
///
/// The tail after the closing quote has to be nothing or a comment.
/// `readHookTrustBlockState`'s regex is anchored at both ends (:904), so
/// `trusted_hash = "…" and then some` is not a hash to Codex — and accepting it
/// here would be reading trust out of a line Codex ignores.
fn quoted_value(line: &str, want: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(want)?;
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let inner = rest.strip_prefix('"')?;
    // Walk the string honouring escapes, so a `\"` does not end it early.
    let mut value = String::new();
    let mut chars = inner.char_indices();
    let closed = loop {
        let (at, glyph) = chars.next()?;
        match glyph {
            '\\' => value.push_str(&unescape(chars.next().map(|(_, next)| next)?)),
            '"' => break at,
            other => value.push(other),
        }
    };
    let tail = inner[closed + 1..].trim_start();
    if !tail.is_empty() && !tail.starts_with('#') {
        return None;
    }
    Some(value)
}

/// TOML's basic-string escapes (`unescapeTomlBasicStringEscape`, :949-958).
///
/// An escape TOML does not define keeps its backslash, so an unknown escape
/// cannot quietly turn into a different value.
fn unescape(next: char) -> String {
    match next {
        'n' => "\n".into(),
        'r' => "\r".into(),
        't' => "\t".into(),
        'b' => "\u{8}".into(),
        'f' => "\u{c}".into(),
        '"' => "\"".into(),
        '\\' => "\\".into(),
        other => format!("\\{other}"),
    }
}

fn bool_value(line: &str, want: &str) -> Option<bool> {
    let rest = line.trim_start().strip_prefix(want)?;
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let word = match rest.strip_prefix("true") {
        Some(tail) => (true, tail),
        None => (false, rest.strip_prefix("false")?),
    };
    let tail = word.1.trim_start();
    if !tail.is_empty() && !tail.starts_with('#') {
        return None;
    }
    Some(word.0)
}

/// Read the trust tables out of `config.toml` text.
///
/// Three rules here exist only because Codex applies them, and getting any of
/// them wrong reports trust the agent will not honour — which does not fail
/// loudly, it stalls the agent on a review prompt in a pane nobody is watching:
///
/// 1. **Only structural lines count.** A `[hooks.state."…"]` written inside a
///    multi-line string value is text. See [`crate::toml_lines`].
/// 2. **`enabled = false` is sticky.** Within a block and across repeated blocks
///    for the same key, once false it stays false
///    (`readHookTrustBlockState`: `enabled !== false && …`, :907).
/// 3. **Two different hashes for one key is a conflict, not a choice.** Codex
///    drops the hash entirely when a key carries more than one
///    (`readHookTrustEntriesFromContent`, :938-941). Treating a matching hash
///    among several as trust would be the reverse of what Codex does.
pub fn read_trust_states_from(text: &str) -> BTreeMap<String, TrustState> {
    let mut states: BTreeMap<String, TrustState> = BTreeMap::new();
    let mut open: Option<String> = None;
    let mut scan = crate::toml_lines::Scan::new();
    for (index, line) in text.lines().enumerate() {
        let bare = if index == 0 {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if !scan.structural() {
            scan = scan.advance(bare);
            continue;
        }
        if let Some(key) = hook_state_header(bare) {
            open = Some(key.clone());
            states.entry(key).or_insert(TrustState {
                trusted_hashes: Vec::new(),
                enabled: None,
            });
            scan = scan.advance(bare);
            continue;
        }
        // Any other table header closes the one we were in. Without this, a
        // `trusted_hash` further down the file would be read as belonging to the
        // last hook table — trust from another section entirely.
        if crate::toml_lines::table_header(bare).is_some() {
            open = None;
            scan = scan.advance(bare);
            continue;
        }
        if let Some(key) = &open
            && let Some(state) = states.get_mut(key)
        {
            if let Some(hash) = quoted_value(bare, "trusted_hash") {
                if !state.trusted_hashes.contains(&hash) {
                    state.trusted_hashes.push(hash);
                }
            } else if let Some(enabled) = bool_value(bare, "enabled") {
                state.enabled = Some(state.enabled != Some(false) && enabled);
            }
        }
        scan = scan.advance(bare);
    }
    states
}

/// Whether Codex will run this hook without asking.
///
/// Both halves have to hold: the key is trusted for exactly this identity's
/// hash, and it has not been switched off. `enabled = false` with a matching
/// hash is a hook the user turned off on purpose, and installing over that would
/// be overruling them.
pub fn is_trusted(entry: &TrustEntry, states: &BTreeMap<String, TrustState>) -> bool {
    let Some(state) = states.get(&trust_key(entry)) else {
        return false;
    };
    matches!(trust_of_state(state, entry), CodexTrust::Trusted)
}

/// Why a Codex hook is not running, in the words the window says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexTrust {
    /// Trusted for this exact hook. It will run.
    Trusted,
    /// The key is there and the hash is for a different hook — the command or
    /// the timeout changed since it was trusted, so Codex will ask again.
    Stale,
    /// Switched off by the user.
    Disabled,
    /// The key carries more than one `trusted_hash`, so Codex uses none of them
    /// — including one that matches. A separate word from `Stale` because the
    /// cause is different and so is the fix: the config has two answers, and
    /// nothing about the hook changed.
    Conflicted,
    /// Never trusted.
    Absent,
}

pub fn trust_of(entry: &TrustEntry, states: &BTreeMap<String, TrustState>) -> CodexTrust {
    match states.get(&trust_key(entry)) {
        Some(state) => trust_of_state(state, entry),
        None => CodexTrust::Absent,
    }
}

fn trust_of_state(state: &TrustState, entry: &TrustEntry) -> CodexTrust {
    if state.enabled == Some(false) {
        return CodexTrust::Disabled;
    }
    if state.trusted_hashes.len() > 1 {
        return CodexTrust::Conflicted;
    }
    if state.trusted_hashes.contains(&trusted_hash(entry)) {
        CodexTrust::Trusted
    } else {
        CodexTrust::Stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(event_label: &str) -> TrustEntry {
        TrustEntry {
            source_path: PathBuf::from("/home/j/.codex/hooks.json"),
            event_label: event_label.into(),
            group_index: 0,
            handler_index: 0,
            command: "/bin/sh '/home/j/.zerocode/agent-hooks/codex-hook.sh'".into(),
            timeout_seconds: Some(10),
            async_hook: false,
            matcher: None,
            status_message: None,
        }
    }

    /// The key is the four parts Codex joins, with the path normalised — or the
    /// same hook acquires two trust entries and neither ever matches.
    #[test]
    fn the_key_is_the_normalised_path_and_the_three_indices() {
        let mut held = entry("pre_tool_use");
        assert_eq!(
            trust_key(&held),
            "/home/j/.codex/hooks.json:pre_tool_use:0:0"
        );
        held.group_index = 2;
        held.handler_index = 1;
        assert_eq!(
            trust_key(&held),
            "/home/j/.codex/hooks.json:pre_tool_use:2:1"
        );

        // And the path folds the way `path.posix.normalize` does.
        for (written, wanted) in [
            ("/a/b/../c/hooks.json", "/a/c/hooks.json"),
            ("/a/./b/hooks.json", "/a/b/hooks.json"),
            ("/a//b/hooks.json", "/a/b/hooks.json"),
            ("/../a/hooks.json", "/a/hooks.json"),
        ] {
            assert_eq!(
                normalize_source_path(Path::new(written)),
                wanted,
                "{written}"
            );
        }
    }

    /// The hash is over canonical JSON: keys sorted at every level, optional
    /// keys ABSENT rather than null.
    ///
    /// This is the assertion that stops the module from being subtly useless. A
    /// hash that is merely "a sha256 of something reasonable" never matches what
    /// Codex computes, and the symptom is an agent stopping on a prompt — not an
    /// error anybody can trace back to here.
    #[test]
    fn the_identity_is_canonical_json_in_sorted_order() {
        let held = entry("pre_tool_use");
        assert_eq!(
            canonical_identity(&held),
            "{\"event_name\":\"pre_tool_use\",\"hooks\":[{\"async\":false,\
             \"command\":\"/bin/sh '/home/j/.zerocode/agent-hooks/codex-hook.sh'\",\
             \"timeout\":10,\"type\":\"command\"}]}"
        );
        assert!(trusted_hash(&held).starts_with("sha256:"));
        assert_eq!(trusted_hash(&held).len(), "sha256:".len() + 64);

        // A matcher joins the object AFTER `hooks`, because `m` sorts after `h`.
        let mut matched = entry("pre_tool_use");
        matched.matcher = Some("Bash".into());
        assert!(
            matched
                .matcher
                .as_ref()
                .is_some_and(|one| canonical_identity(&matched)
                    .ends_with(&format!(",\"matcher\":\"{one}\"}}"))),
            "{}",
            canonical_identity(&matched)
        );

        // A status message sorts between `command` and `timeout`.
        let mut spoken = entry("pre_tool_use");
        spoken.status_message = Some("watching".into());
        assert!(
            canonical_identity(&spoken).contains("\"statusMessage\":\"watching\",\"timeout\""),
            "{}",
            canonical_identity(&spoken)
        );
    }

    /// The hashes, pinned against JavaScript computing the same thing.
    ///
    /// This is the assertion the module stands on. Everything else here checks
    /// that the *shape* is what was measured; this checks that the bytes are what
    /// Codex will actually compare against, and it was verified by transcribing
    /// `canonicalize` + `computeTrustedHash` (:442-481) into node and running
    /// both sides on the same four entries. They agreed on all four, including
    /// the three that are easy to get wrong: `stop` dropping a matcher it was
    /// given, `statusMessage` sorting between `command` and `timeout`, and
    /// `Math.max(1, 0)`.
    ///
    /// Pinned as literals rather than recomputed, because a test that recomputes
    /// with the same code it is testing agrees with itself no matter what.
    #[test]
    fn the_hashes_are_the_ones_javascript_produces() {
        let of =
            |label: &str, matcher: Option<&str>, timeout: Option<u64>, message: Option<&str>| {
                let held = TrustEntry {
                    source_path: PathBuf::from("/home/j/.codex/hooks.json"),
                    event_label: label.into(),
                    group_index: 0,
                    handler_index: 0,
                    command: "/bin/sh '/h/.zerocode/agent-hooks/codex-hook.sh'".into(),
                    timeout_seconds: timeout,
                    async_hook: false,
                    matcher: matcher.map(str::to_string),
                    status_message: message.map(str::to_string),
                };
                trusted_hash(&held)
            };
        assert_eq!(
            of("pre_tool_use", Some("Bash"), Some(10), None),
            "sha256:7b61584811b8d3fee58b598d98886834d5e167810e7e07f80be73712312972b2"
        );
        // Given a matcher, and must not hash it.
        assert_eq!(
            of("stop", Some("Bash"), Some(10), None),
            "sha256:2f44de9c07bdbbd9ccbb9fc3ff1650d268ad53d7363390ef768b2127fd4baed2"
        );
        // No timeout (so 600), and a status message in the middle of the sort.
        assert_eq!(
            of("session_start", None, None, Some("watching")),
            "sha256:ceb315a4d6bd0585e5c062ed831abb78755a12ac46b296a777efc1c951adcbb5"
        );
        // A zero timeout (so 1) and a matcher carrying non-ASCII and a quote.
        assert_eq!(
            of("post_tool_use", Some("에이전트\"x"), Some(0), None),
            "sha256:611271dc5a8035d3835f20477e7824b0303121b620c67402fb68785afc60fc37"
        );
    }

    /// Two events carry no matcher at all, and for them the key must be ABSENT
    /// from the hashed object — not null, not empty. A different string is a
    /// different hash.
    #[test]
    fn the_events_with_no_matcher_omit_the_key_entirely() {
        for label in ["user_prompt_submit", "stop"] {
            let mut held = entry(label);
            held.matcher = Some("Bash".into());
            assert!(
                !event_takes_matcher(label),
                "{label} was given a matcher it does not take"
            );
            assert!(
                !canonical_identity(&held).contains("matcher"),
                "{label} put a matcher in its identity: {}",
                canonical_identity(&held)
            );
            // And the hash is the same as it would be without one, which is the
            // property that actually matters.
            let mut bare = entry(label);
            bare.matcher = None;
            assert_eq!(trusted_hash(&held), trusted_hash(&bare));
        }
        // Every other measured event does take one.
        for (_, label) in CODEX_EVENTS {
            if label == "user_prompt_submit" || label == "stop" {
                continue;
            }
            assert!(event_takes_matcher(label), "{label}");
        }
    }

    /// A missing timeout hashes as Codex's default, and a nonsense one as 1 —
    /// so a hook somebody else wrote is still recognised by its hash.
    #[test]
    fn a_missing_timeout_takes_codexs_default_and_a_zero_takes_one() {
        let mut none = entry("stop");
        none.timeout_seconds = None;
        assert!(
            canonical_identity(&none).contains(&format!("\"timeout\":{DEFAULT_TIMEOUT_SECONDS}")),
            "{}",
            canonical_identity(&none)
        );
        let mut zero = entry("stop");
        zero.timeout_seconds = Some(0);
        assert!(
            canonical_identity(&zero).contains("\"timeout\":1"),
            "{}",
            canonical_identity(&zero)
        );
    }

    /// A command with a quote or a newline in it still produces valid JSON, the
    /// way `JSON.stringify` would.
    #[test]
    fn a_command_is_escaped_the_way_javascript_escapes_it() {
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
        // Non-ASCII is NOT escaped, which is what JSON.stringify does — escaping
        // it would change the byte string and the hash.
        assert_eq!(json_string("에이전트"), "\"에이전트\"");
    }

    /// The trust tables are read out of a real config, including one that has
    /// other sections around them.
    #[test]
    fn trust_is_read_from_the_tables_that_carry_it() {
        let config = r#"
model = "gpt-5"

[hooks.state."/home/j/.codex/hooks.json:pre_tool_use:0:0"]
trusted_hash = "sha256:aaa"
enabled = true

[hooks.state."/home/j/.codex/hooks.json:stop:0:0"]
trusted_hash = "sha256:bbb"
trusted_hash = "sha256:ccc"
enabled = false

[other.section]
trusted_hash = "sha256:not-a-hook"
"#;
        let states = read_trust_states_from(config);
        assert_eq!(states.len(), 2, "{states:?}");
        let first = &states["/home/j/.codex/hooks.json:pre_tool_use:0:0"];
        assert_eq!(first.trusted_hashes, vec!["sha256:aaa"]);
        assert_eq!(first.enabled, Some(true));
        let second = &states["/home/j/.codex/hooks.json:stop:0:0"];
        // More than one hash in a table is a set, not a last-wins.
        assert_eq!(second.trusted_hashes, vec!["sha256:bbb", "sha256:ccc"]);
        assert_eq!(second.enabled, Some(false));
        // The section after must not have leaked its hash into the hook above
        // it. Without the "any other header closes the block" rule, trust from
        // an unrelated part of the file counts as a hook's.
        assert!(
            !second
                .trusted_hashes
                .iter()
                .any(|one| one.contains("not-a-hook")),
            "trust leaked across a section boundary: {second:?}"
        );
    }

    /// A file that is not there is no trust, not an error — which is the normal
    /// state of every machine that has never trusted a hook.
    #[test]
    fn a_config_that_does_not_exist_is_simply_no_trust() {
        let states = read_trust_states(Path::new("/nonexistent/zerocode/config.toml"));
        assert!(states.is_empty());
        assert!(!is_trusted(&entry("stop"), &states));
        assert_eq!(trust_of(&entry("stop"), &states), CodexTrust::Absent);
    }

    /// The five answers, and the ones that look alike and are not: a hash for a
    /// DIFFERENT hook is stale, a matching hash that was switched off is
    /// disabled, and a matching hash among several is no hash at all. Installing
    /// over any of them would be wrong in a different way.
    #[test]
    fn the_five_answers_are_told_apart() {
        let held = entry("pre_tool_use");
        let key = trust_key(&held);
        let good = trusted_hash(&held);

        let mut states = BTreeMap::new();
        states.insert(
            key.clone(),
            TrustState {
                trusted_hashes: vec![good.clone()],
                enabled: Some(true),
            },
        );
        assert_eq!(trust_of(&held, &states), CodexTrust::Trusted);
        assert!(is_trusted(&held, &states));

        // `enabled` absent is still trusted — Codex's default is on, and a
        // config that only records the hash is the common shape.
        states.get_mut(&key).expect("the entry").enabled = None;
        assert_eq!(trust_of(&held, &states), CodexTrust::Trusted);

        // The command changed since it was trusted.
        let mut moved = held.clone();
        moved.command = "/bin/sh '/somewhere/else.sh'".into();
        assert_eq!(trust_of(&moved, &states), CodexTrust::Stale);
        assert!(!is_trusted(&moved, &states));
        // So did a timeout change, which is the one easy to overlook: the hash
        // covers it, so bumping our own timeout un-trusts every hook.
        let mut slower = held.clone();
        slower.timeout_seconds = Some(30);
        assert_eq!(trust_of(&slower, &states), CodexTrust::Stale);

        // A second hash next to the good one. Codex uses neither, so a reader
        // that answers Trusted here promises a hook that will stall on review.
        states.get_mut(&key).expect("the entry").trusted_hashes =
            vec![good.clone(), "sha256:someone-elses".into()];
        assert_eq!(trust_of(&held, &states), CodexTrust::Conflicted);
        assert!(
            !is_trusted(&held, &states),
            "a matching hash among several was treated as trust"
        );

        states.get_mut(&key).expect("the entry").trusted_hashes = vec![good];
        states.get_mut(&key).expect("the entry").enabled = Some(false);
        assert_eq!(trust_of(&held, &states), CodexTrust::Disabled);
        assert!(
            !is_trusted(&held, &states),
            "a hook the user switched off was treated as runnable"
        );
    }

    /// Three ways a config can claim trust that Codex does not honour. All three
    /// fail in the same direction — reporting a hook as runnable when the agent
    /// will stop and ask — which is the failure that hides, because it looks like
    /// a working install until a pane nobody is watching goes quiet.
    #[test]
    fn a_config_cannot_talk_us_into_trust_codex_would_not_give() {
        let key = "/a/hooks.json:stop:0:0";

        // 1. A trust table written inside a multi-line string value is text.
        let smuggled = read_trust_states_from(concat!(
            "notify = \"\"\"\n",
            "[hooks.state.\"/a/hooks.json:stop:0:0\"]\n",
            "trusted_hash = \"sha256:forged\"\n",
            "\"\"\"\n",
        ));
        assert!(
            smuggled.is_empty(),
            "trust was read out of a string value: {smuggled:?}"
        );

        // 2. `enabled = false` is sticky. A later `true` in the same table does
        //    not switch it back on.
        let toggled = read_trust_states_from(
            "[hooks.state.\"/a/hooks.json:stop:0:0\"]\nenabled = false\nenabled = true\n",
        );
        assert_eq!(toggled[key].enabled, Some(false));

        // 3. A line with anything after the closing quote is not a value.
        let junk = read_trust_states_from(
            "[hooks.state.\"/a/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:x\" oops\n",
        );
        assert!(
            junk[key].trusted_hashes.is_empty(),
            "a malformed line was read as a hash: {junk:?}"
        );

        // And the shapes that ARE legal still read: a comment after the value,
        // and an escape inside it.
        let fine = read_trust_states_from(concat!(
            "[hooks.state.\"/a/hooks.json:stop:0:0\"]\n",
            "trusted_hash = \"sha256:y\"  # approved 2026-01-01\n",
            "enabled = true # on\n",
        ));
        assert_eq!(fine[key].trusted_hashes, vec!["sha256:y"]);
        assert_eq!(fine[key].enabled, Some(true));
    }

    /// A key written with TOML spacing, or with escapes in it, still resolves —
    /// this file belongs to the user and they may have edited it by hand.
    #[test]
    fn a_header_written_by_hand_still_resolves() {
        let states = read_trust_states_from(
            "[ hooks . state . \"/a/hooks.json:stop:0:0\" ]\ntrusted_hash = \"sha256:x\"\n",
        );
        assert_eq!(
            states.keys().collect::<Vec<_>>(),
            vec!["/a/hooks.json:stop:0:0"],
            "{states:?}"
        );
        // A quoted key holding an escaped quote.
        let odd = read_trust_states_from(
            "[hooks.state.\"/a/od\\\"d/hooks.json:stop:0:0\"]\nenabled = false\n",
        );
        assert_eq!(
            odd.keys().collect::<Vec<_>>(),
            vec!["/a/od\"d/hooks.json:stop:0:0"],
            "{odd:?}"
        );
        // And a line that merely starts like a header is not one.
        assert!(read_trust_states_from("[hooks.state.\"unterminated\n").is_empty());
    }
}
