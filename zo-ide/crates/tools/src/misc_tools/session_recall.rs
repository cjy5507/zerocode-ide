//! `session_recall` — cross-session selective read (read-only).
//!
//! Lets the model pull context out of a PRIOR conversation without resuming it:
//!  - **recall** a specific session (`session_ref`), optionally filtered by a
//!    substring `query`, a `role`, and/or a `last_n` tail; or
//!  - **search** across every saved session (omit `session_ref`, pass `query`)
//!    to find which past chat discussed something, with per-session match counts.
//!
//! It never touches the live session — it loads transcripts from disk via
//! `runtime::session_control` and returns only the matched slice as text. This is
//! the selective counterpart to `/resume`, which swaps in an entire prior
//! conversation wholesale.
//!
//! Recall also surfaces the **compaction vault**: the raw messages summarized
//! out of a session are recovered from its append-only `<id>.vault.jsonl`
//! sidecar and folded into the result (tagged `[evicted …]`). That makes
//! compaction non-destructive in practice — the model can pull back an exact
//! detail that was summarized away, which a lossy summary alone cannot provide.
//!
//! The vault holds a second kind of record too: pre-clear ORIGINALS that
//! microcompact seals before it blanks a tool-result body in place. Those turns
//! are still live, so they are not listed as evicted — they replace the blanked
//! live copy (see [`healed_live_messages`]), which extends the same guarantee
//! to detail a cheap trim removed, not just to detail a full compaction did.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{ToolContext, ToolError};
use runtime::session::{ContentBlock, ConversationMessage, MessageRole, Session};
use runtime::session_control::{
    is_session_reference_alias, list_managed_sessions_for, load_managed_session_for_excluding,
    ManagedSessionSummary,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SessionRecallInput {
    /// Which prior session to read: a session id, an alias ("latest" / "last"
    /// / "recent"), or "current" (THIS session — used to pull raw originals a
    /// compaction round sealed to the vault). In a live session, aliases skip the
    /// current session when the current session id is available; "current" does
    /// not skip it. Omit it (or pass "all" / "*") together with `query` to SEARCH
    /// across every saved session instead of reading one.
    pub session_ref: Option<String>,
    /// Case-insensitive substring to match against message text. Required in
    /// search mode; in recall mode, absent means "return the tail".
    pub query: Option<String>,
    /// Restrict to a single role: "user", "assistant", "tool", or "system".
    pub role: Option<String>,
    /// Return only the last N matching messages (most recent kept).
    pub last_n: Option<usize>,
    /// Lower bound (inclusive) of the seq window to recall. Seqs are one
    /// monotonic domain across a session: an evicted vault record's `vault_seq`
    /// and a live message's absolute index (`first_message_index + i`) share it,
    /// so a `[seq_from, seq_to]` window addresses both. Combined with query/role
    /// (logical AND).
    pub seq_from: Option<u32>,
    /// Upper bound (inclusive) of the seq window to recall. `seq_from > seq_to`
    /// is rejected.
    pub seq_to: Option<u32>,
    /// Include tool-result blocks in the rendered output (default true). Set
    /// false to omit them — a message left with no other content is skipped, and
    /// the omission is noted in the header.
    pub include_tool_results: Option<bool>,
    /// SEARCH-mode only: keep sessions modified within the last N days (a recent
    /// lower bound on the window). Fractional days are honored (0.5 = last 12h).
    /// Applied as a prefilter over session mtimes before the scan cap, so the
    /// window also cuts how many transcripts are re-loaded.
    pub since_days: Option<f64>,
    /// SEARCH-mode only: keep sessions modified more than N days ago (an older
    /// upper bound on the window). Pair with `since_days` to bracket a span, e.g.
    /// `since_days: 7, before_days: 2` = "2 to 7 days ago". A window that asks for
    /// sessions both newer than `since_days` and older than `before_days` days
    /// (`since_days < before_days`) is empty and rejected.
    pub before_days: Option<f64>,
}

// Caps — all surfaced in the output when hit, never silently truncated.
const DEFAULT_RECALL_TAIL: usize = 30; // messages returned when neither query nor last_n is given
const MAX_RECALL_MESSAGES: usize = 200; // hard ceiling on recalled messages
const MAX_SEARCH_SESSIONS: usize = 100; // sessions scanned in search mode
const MAX_RENDERED_CHARS: usize = 4000; // per-message render cap
const SNIPPET_CHARS: usize = 160; // search-result snippet length

pub(crate) fn run_session_recall(
    input: SessionRecallInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let base_dir = recall_base_dir(ctx);
    let role = match input.role.as_deref() {
        Some(r) => Some(parse_role(r)?),
        None => None,
    };
    if input.last_n == Some(0) {
        return Err(ToolError::InvalidInput(
            "session_recall: last_n must be >= 1".to_owned(),
        ));
    }
    if let (Some(from), Some(to)) = (input.seq_from, input.seq_to) {
        if from > to {
            return Err(ToolError::InvalidInput(format!(
                "session_recall: seq_from ({from}) must be <= seq_to ({to})"
            )));
        }
    }
    let include_tool_results = input.include_tool_results.unwrap_or(true);
    let query_lc = input
        .query
        .as_deref()
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());

    let is_search = input
        .session_ref
        .as_deref()
        .map(str::trim)
        .is_none_or(|r| r.is_empty() || r == "all" || r == "*");

    // Time filters narrow which SESSIONS the search scans; a single-session
    // recall already targets one transcript by id/alias, so they have no meaning
    // there — reject rather than silently ignore.
    if !is_search && (input.since_days.is_some() || input.before_days.is_some()) {
        return Err(ToolError::InvalidInput(
            "session_recall: since_days/before_days apply to search mode only (omit `session_ref`)"
                .to_owned(),
        ));
    }
    validate_time_window(input.since_days, input.before_days)?;

    if is_search {
        let query = query_lc.ok_or_else(|| {
            ToolError::InvalidInput(
                "session_recall: provide `query` to search across sessions, or `session_ref` \
                 to recall a specific one"
                    .to_owned(),
            )
        })?;
        run_search(&base_dir, &query, role, input.since_days, input.before_days)
    } else {
        let reference = input.session_ref.unwrap_or_default();
        let reference = reference.trim();
        validate_session_ref(reference)?;
        // "current" resolves to THIS session (no exclusion) so the model can pull
        // back raw originals the just-finished compaction round sealed to this
        // session's vault — the recall affordance the continuation message emits.
        // Every other alias keeps EXCLUDING the current session.
        let (resolved_reference, exclude_session_id) = if reference.eq_ignore_ascii_case("current") {
            let id = ctx.session_id().ok_or_else(|| {
                ToolError::InvalidInput(
                    "session_recall: no active persisted session — pass an explicit session id"
                        .to_owned(),
                )
            })?;
            (id, None)
        } else {
            let exclude = is_session_reference_alias(reference)
                .then(|| ctx.session_id())
                .flatten();
            (reference.to_owned(), exclude)
        };
        run_recall(
            &base_dir,
            &resolved_reference,
            query_lc.as_deref(),
            role,
            input.last_n,
            exclude_session_id.as_deref(),
            input.seq_from,
            input.seq_to,
            include_tool_results,
        )
    }
}

/// Confine recall to a session id or alias resolved under the project's own
/// `.zo/sessions`. Reject path-like references (separators, absolute paths,
/// or `..`) so a model cannot aim the loader at a transcript outside the
/// workspace — unlike `read_file`, this tool has no other boundary check, and
/// transcripts routinely contain pasted secrets. Ids are discoverable via
/// search mode, so dropping raw-path support costs the model nothing.
fn validate_session_ref(reference: &str) -> Result<(), ToolError> {
    let path_like = reference.is_empty()
        || reference.contains('/')
        || reference.contains('\\')
        || reference.contains("..")
        // A bare filename with an extension ("incident.jsonl") has no separators
        // but is still a path the loader would honor, reaching any transcript in
        // the workspace (and its vault). Zo session ids and the aliases have
        // no dots, so rejecting `.` confines recall to real ids/aliases.
        || reference.contains('.')
        || std::path::Path::new(reference).is_absolute();
    if path_like {
        return Err(ToolError::InvalidInput(format!(
            "session_recall: `{reference}` is not a valid session reference — use a session id \
             or \"latest\"/\"last\"/\"recent\" (run a search to discover ids); paths are not allowed"
        )));
    }
    Ok(())
}

/// The directory whose `.zo/sessions` holds the transcripts. Honors the
/// agent's pinned cwd / workspace root (worktree isolation), falling back to the
/// process cwd so a plain run still finds the project's sessions.
fn recall_base_dir(ctx: &ToolContext) -> PathBuf {
    ctx.cwd
        .clone()
        .or_else(|| ctx.workspace_root.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The absolute seq of the live message at local index `index`. Used for the
/// `[seq_from, seq_to]` window — and, for the one case that has no stable id,
/// as the gated fallback address of a sealed standalone image (see
/// [`healed_live_messages`]).
fn live_seq(first_live_seq: u32, index: usize) -> u32 {
    first_live_seq.saturating_add(u32::try_from(index).unwrap_or(u32::MAX))
}

/// A tool result's raw `(output, images)` borrowed out of a vault record —
/// borrowed rather than owned because the heal copies it at most once, into the
/// single live block that needs it.
type SealedToolBody<'v> = (&'v str, &'v [(String, String)]);

/// Sealed bodies addressed the way the heal addresses them: by `tool_use_id`.
type SealedToolBodies<'v> = std::collections::HashMap<&'v str, SealedToolBody<'v>>;

/// Every non-placeholder tool-result body among the sealed pre-clear
/// `originals`, indexed by `tool_use_id`. Placeholder bodies are skipped rather
/// than indexed, so a record that itself caught an earlier round's clear cannot
/// "heal" a live block back to the same placeholder. First writer wins — the
/// same rule the runtime's `index_raw_tool_bodies` applies, and since
/// `read_vault` returns records ascending by seq (already collapsed last-wins
/// per seq), that is the lowest seq carrying the id.
fn sealed_tool_bodies<'v>(
    originals: &[&'v runtime::session::VaultRecord],
) -> SealedToolBodies<'v> {
    let mut bodies: SealedToolBodies<'v> = std::collections::HashMap::new();
    for record in originals {
        for block in &record.message.blocks {
            if let ContentBlock::ToolResult {
                tool_use_id,
                output,
                images,
                ..
            } = block
            {
                if output != runtime::MICROCOMPACT_PLACEHOLDER {
                    bodies
                        .entry(tool_use_id.as_str())
                        .or_insert((output.as_str(), images.as_slice()));
                }
            }
        }
    }
    bodies
}

/// The sealed pre-clear originals that actually carry an `Image` block, keyed by
/// seq. Only this narrow case needs a positional key: a standalone user-pasted
/// image has no `tool_use_id` to address it by (see the SCOPE note on the
/// runtime's `seal_microcompact_originals`), so recovering it has to fall back
/// to "what stood at seq N". Restricting the map to records that hold an image
/// is one gate of three that keep that fallback from rendering the wrong turn's
/// content; the other two are applied per candidate in [`healed_live_messages`],
/// because they compare the record against the live turn it would replace.
fn sealed_image_originals<'v>(
    originals: &[&'v runtime::session::VaultRecord],
) -> std::collections::HashMap<u32, &'v ConversationMessage> {
    originals
        .iter()
        .filter(|record| {
            record
                .message
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }))
        })
        .map(|record| (record.vault_seq, &record.message))
        .collect()
}

/// The vault holds two kinds of record on one seq axis, and recall must not
/// conflate them.
///
/// Below `first_live_seq` are messages compaction EVICTED — gone from the
/// transcript, so they are shown in their own right, tagged `[evicted …]`.
/// At or above it are pre-clear ORIGINALS that `microcompact_session` sealed
/// before blanking a body in place: those messages are STILL LIVE, so listing
/// them as evicted would show the same turn twice. This returns the healed form
/// of each live message a sealed original can restore — keyed by LOCAL live
/// index, for callers to substitute for the blanked copy, which is what recall
/// is for since the reader is asking for the detail that was trimmed away.
///
/// The heal is CONTENT-ADDRESSED (by `tool_use_id`), exactly like the runtime's
/// `restore_microcompacted_bodies`, and that is the load-bearing choice: the two
/// stampers on the seq axis disagree by one. Microcompact seals at
/// `first_message_index + local_index`, and on the one compaction round whose
/// `compacted_prefix_len` is 0 (a session's first) `apply_compaction` inserts a
/// synthetic continuation at live index 0 while `first_message_index` advances
/// only by the removed count — so a preserved-tail message's seq gains +1 that
/// its already-written vault record does not. Later rounds add nothing further
/// (the continuation they insert replaces one they evicted, so the tail's seq and
/// `first_message_index` move together), which makes the drift exactly +1, once,
/// and permanent. A positional heal would therefore render one turn's sealed body
/// in the PREVIOUS turn's slot and leave the blanked turn blank forever. Matching
/// on the id is immune to that drift.
///
/// Only blocks that are actually the placeholder AND whose id was sealed are
/// replaced, so this never rewrites a live message that was never blanked.
fn healed_live_messages(
    vault: &[runtime::session::VaultRecord],
    first_live_seq: u32,
    live: &[ConversationMessage],
) -> std::collections::HashMap<usize, ConversationMessage> {
    let originals: Vec<&runtime::session::VaultRecord> = vault
        .iter()
        .filter(|record| record.vault_seq >= first_live_seq)
        .collect();
    if originals.is_empty() {
        return std::collections::HashMap::new();
    }
    let bodies = sealed_tool_bodies(&originals);
    let images = sealed_image_originals(&originals);
    let mut healed = std::collections::HashMap::new();
    for (index, message) in live.iter().enumerate() {
        // The standalone-image fallback is POSITIONAL, so the seq it computes is
        // treated as a candidate, never as an answer. Three things must hold
        // before a sealed record is rendered in this turn's slot:
        //
        // 1. the live turn really holds the image placeholder, and the record at
        //    that seq really holds an `Image` (the two-sided gate that keeps the
        //    fallback off every other kind of turn);
        // 2. the record is structurally a pre-clear original OF THIS TURN
        //    (`runtime::is_pre_clear_original_of` — same role, same blocks, modulo
        //    the blocks a clear blanks); and
        // 3. the record is not equally a pre-clear original of the NEXT turn, the
        //    only other turn the +1 axis drift could hand it to.
        //
        // Gate 1 alone was not enough, and the gap it left was not "fails to
        // heal" but "heals with someone else's content": the drift documented
        // above is established at the FIRST full compaction and is permanent, so
        // after any compaction this lookup systematically resolves to the NEXT
        // turn's record, and whenever that neighbour happened to carry an image
        // its message was rendered here — destroying this turn's own text and
        // leaving the true owner blanked. Gates 2 and 3 make the failure mode
        // "still blanked" instead, which is what the reader had before any heal
        // existed. Two adjacent turns that are byte-identical once blanked (two
        // bare pasted images in a row, no caption) are therefore both left
        // blanked rather than guessed at — the axes cannot distinguish them, and
        // that is the whole point of refusing.
        let from_image_seal = if message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text == runtime::MICROCOMPACT_IMAGE_PLACEHOLDER)
        }) {
            images
                .get(&live_seq(first_live_seq, index))
                .copied()
                .filter(|sealed| runtime::is_pre_clear_original_of(sealed, message))
                .filter(|sealed| {
                    !live
                        .get(index + 1)
                        .is_some_and(|next| runtime::is_pre_clear_original_of(sealed, next))
                })
        } else {
            None
        };
        let has_sealed_body = message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::ToolResult { tool_use_id, output, .. }
                if output == runtime::MICROCOMPACT_PLACEHOLDER
                    && bodies.contains_key(tool_use_id.as_str()))
        });
        if from_image_seal.is_none() && !has_sealed_body {
            continue; // nothing sealed for this turn — leave the live copy alone
        }
        // Clone only what is actually healed. The image case starts from the
        // sealed record (the whole pre-clear message, images included); the
        // tool-result swap then still runs over it, so a record that itself
        // carries an earlier round's placeholder is healed too.
        let mut candidate = from_image_seal.unwrap_or(message).clone();
        for block in &mut candidate.blocks {
            let ContentBlock::ToolResult {
                tool_use_id,
                output,
                images: block_images,
                ..
            } = block
            else {
                continue;
            };
            if output != runtime::MICROCOMPACT_PLACEHOLDER {
                continue;
            }
            if let Some((sealed_output, sealed_images)) = bodies.get(tool_use_id.as_str()) {
                (*sealed_output).clone_into(output);
                *block_images = (*sealed_images).to_vec();
            }
        }
        healed.insert(index, candidate);
    }
    healed
}

#[allow(clippy::too_many_arguments)]
fn run_recall(
    base_dir: &std::path::Path,
    reference: &str,
    query_lc: Option<&str>,
    role: Option<MessageRole>,
    last_n: Option<usize>,
    exclude_session_id: Option<&str>,
    seq_from: Option<u32>,
    seq_to: Option<u32>,
    include_tool_results: bool,
) -> Result<String, ToolError> {
    let loaded = load_managed_session_for_excluding(base_dir, reference, exclude_session_id)
        .map_err(|error| ToolError::Execution(format!("session_recall: {error}")))?;
    let session = loaded.session;
    // Raw messages compacted out of this session, recovered from the append-only
    // vault — the context Claude Code permanently loses on compaction. They are
    // chronologically older than the live transcript (which now holds only the
    // summary + recent tail), so they lead the combined view, letting the model
    // pull back an exact detail that was summarized away.
    let vault = session.read_vault();
    // Live messages address the same monotonic seq domain the vault records use:
    // a live message at local index `i` has absolute seq `first_message_index + i`.
    // That frontier is what splits the vault into the two record kinds: below it
    // are EVICTED messages (listed in their own right), at or above it are
    // pre-clear ORIGINALS of turns that are still live (which heal the blanked
    // live copy instead of being listed — see [`healed_live_messages`]).
    let first_live_seq = session.first_message_index();
    let evicted_total = vault
        .iter()
        .filter(|record| record.vault_seq < first_live_seq)
        .count();
    let healed = healed_live_messages(&vault, first_live_seq, &session.messages);
    let total = evicted_total + session.messages.len();

    let seq_in_range = |seq: u32| {
        seq_from.is_none_or(|from| seq >= from) && seq_to.is_none_or(|to| seq <= to)
    };
    let matches_filters = |msg: &ConversationMessage| {
        role.is_none_or(|want| msg.role == want)
            && query_lc.is_none_or(|q| message_plain(msg).to_lowercase().contains(q))
    };

    // Recovered originals are ALWAYS surfaced (the whole point of recall) —
    // never dropped by the recent-tail cap, which only ever applied to live
    // messages. They lead the output, tagged `[evicted …]`.
    let mut evicted: Vec<&ConversationMessage> = vault
        .iter()
        .filter(|record| record.vault_seq < first_live_seq && seq_in_range(record.vault_seq))
        .map(|record| &record.message)
        .filter(|msg| matches_filters(msg))
        .collect();

    // Live transcript. When the vault recovered the originals, the index-0
    // compaction summary is the LOSSY restatement of those same messages —
    // suppress it so the model is not handed both the raw batch and its summary.
    let suppress_summary = evicted_total > 0;
    let mut live: Vec<&ConversationMessage> = session
        .messages
        .iter()
        .enumerate()
        .filter(|(index, msg)| {
            !(suppress_summary && *index == 0 && msg.role == MessageRole::System)
                && seq_in_range(live_seq(first_live_seq, *index))
        })
        // Prefer the sealed pre-clear original over the live, body-blanked copy
        // of the same turn. The swap happens before `matches_filters` so a query
        // matches the text the reader is actually shown.
        .map(|(index, msg)| healed.get(&index).unwrap_or(msg))
        .filter(|msg| matches_filters(msg))
        .collect();

    let matched_count = evicted.len() + live.len();

    // The recent-tail / `last_n` cap applies to the LIVE transcript only. With
    // no `last_n` and no query a default tail keeps an un-narrowed recall from
    // dumping the whole transcript; `last_n` (>= 1, validated) is clamped to the
    // hard ceiling.
    let live_limit = match (last_n, query_lc.is_some()) {
        (Some(n), _) => n.min(MAX_RECALL_MESSAGES),
        (None, false) => DEFAULT_RECALL_TAIL,
        (None, true) => MAX_RECALL_MESSAGES,
    };
    let mut notes = String::new();
    if live.len() > live_limit {
        notes = match last_n {
            Some(n) if n > MAX_RECALL_MESSAGES => {
                format!(" (live: requested last {n}, capped at {MAX_RECALL_MESSAGES})")
            }
            Some(n) => format!(" (live: showing last {n})"),
            None if query_lc.is_some() => format!(" (live: capped at {MAX_RECALL_MESSAGES})"),
            None => format!(" (live: showing last {DEFAULT_RECALL_TAIL})"),
        };
        let cut = live.len() - live_limit;
        live.drain(..cut);
    }
    // Bound the recovered set too (keep the most-recent = highest seq) so a
    // pathologically large vault can't dump unbounded.
    if evicted.len() > MAX_RECALL_MESSAGES {
        let cut = evicted.len() - MAX_RECALL_MESSAGES;
        evicted.drain(..cut);
        let _ = write!(notes, " (evicted: showing last {MAX_RECALL_MESSAGES})");
    }

    let filt = describe_filter(query_lc, role, seq_from, seq_to);
    let recovered = if evicted_total > 0 {
        format!(
            " · {} message(s) recovered from the compaction vault",
            evicted.len()
        )
    } else {
        String::new()
    };
    let omitted_note = if include_tool_results {
        ""
    } else {
        " · tool results omitted from render"
    };
    let mut out = format!(
        "Recalled from session `{}` — {matched_count} of {total} message(s) match{filt}{notes}{recovered}{omitted_note}.\n",
        loaded.handle.id,
    );
    if evicted.is_empty() && live.is_empty() {
        out.push_str("\n(no messages matched)");
        return Ok(out);
    }
    // Render each message, skipping any left with no content once tool results
    // are omitted (a tool-result-only message renders to nothing).
    for msg in &evicted {
        if let Some(rendered) = render_message(msg, true, include_tool_results) {
            out.push('\n');
            out.push_str(&rendered);
            out.push('\n');
        }
    }
    for msg in &live {
        if let Some(rendered) = render_message(msg, false, include_tool_results) {
            out.push('\n');
            out.push_str(&rendered);
            out.push('\n');
        }
    }
    Ok(out)
}

fn run_search(
    base_dir: &std::path::Path,
    query_lc: &str,
    role: Option<MessageRole>,
    since_days: Option<f64>,
    before_days: Option<f64>,
) -> Result<String, ToolError> {
    let mut summaries = list_managed_sessions_for(base_dir)
        .map_err(|error| ToolError::Execution(format!("session_recall: {error}")))?;
    // `list_managed_sessions_for` already drops adjacent same-(mtime,id)
    // duplicates (e.g. its doubled search dir), but two distinct files sharing
    // one internal session_id with differing mtimes — a legacy `<id>.json` plus a
    // migrated `<id>.jsonl` — are not adjacent after its (mtime desc) sort and
    // both survive. Dedup by id here so such a session is searched once.
    let mut seen = std::collections::BTreeSet::new();
    summaries.retain(|s| seen.insert(s.id.clone()));

    // Apply the time window on session mtimes BEFORE the scan cap so it also cuts
    // how many transcripts are re-loaded below: an out-of-window session is
    // dropped here instead of consuming a `Session::load_from_path`. `since_days`
    // is a recent lower bound (kept when modified >= now − since_days), `before_days`
    // an older upper bound (kept when modified <= now − before_days).
    let now_ms = now_epoch_millis();
    let since_cutoff = since_days.map(|days| now_ms.saturating_sub(days_to_millis(days)));
    let before_cutoff = before_days.map(|days| now_ms.saturating_sub(days_to_millis(days)));
    summaries.retain(|summary| {
        since_cutoff.is_none_or(|lo| summary.modified_epoch_millis >= lo)
            && before_cutoff.is_none_or(|hi| summary.modified_epoch_millis <= hi)
    });

    let total_sessions = summaries.len();
    let truncated = total_sessions > MAX_SEARCH_SESSIONS;
    summaries.truncate(MAX_SEARCH_SESSIONS);

    let hits = search_hits(&summaries, query_lc, role, true);

    let role_note = role
        .map(|r| format!(", role={}", role_label(r)))
        .unwrap_or_default();
    let window_note = describe_time_window(since_days, before_days);
    let scanned = summaries.len();
    let mut out = format!(
        "Searched {scanned} session(s) for \"{query_lc}\"{role_note}{window_note} — {} with matches (most recent first):\n",
        hits.len(),
    );
    if truncated {
        let _ = writeln!(out, "{}", truncation_note(total_sessions, before_days));
    }
    if hits.is_empty() {
        out.push_str("\nNo saved session matched. Try a broader query.");
        return Ok(out);
    }
    for (id, count, snippet) in &hits {
        let _ = write!(out, "\n• `{id}` — {count} match(es): {snippet}");
    }
    let _ = write!(
        out,
        "\n\nRecall full context with session_recall {{\"session_ref\":\"<id>\",\"query\":\"{query_lc}\"}}.",
    );
    Ok(out)
}

/// The sessions among `summaries` holding a message that matches `query_lc`
/// (and `role`): id, match count, first snippet, in the order given. With
/// `skip_unmatchable` a transcript is parsed only when
/// [`transcript_may_contain`] cannot rule it out; the answer is the same
/// either way (pinned in tests) — only the parses differ.
fn search_hits(
    summaries: &[ManagedSessionSummary],
    query_lc: &str,
    role: Option<MessageRole>,
    skip_unmatchable: bool,
) -> Vec<(String, usize, String)> {
    let mut hits: Vec<(String, usize, String)> = Vec::new();
    for summary in summaries {
        if skip_unmatchable && !transcript_may_contain(&summary.path, query_lc) {
            continue; // certain: no message in this session can hold the query
        }
        let Ok(session) = Session::load_from_path(&summary.path) else {
            continue; // skip unreadable/corrupt transcripts (best-effort search)
        };
        // Search the compaction vault too, so a term that was summarized out of a
        // session is still discoverable — otherwise the lossless recovery is
        // invisible on the primary discovery path. Same split as `run_recall`
        // (see [`healed_live_messages`]): evicted records are searched
        // alongside the live messages, while a pre-clear original heals its
        // still-live copy, which makes text a microcompact blanked findable
        // without counting the same turn twice.
        let vault = session.read_vault();
        let first_live_seq = session.first_message_index();
        let healed = healed_live_messages(&vault, first_live_seq, &session.messages);
        let mut count = 0usize;
        let mut snippet = String::new();
        for msg in vault
            .iter()
            .filter(|record| record.vault_seq < first_live_seq)
            .map(|record| &record.message)
            .chain(
                session
                    .messages
                    .iter()
                    .enumerate()
                    .map(|(index, msg)| healed.get(&index).unwrap_or(msg)),
            )
        {
            if !role.is_none_or(|want| msg.role == want) {
                continue;
            }
            let plain = message_plain(msg);
            if plain.to_lowercase().contains(query_lc) {
                count += 1;
                if snippet.is_empty() {
                    snippet = make_snippet(&plain, query_lc);
                }
            }
        }
        if count > 0 {
            hits.push((summary.id.clone(), count, snippet));
        }
    }
    hits
}

/// Whether the transcript at `path` — or its compaction vault — can hold a
/// message whose plain text contains `query_lc`. A "no" is certain and lets
/// the search skip the parse; a "yes" only means "look".
///
/// Sound because a searched message's plain text is its string values
/// (text, tool name and input, tool output, image media type) joined by the
/// seams in [`PLAIN_SEAMS`], and the writer renders every string value
/// verbatim except for the characters it escapes (`"`, `\`, controls). So a
/// query that carries none of those and cannot touch a seam (see
/// [`query_is_seam_safe`]) matches a message only where it matches a single
/// string value — which the lowercased raw file then contains as well. The
/// vault is scanned too: evicted records, and the originals that heal
/// microcompact-blanked live messages, come from there.
fn transcript_may_contain(path: &std::path::Path, query_lc: &str) -> bool {
    if !query_is_seam_safe(query_lc) {
        return true;
    }
    let vault = runtime::session::vault_path_for(path);
    for file in [Some(path), vault.exists().then_some(vault.as_path())]
        .into_iter()
        .flatten()
    {
        match std::fs::read_to_string(file) {
            Ok(raw) => {
                if lowercase(raw).contains(query_lc) {
                    return true;
                }
            }
            // Unreadable here is unreadable to the loader too; let it decide.
            Err(_) => return true,
        }
    }
    false
}

/// `to_lowercase` without the second allocation when the text is ASCII —
/// the two agree on ASCII, and English transcripts are mostly that.
fn lowercase(mut raw: String) -> String {
    if raw.is_ascii() {
        raw.make_ascii_lowercase();
        raw
    } else {
        raw.to_lowercase()
    }
}

/// What [`message_plain`] puts around and between the string values it joins.
const PLAIN_SEAMS: &[&str] = &[
    "[tool call: ",
    "] ",
    "[tool result: ",
    "[tool error: ",
    "[image: ",
    "]",
    "[thinking]",
    "[redacted thinking]",
    "\n",
];

/// Whether a match for `query_lc` in a message's plain text can only lie
/// inside one string value. A match that touches a seam either contains the
/// seam's first or last character (`[`, `]`, newline), lies wholly inside the
/// seam, or starts inside it — beginning with a proper suffix of it — and runs
/// on into the value after it. Characters the writer escapes cannot be matched
/// in raw bytes either, so they also make a query unsafe to prefilter.
fn query_is_seam_safe(query_lc: &str) -> bool {
    if query_lc
        .chars()
        .any(|c| matches!(c, '"' | '\\' | '[' | ']' | '\n') || c.is_control())
    {
        return false;
    }
    !PLAIN_SEAMS.iter().any(|seam| {
        seam.contains(query_lc)
            || (1..seam.len())
                .filter(|cut| seam.is_char_boundary(*cut))
                .any(|cut| query_lc.starts_with(&seam[cut..]))
    })
}

const MILLIS_PER_DAY: f64 = 86_400_000.0;

/// Reject a nonsensical time window up front (search mode). Each bound must be a
/// finite, non-negative day count; `since_days < before_days` asks for sessions
/// both newer than `since_days` and older than `before_days` days — an empty
/// window (mirrors the `seq_from > seq_to` guard).
fn validate_time_window(since_days: Option<f64>, before_days: Option<f64>) -> Result<(), ToolError> {
    for (label, value) in [("since_days", since_days), ("before_days", before_days)] {
        if let Some(days) = value {
            if !days.is_finite() || days < 0.0 {
                return Err(ToolError::InvalidInput(format!(
                    "session_recall: {label} ({days}) must be a finite, non-negative number of days"
                )));
            }
        }
    }
    if let (Some(since), Some(before)) = (since_days, before_days) {
        if since < before {
            return Err(ToolError::InvalidInput(format!(
                "session_recall: since_days ({since}) < before_days ({before}) is an empty window \
                 — asks for sessions both newer than {since} and older than {before} days"
            )));
        }
    }
    Ok(())
}

/// Now as epoch milliseconds, matching `ManagedSessionSummary.modified_epoch_millis`.
/// A clock before the epoch (unreachable in practice) yields 0, which disables the
/// lower bound rather than panicking.
fn now_epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

/// Convert a (validated finite, non-negative) day count to milliseconds, rounded.
/// `validate_time_window` already rejected negative/non-finite inputs and the
/// `.max(0.0)` floors any residue, so the cast neither wraps a negative nor
/// truncates meaningfully (day counts are far below `u128`'s range).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn days_to_millis(days: f64) -> u128 {
    (days * MILLIS_PER_DAY).round().max(0.0) as u128
}

/// A short header note describing the active time window, or empty when neither
/// bound is set. `since_days` is the recent edge, `before_days` the older edge.
/// The note printed when the scan hit `MAX_SEARCH_SESSIONS`, naming both what
/// was skipped and the one field that reaches it.
///
/// The old note said only how many were scanned. In real use that read as a
/// completed search: 16 production searches printed it, and ten of those
/// returned the searching session itself as their only hit — the answer was in
/// the unscanned remainder every time. `before_days` is the sole way to move
/// the window backwards, and it was used in 0 of 937 calls, which is what a
/// capability nobody is told about looks like. Naming it at the exact moment
/// it becomes useful is cheaper than any amount of tool-description prose.
///
/// Silent when the caller already set `before_days`: they are steering the
/// window themselves, and repeating the advice would be noise.
fn truncation_note(total_sessions: usize, before_days: Option<f64>) -> String {
    let skipped = total_sessions.saturating_sub(MAX_SEARCH_SESSIONS);
    if before_days.is_some() {
        return format!(
            "(scanned the {MAX_SEARCH_SESSIONS} most recent of {total_sessions} sessions in this window)"
        );
    }
    format!(
        "(scanned the {MAX_SEARCH_SESSIONS} most recent of {total_sessions} sessions —          {skipped} older session(s) were NOT searched. To search them, re-run with          `before_days: <N>` to move the window back past the newest N days.)"
    )
}

fn describe_time_window(since_days: Option<f64>, before_days: Option<f64>) -> String {
    match (since_days, before_days) {
        (Some(since), Some(before)) => {
            format!(", modified {before}–{since} day(s) ago")
        }
        (Some(since), None) => format!(", modified within the last {since} day(s)"),
        (None, Some(before)) => format!(", modified more than {before} day(s) ago"),
        (None, None) => String::new(),
    }
}

fn parse_role(role: &str) -> Result<MessageRole, ToolError> {
    match role.trim().to_lowercase().as_str() {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "tool" => Ok(MessageRole::Tool),
        "system" => Ok(MessageRole::System),
        other => Err(ToolError::InvalidInput(format!(
            "session_recall: unknown role '{other}' (use user/assistant/tool/system)"
        ))),
    }
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn describe_filter(
    query_lc: Option<&str>,
    role: Option<MessageRole>,
    seq_from: Option<u32>,
    seq_to: Option<u32>,
) -> String {
    let mut parts = Vec::new();
    if let Some(q) = query_lc {
        parts.push(format!("query \"{q}\""));
    }
    if let Some(r) = role {
        parts.push(format!("role {}", role_label(r)));
    }
    match (seq_from, seq_to) {
        (Some(from), Some(to)) => parts.push(format!("seq {from}-{to}")),
        (Some(from), None) => parts.push(format!("seq >= {from}")),
        (None, Some(to)) => parts.push(format!("seq <= {to}")),
        (None, None) => {}
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// Flatten a message's content blocks into searchable/renderable plain text.
/// Tool calls/results and images are condensed to short tags so the search still
/// matches their text without dumping base64 or huge JSON.
fn message_plain(msg: &ConversationMessage) -> String {
    message_plain_with(msg, true)
}

/// Like [`message_plain`], but omits `ToolResult` blocks when
/// `include_tool_results` is false (used by render when the caller asked to drop
/// tool output). With `include_tool_results = true` the output is byte-identical
/// to [`message_plain`], so search/query matching is unchanged.
fn message_plain_with(msg: &ConversationMessage, include_tool_results: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in &msg.blocks {
        match block {
            ContentBlock::Text { text } => parts.push(text.clone()),
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(format!("[tool call: {name}] {input}"));
            }
            ContentBlock::ToolResult {
                tool_name,
                output,
                is_error,
                ..
            } => {
                if !include_tool_results {
                    continue;
                }
                let tag = if *is_error {
                    "tool error"
                } else {
                    "tool result"
                };
                parts.push(format!("[{tag}: {tool_name}] {output}"));
            }
            ContentBlock::Image { media_type, .. } => {
                parts.push(format!("[image: {media_type}]"));
            }
            // Reasoning blocks are internal; surface only a marker in recall text.
            ContentBlock::Thinking { .. } => parts.push("[thinking]".to_owned()),
            ContentBlock::RedactedThinking { .. } => parts.push("[redacted thinking]".to_owned()),
        }
    }
    parts.join("\n")
}

/// Render one recalled message, or `None` when omitting tool results leaves it
/// with no content (a tool-result-only message asked to drop tool output).
fn render_message(
    msg: &ConversationMessage,
    evicted: bool,
    include_tool_results: bool,
) -> Option<String> {
    let plain = message_plain_with(msg, include_tool_results);
    if plain.trim().is_empty() {
        return None;
    }
    let body = truncate_chars(&plain, MAX_RENDERED_CHARS);
    // Tag vault-recovered originals so the model knows it is reading the exact
    // pre-compaction content, not the lossy summary that replaced it.
    let tag = if evicted { "evicted " } else { "" };
    Some(format!("[{tag}{}] {body}", role_label(msg.role)))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}… [truncated]")
    }
}

/// A one-line snippet of `plain` centered on the (case-insensitive) match, for
/// search results. The match position is found in the lowercased text, then
/// mapped back to a char offset in the original — because `to_lowercase` is not
/// 1:1 (e.g. Turkish 'İ' expands to two chars), slicing the original at the
/// lowercased offset would mis-center or drop the match. Char-based throughout,
/// so it never splits a UTF-8 boundary; preserves the original casing.
fn make_snippet(plain: &str, query_lc: &str) -> String {
    let collapsed: String = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let total = collapsed.chars().count();
    let lc = collapsed.to_lowercase();
    let Some((byte, _)) = lc.match_indices(query_lc).next() else {
        // Match spanned whitespace the collapse removed (rare) — show the head.
        return one_line_window(&collapsed, 0, total);
    };
    let lc_match_char = lc[..byte].chars().count();
    // Map the lowercased char offset back onto `collapsed`: walk its chars,
    // accumulating how many lowercased chars each produces, until we reach the
    // match's lowercased offset.
    let mut lc_seen = 0usize;
    let mut match_char = total;
    for (idx, ch) in collapsed.chars().enumerate() {
        if lc_seen >= lc_match_char {
            match_char = idx;
            break;
        }
        lc_seen += ch.to_lowercase().count();
    }
    let start = match_char.saturating_sub(SNIPPET_CHARS / 4);
    one_line_window(&collapsed, start, total)
}

/// A `SNIPPET_CHARS`-wide char window of `text` starting at char `start`, with
/// leading/trailing ellipses when content is elided.
fn one_line_window(text: &str, start: usize, total: usize) -> String {
    let window: String = text.chars().skip(start).take(SNIPPET_CHARS).collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if total > start + SNIPPET_CHARS {
        "…"
    } else {
        ""
    };
    format!("{prefix}{window}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn msg(role: MessageRole, text: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            blocks: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        }
    }

    /// A tool-result message — the shape microcompact blanks in place and the
    /// vault heals by `tool_use_id` (never by position).
    fn tool_msg(tool_use_id: &str, output: &str) -> ConversationMessage {
        ConversationMessage::tool_result(tool_use_id, "read_file", output, false)
    }

    /// Write a session transcript under `<base>/.zo/sessions/<id>.jsonl`.
    fn write_session(base: &std::path::Path, id: &str, messages: Vec<ConversationMessage>) {
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let mut session = Session::new();
        session.session_id = id.to_owned();
        session.messages = Arc::new(messages);
        session
            .save_to_path(dir.join(format!("{id}.jsonl")))
            .expect("save session");
    }

    fn temp_base(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("zo-g19-{tag}-{unique}"));
        std::fs::create_dir_all(&base).expect("mk base");
        base
    }

    fn ctx_for(base: &std::path::Path) -> ToolContext {
        ToolContext::new().with_cwd(base.to_path_buf())
    }

    #[test]
    fn recall_filters_by_query_and_role() {
        let base = temp_base("recall");
        write_session(
            &base,
            "sess-A",
            vec![
                msg(MessageRole::User, "how do I fix the parser bug"),
                msg(MessageRole::Assistant, "the parser bug is in tokenize()"),
                msg(MessageRole::User, "thanks, unrelated chatter"),
            ],
        );
        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-A".into()),
                query: Some("parser".into()),
                role: Some("assistant".into()),
                last_n: None,
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(out.contains("session `sess-A`"), "{out}");
        assert!(
            out.contains("tokenize()"),
            "matched assistant line present: {out}"
        );
        assert!(
            !out.contains("unrelated chatter"),
            "non-matching filtered out: {out}"
        );
        assert!(
            !out.contains("how do I fix"),
            "wrong role filtered out: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn recall_without_query_returns_tail() {
        let base = temp_base("tail");
        let many: Vec<_> = (0..40)
            .map(|i| msg(MessageRole::User, &format!("line {i}")))
            .collect();
        write_session(&base, "sess-tail", many);
        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-tail".into()),
                query: None,
                role: None,
                last_n: Some(3),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");
        assert!(out.contains("line 39") && out.contains("line 37"), "{out}");
        assert!(!out.contains("line 36"), "only last 3: {out}");
        assert!(out.contains("showing last 3"), "tail note: {out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn recall_recovers_evicted_messages_from_the_vault() {
        // A compacted session: the on-disk transcript holds only a summary, but
        // the raw original was sealed to the vault. Recall must surface the raw
        // content (tagged evicted) — the detail CC would have lost.
        let base = temp_base("vault-recall");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let session_path = dir.join("sess-vault.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-vault".to_owned();
        let session = session.with_persistence_path(session_path.clone());
        let _ = session.seal_evicted_to_vault(&[msg(
            MessageRole::User,
            "the exact error was ECONNREFUSED on db/pool.rs:42",
        )]);
        let mut session = session;
        session.messages = Arc::new(vec![msg(
            MessageRole::System,
            "Summary: discussed a database connection issue",
        )]);
        // `record_compaction` is what makes this an EVICTED record rather than a
        // microcompact pre-clear original: it advances `first_message_index`
        // past the sealed seq, which is the boundary recall splits the vault on.
        session.record_compaction("Summary: discussed a database connection issue", 1);
        session.save_to_path(&session_path).expect("save");

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-vault".into()),
                query: Some("ECONNREFUSED".into()),
                role: None,
                last_n: None,
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(
            out.contains("ECONNREFUSED on db/pool.rs:42"),
            "raw evicted content recovered from vault: {out}"
        );
        assert!(out.contains("evicted"), "recovered content is tagged: {out}");
        assert!(
            out.contains("recovered from the compaction vault"),
            "header notes vault recovery: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The seq window the compaction advertises must actually address the vault.
    ///
    /// After a compaction the continuation message hands the model a
    /// ready-to-paste call —
    /// `{"session_ref": "current", "seq_from": X, "seq_to": Y}` — built from the
    /// span `Session::seal_evicted_to_vault` returned
    /// (`runtime::compact::compact_session`). Both ends were tested and the
    /// joint was not: the compaction side asserts the sentence names a span,
    /// and this module's own vault tests recall with **no** seq window at all.
    /// If the two numberings ever disagree the model's one door back to the
    /// pre-compaction original returns nothing, and nothing anywhere goes red.
    #[test]
    fn the_seq_span_sealing_returns_is_the_span_that_recalls_it() {
        let base = temp_base("vault-seq-span");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let session_path = dir.join("sess-span.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-span".to_owned();
        let session = session.with_persistence_path(session_path.clone());
        let span = session
            .seal_evicted_to_vault(&[
                msg(MessageRole::User, "VAULT_ALPHA the first evicted turn"),
                msg(MessageRole::Assistant, "VAULT_BETA the answer to it"),
            ])
            .expect("sealing a non-empty eviction returns its span");
        let mut session = session;
        session.messages = Arc::new(vec![msg(MessageRole::System, "Summary: discussed stuff")]);
        // Advance the live index past BOTH sealed seqs. A record at or above
        // `first_message_index` reads as a pre-clear original of a still-live
        // turn (a heal source), not as evicted — so an eviction that advances
        // by less than it sealed makes its own last message unreachable.
        session.record_compaction("Summary: discussed stuff", 2);
        session.save_to_path(&session_path).expect("save");

        // Exactly the call the continuation message writes out.
        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-span".into()),
                seq_from: Some(span.0),
                seq_to: Some(span.1),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(
            out.contains("VAULT_ALPHA the first evicted turn"),
            "the advertised span must reach the sealed original (span {span:?}): {out}"
        );
        assert!(
            out.contains("VAULT_BETA the answer to it"),
            "every message in the sealed span, not just the first (span {span:?}): {out}"
        );
    }

    #[test]
    fn recall_keeps_evicted_even_when_live_tail_is_capped() {
        // Regression (Codex P2 #1): the recent-tail cap must NOT drop recovered
        // evicted content, and the index-0 compaction summary is suppressed when
        // the raw originals are recovered.
        let base = temp_base("vault-tailcut");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let session_path = dir.join("sess-tc.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-tc".to_owned();
        let session = session.with_persistence_path(session_path.clone());
        let _ = session.seal_evicted_to_vault(&[msg(MessageRole::User, "EVICTED_MARKER one")]);
        let mut session = session;
        let mut live = vec![msg(MessageRole::System, "Summary: discussed stuff")];
        live.extend((0..40).map(|i| msg(MessageRole::User, &format!("live {i}"))));
        session.messages = Arc::new(live);
        // Advance past the sealed seq so the record reads as EVICTED, not as a
        // microcompact pre-clear original of a still-live turn.
        session.record_compaction("Summary: discussed stuff", 1);
        session.save_to_path(&session_path).expect("save");

        // Default recall (no query, no last_n): the live tail is capped, but the
        // recovered evicted message must still appear.
        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-tc".into()),
                query: None,
                role: None,
                last_n: None,
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(
            out.contains("EVICTED_MARKER one"),
            "evicted content survives the live-tail cap: {out}"
        );
        assert!(out.contains("live 39"), "recent live tail still shown: {out}");
        assert!(!out.contains("live 0"), "old live messages capped out: {out}");
        assert!(
            !out.contains("Summary: discussed stuff"),
            "index-0 compaction summary suppressed when raw recovered: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_pre_clear_original_heals_its_live_copy_instead_of_being_listed_twice() {
        // Microcompact seals pre-clear originals at seqs that are STILL LIVE, so
        // the vault now overlaps the transcript. Recall must not show that turn
        // twice (once "evicted", once live) — it substitutes the original body for
        // the blanked live copy, which is exactly the detail a reader recalls
        // for.
        //
        // SCOPE of this case, so it is not mistaken for the regression guard it
        // is not: `first_message_index` is 0 and nothing has compacted yet, so seq
        // 0 and live index 0 coincide and a POSITIONAL heal would pass here too
        // (verified: reverting the heal to seq-addressing leaves this test green).
        // What pins the content-addressed heal is
        // `a_sealed_body_heals_its_own_turn_after_compaction_shifts_the_seq_axis`,
        // where the axes disagree.
        let base = temp_base("vault-heal");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let session_path = dir.join("sess-heal.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-heal".to_owned();
        let session = session.with_persistence_path(session_path.clone());
        // No `record_compaction` here: `first_message_index` stays 0, so seq 0
        // is inside the live range — the pre-clear-original case.
        assert!(
            session.seal_originals_to_vault(&[(
                0,
                tool_msg("tu-1", "PRE_CLEAR_ORIGINAL full tool body"),
            )]),
            "the fixture's vault seed must actually reach the vault"
        );
        let mut session = session;
        session.messages = Arc::new(vec![tool_msg("tu-1", runtime::MICROCOMPACT_PLACEHOLDER)]);
        session.save_to_path(&session_path).expect("save");

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-heal".into()),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(
            out.contains("PRE_CLEAR_ORIGINAL full tool body"),
            "the live copy is healed from the sealed original: {out}"
        );
        assert!(
            !out.contains(runtime::MICROCOMPACT_PLACEHOLDER),
            "the blanked live copy is replaced, not shown alongside: {out}"
        );
        assert!(
            out.contains("1 of 1 message(s)"),
            "the turn is counted once, not once per source: {out}"
        );
        assert!(
            !out.contains("recovered from the compaction vault"),
            "a still-live turn is not reported as evicted: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_sealed_body_heals_its_own_turn_after_compaction_shifts_the_seq_axis() {
        // The two stampers on the seq axis disagree by one after the first full
        // compaction: microcompact sealed at `first_message_index + index`, then
        // `apply_compaction` prepended a synthetic continuation while
        // `first_message_index` advanced only by the removed count. A positional
        // heal therefore lands one turn early — it would paste the sealed body
        // over the PREVIOUS turn and leave the blanked turn blank forever. The
        // heal is keyed on `tool_use_id`, so the drift cannot mis-target it.
        let base = temp_base("vault-heal-drift");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let session_path = dir.join("sess-drift.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-drift".to_owned();
        let session = session.with_persistence_path(session_path.clone());
        // Sealed while the turn sat at absolute seq 4.
        assert!(
            session.seal_originals_to_vault(&[(4, tool_msg("tu-late", "LATE_BODY exact bytes"))]),
            "the fixture's vault seed must actually reach the vault"
        );
        let mut session = session;
        // Post-compaction transcript: continuation at local 0, then the preserved
        // tail. With `first_message_index == 3` the local indices address seqs
        // 3, 4, 5 — so seq 4 (the sealed record) now points at the DECOY, one
        // slot before the turn that actually owns `tu-late`.
        session.messages = Arc::new(vec![
            msg(MessageRole::System, "Summary: continuation"),
            msg(MessageRole::User, "POSITION_FOUR_DECOY"),
            tool_msg("tu-late", runtime::MICROCOMPACT_PLACEHOLDER),
        ]);
        session.record_compaction("Summary: continuation", 3);
        session.save_to_path(&session_path).expect("save");
        assert_eq!(session.first_message_index(), 3, "seq base after one round");

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-drift".into()),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(
            out.contains("LATE_BODY exact bytes"),
            "the sealed body is restored despite the +1 seq drift: {out}"
        );
        assert!(
            !out.contains(runtime::MICROCOMPACT_PLACEHOLDER),
            "the turn that owns tu-late is healed, not left blank: {out}"
        );
        assert!(
            out.contains("POSITION_FOUR_DECOY"),
            "the turn at the sealed seq is untouched — substitution needs a \
             blanked body with a matching id, not a position: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The pre-clear original of a captioned pasted image, and the blanked form
    /// microcompact leaves in its place — the clear rewrites the `Image` block IN
    /// PLACE, so the two differ only at block 1. Tests that hand-build a live
    /// turn must use this shape, or they are pinning a message no production
    /// path can produce (and the heal's structural check will reject it).
    fn image_turn(caption: &str, data: &str) -> (ConversationMessage, ConversationMessage) {
        let build = |second: ContentBlock| ConversationMessage {
            role: MessageRole::User,
            blocks: vec![
                ContentBlock::Text {
                    text: caption.to_owned(),
                },
                second,
            ],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        };
        (
            build(ContentBlock::Image {
                media_type: "image/png".to_owned(),
                data: data.to_owned(),
            }),
            build(ContentBlock::Text {
                text: runtime::MICROCOMPACT_IMAGE_PLACEHOLDER.to_owned(),
            }),
        )
    }

    #[test]
    fn a_sealed_standalone_image_heals_only_the_turn_that_lost_an_image() {
        // A standalone pasted image has no `tool_use_id`, so its recovery is the
        // one positional case left. Both sides must agree before it fires: the
        // live turn must actually hold the image placeholder AND the record at
        // that seq must actually hold an `Image`. The second live turn has the
        // placeholder but no image was sealed at its seq, so it stays blanked
        // rather than borrowing some other record's content.
        let base = temp_base("vault-heal-image");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let session_path = dir.join("sess-image.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-image".to_owned();
        let session = session.with_persistence_path(session_path.clone());
        let (sealed_image, blanked_image) = image_turn("SCREENSHOT_CONTEXT", "AAAA");
        assert!(
            session.seal_originals_to_vault(&[
                (0, sealed_image),
                // A record at seq 1 that carries no image at all — the negative
                // side of the gate.
                (1, tool_msg("tu-other", "UNRELATED_SEALED_BODY")),
            ]),
            "the fixture's vault seed must actually reach the vault"
        );
        let mut session = session;
        session.messages = Arc::new(vec![
            blanked_image,
            msg(MessageRole::User, runtime::MICROCOMPACT_IMAGE_PLACEHOLDER),
        ]);
        session.save_to_path(&session_path).expect("save");

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-image".into()),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(
            out.contains("SCREENSHOT_CONTEXT") && out.contains("[image: image/png]"),
            "the turn whose image was sealed is healed whole: {out}"
        );
        assert_eq!(
            out.matches(runtime::MICROCOMPACT_IMAGE_PLACEHOLDER).count(),
            1,
            "the turn with no sealed image at its seq keeps its placeholder: {out}"
        );
        assert!(
            !out.contains("UNRELATED_SEALED_BODY"),
            "a record without an image never substitutes for an image turn: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn adjacent_blanked_image_turns_never_borrow_each_other_s_sealed_content() {
        // The harm this pins is not "fails to heal" but "heals with the WRONG
        // turn's content". The +1 seq drift is established at the first full
        // compaction and is permanent, so every preserved-tail lookup lands one
        // record late: turn A at seq 4 resolves to the record sealed for turn B.
        // Both sides of the old gate agreed (A holds the image placeholder, the
        // record at 4 holds an Image), so B's whole message was rendered in A's
        // slot — A's own caption gone, B still blanked.
        //
        // The heal is structural now: the record must be a pre-clear original OF
        // THIS turn. B's record cannot pass for A (different caption), so both
        // turns degrade to "still blanked", which is what the reader had before
        // any image heal existed.
        let base = temp_base("vault-image-adjacent");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let session_path = dir.join("sess-adj.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-adj".to_owned();
        let session = session.with_persistence_path(session_path.clone());
        let (sealed_a, blanked_a) = image_turn("CAPTION_ALPHA", "AAAA");
        let (sealed_b, blanked_b) = image_turn("CAPTION_BETA", "BBBB");
        // Sealed BEFORE the session's first compaction, so both records sit one
        // seq below the live axis the transcript below addresses.
        assert!(
            session.seal_originals_to_vault(&[(3, sealed_a), (4, sealed_b)]),
            "the fixture's vault seed must actually reach the vault"
        );
        let mut session = session;
        // `first_message_index == 3`: local indices 0,1,2 address seqs 3,4,5, so
        // turn A (local 1) looks up seq 4 — the record sealed for turn B.
        session.messages = Arc::new(vec![
            msg(MessageRole::System, "Summary: continuation"),
            blanked_a,
            blanked_b,
        ]);
        session.record_compaction("Summary: continuation", 3);
        session.save_to_path(&session_path).expect("save");
        assert_eq!(session.first_message_index(), 3, "seq base after one round");

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-adj".into()),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(
            out.contains("CAPTION_ALPHA"),
            "turn A keeps its OWN text — a neighbouring record must never \
             overwrite it: {out}"
        );
        assert!(
            out.contains("CAPTION_BETA"),
            "turn B keeps its own text too: {out}"
        );
        assert!(
            !out.contains("[image:"),
            "neither turn borrows an image it does not own; the degraded mode is \
             'still blanked', not 'wrong content': {out}"
        );
        assert_eq!(
            out.matches(runtime::MICROCOMPACT_IMAGE_PLACEHOLDER).count(),
            2,
            "both turns stay blanked: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn alias_recall_skips_current_session_when_session_id_is_known() {
        let base = temp_base("alias-skip-current");
        write_session(
            &base,
            "sess-prior",
            vec![msg(MessageRole::User, "prior session todo marker")],
        );
        std::thread::sleep(Duration::from_millis(5));
        write_session(
            &base,
            "sess-current",
            vec![msg(MessageRole::User, "current empty-session marker")],
        );
        let ctx = ctx_for(&base);
        ctx.set_session_id("sess-current");

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("latest".into()),
                query: None,
                role: None,
                last_n: None,
                ..Default::default()
            },
            &ctx,
        )
        .expect("alias recall ok");

        assert!(out.contains("session `sess-prior`"), "{out}");
        assert!(out.contains("prior session todo marker"), "{out}");
        assert!(!out.contains("current empty-session marker"), "{out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn alias_recall_without_current_session_id_keeps_latest_behavior() {
        let base = temp_base("alias-no-current-id");
        write_session(
            &base,
            "sess-prior",
            vec![msg(MessageRole::User, "prior marker")],
        );
        std::thread::sleep(Duration::from_millis(5));
        write_session(
            &base,
            "sess-current",
            vec![msg(MessageRole::User, "latest marker")],
        );

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("latest".into()),
                query: None,
                role: None,
                last_n: None,
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("alias recall ok");

        assert!(out.contains("session `sess-current`"), "{out}");
        assert!(out.contains("latest marker"), "{out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn search_across_sessions_ranks_by_match() {
        let base = temp_base("search");
        write_session(
            &base,
            "chat-1",
            vec![msg(
                MessageRole::User,
                "the deadlock happens in the mutex guard",
            )],
        );
        write_session(
            &base,
            "chat-2",
            vec![
                msg(MessageRole::User, "a mutex deadlock again"),
                msg(MessageRole::Assistant, "yes the mutex is the cause"),
            ],
        );
        write_session(
            &base,
            "chat-3",
            vec![msg(MessageRole::User, "totally different topic")],
        );

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: None,
                query: Some("mutex".into()),
                role: None,
                last_n: None,
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("search ok");

        assert!(out.contains("chat-1"), "{out}");
        assert!(out.contains("chat-2"), "{out}");
        assert!(
            !out.contains("chat-3"),
            "non-matching session omitted: {out}"
        );
        assert!(
            out.contains("2 with matches"),
            "two sessions matched: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn search_requires_a_query() {
        let base = temp_base("noquery");
        let err = run_session_recall(
            SessionRecallInput {
                session_ref: None,
                query: None,
                role: None,
                last_n: None,
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect_err("search without query is rejected");
        assert!(err.to_string().contains("provide `query`"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unknown_session_ref_errors_clearly() {
        let base = temp_base("missing");
        std::fs::create_dir_all(base.join(".zo").join("sessions")).expect("mk");
        let err = run_session_recall(
            SessionRecallInput {
                session_ref: Some("does-not-exist".into()),
                query: None,
                role: None,
                last_n: None,
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect_err("missing session errors");
        assert!(err.to_string().contains("session_recall:"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn invalid_role_is_rejected() {
        let err = parse_role("captain").expect_err("bad role");
        assert!(err.to_string().contains("unknown role"), "{err}");
    }

    #[test]
    fn path_like_session_ref_is_rejected() {
        // A model-controlled ref must not escape the workspace via a path; only
        // ids/aliases are allowed (no disk access happens — rejected up front).
        for bad in [
            "../../etc/passwd",
            "/etc/passwd",
            "sub/dir/x",
            "..\\win",
            "incident.jsonl",
            "secrets.json",
        ] {
            let err = run_session_recall(
                SessionRecallInput {
                    session_ref: Some(bad.to_owned()),
                    query: None,
                    role: None,
                    last_n: None,
                    ..Default::default()
                },
                &ToolContext::new(),
            )
            .expect_err("path-like ref rejected");
            assert!(
                err.to_string().contains("not a valid session reference"),
                "ref {bad}: {err}"
            );
        }
    }

    #[test]
    fn last_n_zero_is_rejected() {
        let err = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-x".into()),
                query: None,
                role: None,
                last_n: Some(0),
                ..Default::default()
            },
            &ToolContext::new(),
        )
        .expect_err("last_n=0 rejected");
        assert!(err.to_string().contains("last_n must be >= 1"), "{err}");
    }

    #[test]
    fn dual_cap_note_reports_both_request_and_ceiling() {
        // last_n above the hard ceiling must surface BOTH numbers, not silently
        // relabel as the global cap (the cap-clobber bug).
        let base = temp_base("dualcap");
        let many: Vec<_> = (0..MAX_RECALL_MESSAGES + 5)
            .map(|i| msg(MessageRole::User, &format!("match {i}")))
            .collect();
        write_session(&base, "sess-big", many);
        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-big".into()),
                query: Some("match".into()),
                role: None,
                last_n: Some(MAX_RECALL_MESSAGES + 50),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");
        assert!(
            out.contains(&format!("requested last {}", MAX_RECALL_MESSAGES + 50)),
            "note keeps the requested value: {}",
            out.lines().next().unwrap_or_default()
        );
        assert!(
            out.contains(&format!("capped at {MAX_RECALL_MESSAGES}")),
            "note keeps the ceiling: {}",
            out.lines().next().unwrap_or_default()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn snippet_centers_on_match_with_case_expanding_chars() {
        // Turkish 'İ' lowercases to TWO chars; the lowercased match offset must be
        // mapped back to the original so the snippet still contains the match.
        let text = format!("{} CRITICALBUG happened here", "İ".repeat(60));
        let snippet = make_snippet(&text, "criticalbug");
        assert!(
            snippet.to_lowercase().contains("criticalbug"),
            "snippet must include the matched term: {snippet}"
        );
    }

    #[test]
    fn message_plain_condenses_tool_and_image_blocks() {
        let m = ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![
                ContentBlock::Text {
                    text: "running it".into(),
                },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: "{\"command\":\"ls\"}".into(),
                },
                ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "QUJD".into(),
                },
            ],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        };
        let plain = message_plain(&m);
        assert!(plain.contains("running it"));
        assert!(plain.contains("[tool call: bash]"));
        assert!(plain.contains("[image: image/png]"));
        assert!(
            !plain.contains("QUJD"),
            "base64 must not be dumped: {plain}"
        );
    }

    fn tool_result_msg(id: &str, name: &str, output: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_owned(),
                tool_name: name.to_owned(),
                output: output.to_owned(),
                is_error: false,
                images: Vec::new(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        }
    }

    #[test]
    fn recall_seq_range_filters_across_evicted_and_live_boundary() {
        // A compacted session: seqs 0-2 live in the vault (evicted), seqs 3-5 are
        // the live tail. A [1, 3] window must span the boundary: evicted 1-2 plus
        // live 3, excluding evicted 0 and live 4-5.
        let base = temp_base("seq-range");
        let dir = base.join(".zo").join("sessions");
        std::fs::create_dir_all(&dir).expect("mk sessions dir");
        let path = dir.join("sess-seq.jsonl");

        let mut session = Session::new();
        session.session_id = "sess-seq".to_owned();
        let session = session.with_persistence_path(path.clone());
        let _ = session.seal_evicted_to_vault(&[
            msg(MessageRole::User, "evicted zero"),
            msg(MessageRole::User, "evicted one"),
            msg(MessageRole::User, "evicted two"),
        ]);
        let mut session = session;
        session.messages = Arc::new(vec![
            msg(MessageRole::User, "live three"),
            msg(MessageRole::User, "live four"),
            msg(MessageRole::User, "live five"),
        ]);
        // Advances first_message_index 0 -> 3 and persists the snapshot, so the
        // live tail addresses seqs 3, 4, 5 on reload.
        session.record_compaction("summary of evicted", 3);

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-seq".into()),
                seq_from: Some(1),
                seq_to: Some(3),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");

        assert!(!out.contains("evicted zero"), "seq 0 excluded: {out}");
        assert!(
            out.contains("evicted one") && out.contains("evicted two"),
            "evicted seqs 1-2 included: {out}"
        );
        assert!(out.contains("live three"), "live seq 3 included: {out}");
        assert!(
            !out.contains("live four") && !out.contains("live five"),
            "live seqs 4-5 excluded: {out}"
        );
        assert!(out.contains("seq 1-3"), "header names the seq window: {out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn recall_can_omit_tool_results() {
        let base = temp_base("omit-tools");
        write_session(
            &base,
            "sess-omit",
            vec![
                msg(MessageRole::User, "please run the build"),
                tool_result_msg("t1", "bash", "BUILD_OUTPUT_XYZ compiled 42 files"),
            ],
        );

        let with = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-omit".into()),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");
        assert!(
            with.contains("BUILD_OUTPUT_XYZ"),
            "tool result shown by default: {with}"
        );

        let without = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-omit".into()),
                include_tool_results: Some(false),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("recall ok");
        assert!(
            !without.contains("BUILD_OUTPUT_XYZ"),
            "tool result omitted: {without}"
        );
        assert!(
            without.contains("please run the build"),
            "text message still rendered: {without}"
        );
        assert!(
            without.contains("tool results omitted from render"),
            "omission noted in the header: {without}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn current_alias_recalls_this_session_without_excluding_it() {
        // `latest` excludes the current session (see `alias_recall_skips_current…`);
        // `current` must instead load THIS session so the model can pull back its
        // own just-compacted originals.
        let base = temp_base("current-alias");
        write_session(
            &base,
            "sess-current",
            vec![msg(MessageRole::User, "CURRENT_SESSION_MARKER present")],
        );
        let ctx = ctx_for(&base);
        ctx.set_session_id("sess-current");

        let out = run_session_recall(
            SessionRecallInput {
                session_ref: Some("current".into()),
                ..Default::default()
            },
            &ctx,
        )
        .expect("current recall ok");
        assert!(out.contains("session `sess-current`"), "{out}");
        assert!(out.contains("CURRENT_SESSION_MARKER present"), "{out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn current_alias_without_active_session_id_errors() {
        let base = temp_base("current-no-id");
        let err = run_session_recall(
            SessionRecallInput {
                session_ref: Some("current".into()),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect_err("current with no active session id is rejected");
        assert!(
            err.to_string().contains("no active persisted session"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Stamp a written session file's mtime so the time-window prefilter (which
    /// reads on-disk mtimes via `list_managed_sessions_for`) can be exercised
    /// deterministically without a `filetime` dependency — `File::set_modified`
    /// is std.
    fn set_session_mtime(base: &std::path::Path, id: &str, mtime: SystemTime) {
        let path = base
            .join(".zo")
            .join("sessions")
            .join(format!("{id}.jsonl"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open session for mtime");
        file.set_modified(mtime).expect("set mtime");
    }

    fn days_ago(days: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(days * 86_400)
    }

    /// Write three sessions all matching "widget", aged 0 / 5 / 30 days.
    fn write_timed_widget_sessions(base: &std::path::Path) {
        for (id, age) in [("sess-recent", 0), ("sess-mid", 5), ("sess-old", 30)] {
            write_session(base, id, vec![msg(MessageRole::User, "the widget broke")]);
            set_session_mtime(base, id, days_ago(age));
        }
    }

    #[test]
    fn search_since_days_keeps_only_recent_sessions() {
        let base = temp_base("since-days");
        write_timed_widget_sessions(&base);
        let out = run_session_recall(
            SessionRecallInput {
                query: Some("widget".into()),
                since_days: Some(7.0),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("search ok");
        assert!(out.contains("sess-recent"), "age 0 within 7d: {out}");
        assert!(out.contains("sess-mid"), "age 5 within 7d: {out}");
        assert!(!out.contains("sess-old"), "age 30 excluded: {out}");
        assert!(
            out.contains("within the last 7 day(s)"),
            "header notes the window: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn search_before_days_keeps_only_old_sessions() {
        let base = temp_base("before-days");
        write_timed_widget_sessions(&base);
        let out = run_session_recall(
            SessionRecallInput {
                query: Some("widget".into()),
                before_days: Some(10.0),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("search ok");
        assert!(!out.contains("sess-recent"), "age 0 excluded: {out}");
        assert!(!out.contains("sess-mid"), "age 5 excluded: {out}");
        assert!(out.contains("sess-old"), "age 30 older than 10d: {out}");
        assert!(
            out.contains("more than 10 day(s) ago"),
            "header notes the window: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn search_time_window_band_brackets_a_span() {
        // since_days >= before_days is a valid band: 2..=20 days ago keeps only
        // the 5-day-old session (excludes the 0-day and 30-day ones).
        let base = temp_base("band-days");
        write_timed_widget_sessions(&base);
        let out = run_session_recall(
            SessionRecallInput {
                query: Some("widget".into()),
                since_days: Some(20.0),
                before_days: Some(2.0),
                ..Default::default()
            },
            &ctx_for(&base),
        )
        .expect("search ok");
        assert!(!out.contains("sess-recent"), "age 0 below band: {out}");
        assert!(out.contains("sess-mid"), "age 5 in band: {out}");
        assert!(!out.contains("sess-old"), "age 30 above band: {out}");
        assert!(
            out.contains("modified 2–20 day(s) ago"),
            "header names the band: {out}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn inverted_time_window_is_rejected() {
        // since_days < before_days: newer than 2 days AND older than 20 days is
        // empty — rejected up front (mirrors the seq_from > seq_to guard).
        let err = run_session_recall(
            SessionRecallInput {
                query: Some("widget".into()),
                since_days: Some(2.0),
                before_days: Some(20.0),
                ..Default::default()
            },
            &ToolContext::new(),
        )
        .expect_err("empty window rejected");
        assert!(
            err.to_string().contains("empty window")
                && err.to_string().contains("since_days"),
            "{err}"
        );
    }

    #[test]
    fn negative_days_is_rejected() {
        let err = run_session_recall(
            SessionRecallInput {
                query: Some("widget".into()),
                since_days: Some(-1.0),
                ..Default::default()
            },
            &ToolContext::new(),
        )
        .expect_err("negative days rejected");
        assert!(err.to_string().contains("non-negative"), "{err}");
    }

    #[test]
    fn time_filters_are_rejected_in_recall_mode() {
        // A single-session recall targets one transcript by id; a session-mtime
        // window has no meaning there, so it is rejected rather than ignored.
        let err = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-x".into()),
                since_days: Some(3.0),
                ..Default::default()
            },
            &ToolContext::new(),
        )
        .expect_err("time filter in recall mode rejected");
        assert!(
            err.to_string().contains("search mode only"),
            "{err}"
        );
    }

    #[test]
    fn seq_from_greater_than_seq_to_is_rejected() {
        let err = run_session_recall(
            SessionRecallInput {
                session_ref: Some("sess-x".into()),
                seq_from: Some(10),
                seq_to: Some(5),
                ..Default::default()
            },
            &ToolContext::new(),
        )
        .expect_err("inverted seq window rejected");
        assert!(
            err.to_string().contains("seq_from") && err.to_string().contains("seq_to"),
            "{err}"
        );
    }

    /// The truncation note must name the escape hatch, because the note is the
    /// only moment the caller learns the search was partial.
    ///
    /// Measured before this: 16 real searches printed the old note, ten of them
    /// returned the searching session itself as their only hit, and
    /// `before_days` — the sole field that moves the window backwards — had
    /// been used in 0 of 937 calls.
    #[test]
    fn the_truncation_note_names_the_field_that_reaches_the_unscanned_sessions() {
        let note = super::truncation_note(711, None);
        assert!(note.contains("711"), "the note states the real total: {note}");
        assert!(
            note.contains(&(711 - super::MAX_SEARCH_SESSIONS).to_string()),
            "it states how many were NOT searched: {note}"
        );
        assert!(
            note.contains("before_days"),
            "it names the one field that reaches them: {note}"
        );
    }

    /// …and stays quiet about it for a caller already steering the window.
    #[test]
    fn a_caller_already_using_before_days_is_not_re_advised() {
        let note = super::truncation_note(711, Some(2.0));
        assert!(note.contains("711"));
        assert!(!note.contains("before_days"), "no advice to repeat: {note}");
    }

    /// t-2947 — a search over a 2,000-session store answers under the bound
    /// (fastest of N), and its hits are exactly the needle sessions among the
    /// newest `MAX_SEARCH_SESSIONS`.
    #[test]
    fn a_search_over_2000_sessions_stays_under_the_bound() {
        use session_corpus::{generate, CorpusSpec, SearchBounds, NEEDLE};

        let base = temp_base("search-bound");
        let manifest = generate(&base.join(".zo").join("sessions"), &CorpusSpec::REAL_MIX)
            .expect("synthetic corpus");
        let ctx = ctx_for(&base);
        let bounds = SearchBounds::T2947;
        let mut out = String::new();
        let elapsed = bounds.min_of_runs(|| {
            out = run_session_recall(
                SessionRecallInput {
                    query: Some(NEEDLE.into()),
                    ..Default::default()
                },
                &ctx,
            )
            .expect("search");
        });
        let expected: Vec<&str> = manifest
            .iter()
            .take(super::MAX_SEARCH_SESSIONS)
            .filter(|session| session.carries_needle)
            .map(|session| session.id.as_str())
            .collect();
        assert!(!expected.is_empty());
        for id in &expected {
            assert!(out.contains(&format!("`{id}`")), "{id} missing from:\n{out}");
        }
        assert!(
            out.contains(&format!("{} with matches", expected.len())),
            "exactly the needle sessions match:\n{out}"
        );
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            elapsed < bounds.session_recall_search,
            "session_recall search took {elapsed:?} over {} sessions (bound {:?}, min of {})",
            manifest.len(),
            bounds.session_recall_search,
            bounds.min_of
        );
    }

    /// t-2947 — the prefilter's verdict on a query: only a query whose every
    /// match must lie inside one string value may skip a parse.
    #[test]
    fn a_query_is_seam_safe_only_when_a_match_must_lie_inside_one_value() {
        use super::query_is_seam_safe;

        for safe in ["needle", "haystack-needle", "세션 검색", "prompt 12", "read_file", "x y z", "tools"] {
            assert!(query_is_seam_safe(safe), "{safe:?} can only match inside a value");
        }
        // Seam characters, whole seams, seam suffixes, escaped characters.
        for unsafe_query in [
            "[tool call",
            "read_file] {",
            "tool",
            "call: read",
            "l: read",
            " prompt",
            "error: ",
            "image: png",
            "thinking",
            "a\"b",
            "a\\b",
            "line\nbreak",
            "\t",
        ] {
            assert!(!query_is_seam_safe(unsafe_query), "{unsafe_query:?} may touch a seam");
        }
    }

    /// t-2947 — the search answers the same with and without the prefilter:
    /// over the synthetic corpus plus sessions built to sit on every seam the
    /// prefilter reasons about — quotes, backslashes, newlines, tabs, tool-call
    /// text, an image, the Kelvin sign and a dotted İ, and a term that lives
    /// only in the vault.
    #[test]
    fn the_prefilter_never_changes_a_search_answer() {
        use session_corpus::{generate, CorpusSpec, NEEDLE};

        let base = temp_base("prefilter-identity");
        let dir = base.join(".zo").join("sessions");
        generate(&dir, &CorpusSpec::REAL_MIX.with_sessions(60)).expect("synthetic corpus");
        write_session(
            &base,
            "seam-tool",
            vec![
                ConversationMessage::assistant(vec![
                    ContentBlock::Text {
                        text: "Let me look.".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "read_file".into(),
                        input: "{\"path\": \"src/x.rs\"}".into(),
                    },
                ]),
                tool_msg("t1", "fn main() {}"),
                ConversationMessage::assistant(vec![ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                }]),
            ],
        );
        write_session(
            &base,
            "seam-escapes",
            vec![msg(
                MessageRole::User,
                "say \"hi\" \\ back\nline two\ttab 3K İstanbul",
            )],
        );
        let vault_path = dir.join("seam-vault.jsonl");
        let mut session = Session::new();
        session.session_id = "seam-vault".to_owned();
        let session = session.with_persistence_path(vault_path.clone());
        let _ = session.seal_evicted_to_vault(&[msg(MessageRole::User, "only-in-vault-term here")]);
        let mut session = session;
        session.messages = Arc::new(vec![msg(MessageRole::System, "Summary: a term was discussed")]);
        session.record_compaction("Summary: a term was discussed", 1);
        session.save_to_path(&vault_path).expect("save vault session");

        let summaries = list_managed_sessions_for(&base).expect("listing");
        assert_eq!(summaries.len(), 63);
        let queries = [
            NEEDLE,
            "prompt 0/",
            "prompt 3",
            "tool",
            "call: read",
            "read_file] {",
            "[tool call",
            "tool call: read_file",
            "\"hi\"",
            "hi\" \\ back",
            "line two",
            "two\ttab",
            "3k",
            "istanbul",
            "i̇stanbul",
            "only-in-vault-term",
            "image: png",
            "image/png",
            "zzz-none",
            "세션",
            "fn main",
        ];
        let mut matched = 0;
        for query in queries {
            let query_lc = query.to_lowercase();
            for role in [None, Some(MessageRole::User), Some(MessageRole::Assistant), Some(MessageRole::Tool)] {
                let with = super::search_hits(&summaries, &query_lc, role, true);
                let without = super::search_hits(&summaries, &query_lc, role, false);
                assert_eq!(with, without, "{query:?} role {role:?}");
                matched += with.len();
            }
        }
        assert!(matched > 0, "the table must exercise hits, not only misses");
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// Measurement harness (t-2947), not a gate: `ZO_SESSION_ROOT=<root> cargo test
/// -p tools --lib -- --ignored --nocapture measure_session_recall` prints how
/// long a search-mode `session_recall` takes over whatever store the root
/// holds (`ZO_MEASURE_QUERY` picks the term; default `needle`).
#[cfg(test)]
mod measure {
    use std::time::{Duration, Instant};

    use super::{run_session_recall, SessionRecallInput};
    use crate::ToolContext;

    const RUNS: usize = 5;
    const DEFAULT_QUERY: &str = "needle";

    fn percentile(samples: &mut [Duration], percent: usize) -> Duration {
        samples.sort_unstable();
        samples[(samples.len() * percent / 100).min(samples.len() - 1)]
    }

    #[test]
    #[ignore = "measurement, not a gate — needs ZO_SESSION_ROOT"]
    fn measure_session_recall_search_on_session_root() {
        let Some(root) = std::env::var_os("ZO_SESSION_ROOT") else {
            eprintln!("ZO_SESSION_ROOT unset; nothing measured");
            return;
        };
        let query = std::env::var("ZO_MEASURE_QUERY").unwrap_or_else(|_| DEFAULT_QUERY.to_string());
        let base = std::env::temp_dir().join(format!("zo-measure-recall-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("base dir");
        let ctx = ToolContext::new().with_cwd(base.clone());
        let mut searches = Vec::with_capacity(RUNS);
        let mut header = String::new();
        for _ in 0..RUNS {
            let started = Instant::now();
            let out = run_session_recall(
                SessionRecallInput {
                    query: Some(query.clone()),
                    ..Default::default()
                },
                &ctx,
            )
            .expect("search");
            searches.push(started.elapsed());
            header = out.lines().next().unwrap_or_default().to_string();
        }
        let _ = std::fs::remove_dir_all(&base);
        println!(
            "session_recall search for {query:?} over {} ({RUNS} runs)\n  {header}\n  search    p50 {:>8.2} ms  p95 {:>8.2} ms",
            root.to_string_lossy(),
            percentile(&mut searches, 50).as_secs_f64() * 1e3,
            percentile(&mut searches, 95).as_secs_f64() * 1e3,
        );
    }
}
