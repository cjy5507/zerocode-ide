#!/usr/bin/env python3
"""r33 -- what fills zo's 511 B/request of reminders, and where it could move.

r31 (`docs/analysis/write-volume-by-tool-r31.md`) closed with the largest
measured-but-uncut lever it found: reminders, 511 B/request, 11.0% of the
new-content gap, led by `Recalled memory` at 171 B/req over 1,606 requests.
It could not say how much of that is the SAME body injected again, which is
what a "write once, read from cache" alternative would actually save. This
script answers that, and then asks the cache question underneath it.

COORDINATE SYSTEM. Every definition that decides which requests are counted --
the zo session reader (live/rot/vault merge, largest payload per turn index),
the request boundary (an assistant message carrying `usage`), the rolling
filter (`seq >= 3`, not first, not compaction), UTF-8 payload bytes -- is
IMPORTED from r31's script rather than restated. r22 §1's rule (a ruler that
cannot reproduce the old numbers may not publish new ones) is enforced in §0
by re-deriving r31's reminder table with this file's own accounting.

WHAT IS NEW HERE, and why each definition is the one it is:

1. KIND, not tag. r31 named a reminder by the first `[zo:...]`/`[system:...]`
   marker in its first 400 characters, falling back to its first line. That is
   a ranking key, not a taxonomy: one producer spreads across several tags when
   its first line varies (`route-hint` renders four different opening lines),
   and two producers collapse when they share one. This script matches each
   injection against a table of PRODUCER PREFIXES read out of the source, so a
   row is one code site. `KIND_SITES` carries the file:symbol for every row, so
   the table can be audited against the tree rather than trusted.

2. CARRIER. A reminder reaches the model three ways and they cost differently:
   (a) as its own `role: "system"` message, appended at the tail by
   `ConversationRuntime::absorb_wire_reminders_into_session`; (b) as a
   `<system-reminder>` span inside another message's text; (c) spliced into a
   tool result (`conversation::helpers::merge_hook_context`). Only (a) is
   append-only. (b) and (c) are written into a message that is built once, so
   they ride whatever that message rides.

3. REPETITION, at three strengths, because "the same reminder again" has three
   meanings and they bound three different savings:
     * EXACT   -- byte-identical body already injected earlier in this session.
                  The ceiling for "inject once per session".
     * ADJACENT-- byte-identical to the immediately preceding injection of the
                  same kind in the same session. The ceiling for "skip if
                  unchanged since last time", which is a strictly weaker rule.
     * KIND    -- a second injection of the kind at all, whatever the body.
                  The ceiling for "once per session, period" -- which changes
                  what the model sees, so it is reported but not proposed.
   Sessions are the scope because the transcript is: a body already in THIS
   session's prefix is what the model can still read for free.

4. RIDES, and the read column. A reminder persisted as a trailing message is
   never rewritten, so it is written once and then read as prefix by every
   later request in the session. `rides` counts those later rolling requests,
   stopping at the next compaction boundary (compaction summarizes the
   persisted copy away -- which is also the re-teach signal
   `install_reminder_until_persisted` keys on). It is printed as a MEDIAN and
   a p90, never multiplied out into a read-bytes total: that product is a claim
   about session-length distribution (this corpus holds 1,400-request sessions
   beside 5-request ones) wearing a cache column's clothes, and a superseded
   reminder is additionally cleared in place to `CLEARED_REMINDER_PLACEHOLDER`
   so it does not ride to the end anyway. The direction is what carries Goal 2:
   reads are the column "move it to the anchor" does NOT change -- an
   anchor-resident block is also written once and read after -- so only the
   duplicate WRITES are recoverable.

5. THE ANCHOR, REPRODUCED. `convert_messages::mark_breakpoints_with_ttl` puts
   the rolling marker on the newest cacheable message and the anchor on
   `previous_request_tail` -- "the newest user-role message strictly behind
   the rolling slot", chosen so the anchor lands where the PREVIOUS request
   left its rolling marker. `MessageRole::System` lowers to wire role "user"
   (`convert_messages`, the role match) and is emitted as its own message, so
   a persisted reminder adds a second user-role message per iteration. This
   script reproduces the marker placement over the real transcripts -- role
   lowering, tool-run merging, empty-content drop, `has_cacheable_block`'s
   thinking exclusion -- and reports how often the anchor lands on a position
   the previous request had marked, WITH the reminder messages and WITHOUT
   them. That difference is the price of the carrier, measured rather than
   argued. It is a REPRODUCTION, not a provider observation: it says where zo
   asked the provider to look, which is what the code controls.

CAVEATS, in the direction the reader should discount:
  * zo byte columns are an upper bound for r31's reason (the transcript keeps
    the pre-`wire_tool_output` body). Reminder bodies are not tool results, so
    carrier (a) is unaffected; carrier (c) is not.
  * The corpus is not version-frozen. `zo:model-handoff` and
    `zo:turn-confidence-contract` appear in it and no longer exist in the
    tree; the session-scope recall collapse
    (`memory::recall::dedup_recalled_section_for_session`) exists in the tree
    and its effect is visible only in the sessions that ran with it. §1 slices
    by month-day so a reader can see which regime a row came from.
  * The anchor reproduction infers wire content emptiness from block shape,
    not from `convert_blocks` itself. It is exact about roles and positions,
    approximate about which messages vanish.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import os
import statistics
import sys
from collections import Counter, defaultdict
from datetime import datetime
from pathlib import Path

# --- import r31's ruler ------------------------------------------------------
_R31 = Path(__file__).with_name("write-volume-by-tool.py")
_spec = importlib.util.spec_from_file_location("write_volume_by_tool", _R31)
if _spec is None or _spec.loader is None:  # pragma: no cover
    raise SystemExit(f"r33 needs r31's ruler beside it: {_R31}")
r31 = importlib.util.module_from_spec(_spec)
sys.modules["write_volume_by_tool"] = r31
_spec.loader.exec_module(r31)

utf8 = r31.utf8
add = r31.add
rolling = r31.rolling
REMINDER_SPAN = r31.REMINDER_SPAN

# ---------------------------------------------------------------------------
# The taxonomy (docstring note 1). Order matters: first match wins.
# ---------------------------------------------------------------------------

# (kind, family, matcher-prefix). The prefix is matched against the body with
# the `<system-reminder>` wrapper stripped, exactly as the producer writes it.
KIND_TABLE: list[tuple[str, str, str]] = [
    ("recalled-memory", "memory", "# Recalled memory"),
    ("state-distill", "working-state", "[zo:state-distill]"),
    ("verified-state", "working-state", "[zo:verified-state]"),
    ("todo-progress", "working-state", "[zo:todo-progress]"),
    ("edited-files", "working-state", "[system: Files this session has ALREADY edited"),
    ("session-commits", "working-state", "[system: Commits THIS session created"),
    ("history-attribution", "working-state", "[system: Commits and edits already in this session"),
    ("route-hint", "routing", "[zo:route-hint]"),
    ("skill-routing", "routing", "[zo:skill-routing]"),
    ("design-guidance", "routing", "[zo:design-guidance]"),
    ("turn-confidence-contract", "turn-control", "[zo:turn-confidence-contract]"),
    ("confidence-cascade", "turn-control", "[zo:confidence-cascade]"),
    ("goal-clarify", "turn-control", "[zo:goal-clarify]"),
    ("recall-hint", "turn-control", "[zo:recall-hint]"),
    ("turn-budget-continuation", "turn-control", "[zo:turn-budget-continuation]"),
    ("grind-escalation", "turn-control", "[zo:grind-escalation]"),
    ("turn-end-gate", "turn-control", "[zo:turn-end-gate]"),
    ("verify-treadmill", "turn-control", "[zo:verify-treadmill]"),
    ("empty-response-retry", "stream-repair", "[zo:empty-response-retry]"),
    ("empty-response-continuation", "stream-repair", "[zo:empty-response-continuation]"),
    ("truncation-continuation", "stream-repair", "[zo:truncation-continuation]"),
    ("hook-context", "hook", "[zo:user-prompt-hook-context]"),
    ("team-inbox", "hook", "Low-trust TeamInbox updates"),
    ("compaction-post", "compaction", "[system: Prior conversation context was automatically compacted"),
    ("compaction-resume", "compaction", "[system: This session was compacted earlier"),
    ("compaction-continue", "compaction", "This session is being continued from a previous conversation"),
    ("model-handoff", "compaction", "[zo:model-handoff]"),
    ("model-handoff", "compaction", "Model handoff:"),
    ("plan-mode", "routing", "[zo:plan-mode]"),
    ("persistent-goal", "routing", "[zo:persistent-goal]"),
    ("cleared-tombstone", "tombstone", "[Superseded reminder cleared]"),
]

# docstring note 1: one row of the table is one code site. Paths are relative
# to the workspace root (`zo-ide/`). Verified by grep at the measurement time
# recorded in the header; a kind with no live site is a corpus fossil and says
# so, which is itself a finding (the corpus spans versions).
KIND_SITES: dict[str, str] = {
    "recalled-memory": "crates/runtime/src/memory/recall.rs:RECALL_SECTION_HEADER + render_recalled_memory_section",
    "state-distill": "crates/runtime/src/conversation/reminders.rs:STATE_DISTILL_REMINDER_PREFIX",
    "verified-state": "crates/runtime/src/verified_state.rs:VERIFIED_STATE_REMINDER_PREFIX",
    "todo-progress": "crates/runtime/src/conversation/reminders.rs:todo_progress_reminder_for",
    "edited-files": "crates/runtime/src/turn_trace.rs:EDITED_FILES_REMINDER_PREFIX",
    "session-commits": "crates/runtime/src/turn_trace.rs:HISTORY_ATTRIBUTION_REMINDER_PREFIX (\"[system: Commits\")",
    "history-attribution": "crates/runtime/src/turn_trace.rs:HISTORY_ATTRIBUTION_REMINDER",
    "route-hint": "crates/runtime/src/auto_fanout.rs:build_route_hint",
    "skill-routing": "crates/runtime/src/skills.rs:SKILL_RECOMMENDATION_REMINDER_PREFIX",
    "design-guidance": "crates/runtime/src/conversation/reminders.rs:build_design_guidance_reminder",
    "turn-confidence-contract": "(no live site -- corpus fossil)",
    "confidence-cascade": "(no live site -- corpus fossil)",
    "goal-clarify": "crates/runtime/src/conversation/reminders.rs:GOAL_CLARIFY_REMINDER_PREFIX",
    "recall-hint": "crates/runtime/src/conversation/reminders.rs:RECALL_HINT_REMINDER",
    "turn-budget-continuation": "crates/runtime/src/conversation/reminders.rs:install_turn_budget_continuation_reminder",
    "grind-escalation": "(no live site -- corpus fossil)",
    "turn-end-gate": "crates/runtime/src/conversation/turn_end_gate.rs",
    "verify-treadmill": "crates/runtime/src/conversation/verify_treadmill.rs:VERIFY_TREADMILL_REMINDER_PREFIX",
    "empty-response-retry": "crates/runtime/src/conversation/mod.rs:EMPTY_STREAM_RETRY_REMINDER",
    "empty-response-continuation": "crates/runtime/src/conversation/mod.rs:EMPTY_STREAM_CONTINUATION_REMINDER",
    "truncation-continuation": "crates/runtime/src/conversation/mod.rs:TRUNCATION_CONTINUATION_REMINDER",
    "hook-context": "crates/runtime/src/conversation/reminders.rs:build_user_prompt_hook_context_reminder",
    "team-inbox": "crates/runtime/src/team_inbox_digest.rs:TEAM_INBOX_REMINDER_PREFIX",
    "compaction-post": "crates/runtime/src/conversation/mod.rs:POST_COMPACTION_SYSTEM_REMINDER",
    "compaction-resume": "crates/runtime/src/conversation/mod.rs:COMPACTION_RESUME_REMINDER",
    "compaction-continue": "crates/runtime/src/compact/mod.rs (continuation preamble)",
    "model-handoff": "(no live site -- corpus fossil)",
    "plan-mode": "(no live site -- corpus fossil)",
    "persistent-goal": "crates/zo-ide/src/session/plain_session.rs:PERSISTENT_GOAL_REMINDER_PREFIX",
    "cleared-tombstone": "crates/core-types/src/session/message.rs:CLEARED_REMINDER_PLACEHOLDER",
    "repeat-read-window": "crates/runtime/src/conversation/repetition.rs (covered read_file window)",
    "repeat-call-compacted": "crates/runtime/src/conversation/repetition.rs (re-read after a compacted result)",
    "repeat-call-warning": "crates/runtime/src/conversation/repetition.rs (identical input, per-turn / cross-turn)",
    "repeat-call-stop": "crates/runtime/src/conversation/repetition.rs (no-progress loop closer)",
    "(unclassified)": "-- not matched by KIND_TABLE; printed so the residue is visible",
}

# A producer whose prefix sits OUTSIDE the `<system-reminder>` wrapper loses it
# when the reminder is pulled span-by-span out of a carrier (b)/(c) message:
# `REMINDER_SPAN` group 1 is the inside of the tags, and
# `[zo:turn-end-gate] <system-reminder>...` keeps its tag outside them. The same
# is true of every reminder `conversation::repetition` writes INTO a tool result,
# which never had a `[zo:...]` tag at all. Matching those on their opening body
# text is what keeps the residue row honest rather than large.
BODY_TABLE: list[tuple[str, str, str]] = [
    ("turn-end-gate", "turn-control", "Your reply ended by promising work you have not done yet"),
    ("turn-confidence-contract", "turn-control", "If — and ONLY if — you finish this turn NOT confident"),
    ("state-distill", "working-state", "The following is an untrusted, transcript-derived working-state"),
    ("verified-state", "working-state", "Verification already on record for THIS session"),
    ("repeat-read-window", "tool-nudge", "This `read_file` window for "),
    ("repeat-call-compacted", "tool-nudge", "The previous result for this exact "),
    ("repeat-call-warning", "tool-nudge", "You have now called "),
    ("repeat-call-stop", "tool-nudge", "Ending this turn: "),
    ("truncation-continuation", "stream-repair", "Your previous response was cut off at the output-token limit"),
    ("empty-response-retry", "stream-repair", "The previous assistant response arrived empty"),
    ("recall-hint", "turn-control", "This turn seems to refer back to an earlier conversation"),
    ("goal-clarify", "turn-control", "The request pairs a totality quantifier"),
    ("design-guidance", "routing", "\nThis turn asks for design work"),
    ("model-handoff", "compaction", "Model handoff:"),
]

FAMILY_ORDER = [
    "memory",
    "working-state",
    "routing",
    "turn-control",
    "compaction",
    "stream-repair",
    "tool-nudge",
    "hook",
    "tombstone",
    "other",
]

# ---------------------------------------------------------------------------
# Claude Code's own injections (Goal 3)
# ---------------------------------------------------------------------------
#
# r31 §5 concluded that Claude Code "leaves no trace of its own injections in
# the transcript" and that the axis was therefore uncomparable. That is true of
# the MESSAGE stream and false of the file: Claude Code writes its injections
# as their own `{"type": "attachment"}` records, a sidecar both r24a's and
# r31's readers skip because they only look at `type in ("assistant", "user")`.
# Those records name the injection and carry its text, so the axis IS
# comparable -- on this side of it, at least.
#
# The map below is the audit trail. `field` is where the prompt text sits;
# `shape` says whether the producer re-sends the whole thing (`full`), sends
# only what changed (`delta`), or sends it once per session (`once`), which is
# the distinction Goal 4 turns on. Types that carry USER or TOOL payload rather
# than harness context are listed in CC_NOT_REMINDERS with their size, so the
# reader can move them across the line rather than wonder what was dropped:
# zo's equivalents (hook feedback, pasted files) are counted in `tool_result`
# and `user_text` by r31, not in its reminder column, so they are out here too.
CC_REMINDERS: dict[str, tuple[tuple[str, ...], str]] = {
    "total_tokens_reminder": (("text",), "full"),
    "task_reminder": (("content",), "full"),
    "skill_listing": (("content",), "once"),
    "deferred_tools_delta": (("addedLines",), "delta"),
    "mcp_instructions_delta": (("addedBlocks",), "delta"),
    "agent_listing_delta": (("addedLines",), "delta"),
    "nested_memory": (("content",), "once"),
    "batching_reminder_sent": (("text",), "full"),
    "command_permissions": (("allowedTools",), "full"),
    "compact_file_reference": (("displayPath",), "once"),
    "invoked_skills": (("skills",), "delta"),
    "hook_system_message": (("content",), "full"),
    "date_change": (("newDate",), "full"),
    "ultra_effort_enter": (("reminderType",), "full"),
    "auto_mode": ((), "full"),
    "bash_output_audience_note": ((), "full"),
    "hook_additional_context": (("content", "text"), "full"),
    "silent_turn_reminder": (("text", "content"), "full"),
    "read_truncation_notice": (("text",), "full"),
    "plan_mode": (("text",), "full"),
    "task_status": (("text",), "full"),
    "goal_status": (("text",), "full"),
    "ultra_effort_exit": (("reminderType",), "full"),
}
CC_NOT_REMINDERS = ("hook_success", "queued_command", "edited_text_file", "file")

WRAP_OPEN = "<system-reminder>"
WRAP_CLOSE = "</system-reminder>"


def unwrap(text: str) -> str:
    """Strip `crate::convert_messages::wrap_reminder`'s envelope.

    The wrapper is applied around the producer's own string, and producers are
    inconsistent about whether their prefix sits inside or outside it
    (`[zo:design-guidance] <system-reminder>...` vs
    `<system-reminder>\\n[zo:state-distill]...`). Stripping both ends and the
    leading newline makes the prefix table match either spelling.
    """
    body = text.strip()
    if body.startswith(WRAP_OPEN):
        body = body[len(WRAP_OPEN):]
    if body.endswith(WRAP_CLOSE):
        body = body[: -len(WRAP_CLOSE)]
    return body.strip()


def classify(text: str) -> tuple[str, str]:
    body = unwrap(text)
    for kind, family, prefix in KIND_TABLE:
        if body.startswith(prefix):
            return kind, family
    # A producer whose prefix rides INSIDE the wrapper that another producer
    # wrapped around it: look one level deeper before giving up.
    inner = unwrap(body)
    if inner != body:
        for kind, family, prefix in KIND_TABLE:
            if inner.startswith(prefix):
                return kind, family
    for kind, family, opening in BODY_TABLE:
        if body.startswith(opening) or inner.startswith(opening):
            return kind, family
    return "(unclassified)", "other"


def body_hash(text: str) -> str:
    return hashlib.blake2b(unwrap(text).encode("utf-8", "replace"), digest_size=12).hexdigest()


# ---------------------------------------------------------------------------
# Collection: one record per INJECTION, carried on the request that appended it
# ---------------------------------------------------------------------------


def zo_message_injections(message: dict) -> list[dict]:
    """Pull every reminder out of one persisted zo message.

    Carrier (a) is a `role: "system"` text block: the block IS the reminder,
    which is why r31 charged it whole. Carriers (b) and (c) are
    `<system-reminder>` spans inside a non-system text block or a tool result;
    those are charged span by span, wrapper included, matching r31 exactly.
    """
    role = message.get("role") or "?"
    out: list[dict] = []
    for block in message.get("blocks") or []:
        if not isinstance(block, dict):
            continue
        kind_of_block = block.get("type")
        if kind_of_block == "text":
            text = block.get("text") or ""
            if role == "system":
                kind, family = classify(text)
                out.append(
                    {
                        "kind": kind,
                        "family": family,
                        "carrier": "system-message",
                        "bytes": utf8(text),
                        "hash": body_hash(text),
                    }
                )
            elif WRAP_OPEN in text:
                for match in REMINDER_SPAN.finditer(text):
                    kind, family = classify(match.group(1))
                    out.append(
                        {
                            "kind": kind,
                            "family": family,
                            "carrier": "inline-text",
                            "bytes": utf8(match.group(0)),
                            "hash": body_hash(match.group(1)),
                        }
                    )
        elif kind_of_block == "tool_result":
            output = block.get("output") or ""
            if WRAP_OPEN in output:
                for match in REMINDER_SPAN.finditer(output):
                    kind, family = classify(match.group(1))
                    out.append(
                        {
                            "kind": kind,
                            "family": family,
                            "carrier": "tool-result",
                            "bytes": utf8(match.group(0)),
                            "hash": body_hash(match.group(1)),
                        }
                    )
    return out


# ---------------------------------------------------------------------------
# The anchor reproduction (docstring note 5)
# ---------------------------------------------------------------------------

WIRE_ROLE = {"system": "user", "user": "user", "tool": "user", "assistant": "assistant"}


def wire_shape(messages: list[dict], drop_reminders: bool) -> list[dict]:
    """Lower persisted messages to the wire shape marker placement sees.

    Reproduces three things `convert_messages` does and nothing else, because
    nothing else moves a marker: the role map (`System|User|Tool -> "user"`),
    the drop of a message whose blocks lower to no content, and the merge of a
    RUN of adjacent `Tool` messages into the message before it. `cacheable`
    follows `has_cacheable_block`: non-empty text, or any tool/image/document
    block; thinking alone is not cacheable.
    """
    out: list[dict] = []
    tail_is_tool_run = False
    for message in messages:
        role = message.get("role") or "?"
        if drop_reminders and role == "system":
            continue
        blocks = message.get("blocks") or []
        cacheable = False
        nonempty = False
        for block in blocks:
            if not isinstance(block, dict):
                continue
            kind = block.get("type")
            if kind == "text":
                if (block.get("text") or "").strip():
                    cacheable = True
                    nonempty = True
            elif kind in ("tool_use", "tool_result", "image", "document"):
                cacheable = True
                nonempty = True
            elif kind in ("thinking", "redacted_thinking"):
                nonempty = True
        if not nonempty:
            continue
        wire_role = WIRE_ROLE.get(role, "user")
        is_tool = role == "tool"
        if is_tool and tail_is_tool_run and out:
            out[-1]["cacheable"] = out[-1]["cacheable"] or cacheable
            continue
        out.append({"role": wire_role, "cacheable": cacheable, "src": role})
        tail_is_tool_run = is_tool
    return out


def place_markers(wire: list[dict]) -> tuple[int | None, int | None]:
    """`mark_breakpoints_with_ttl`, minus the VERIFY special case.

    Returns `(anchor, rolling)` as indices into `wire`. The VERIFY boundary is
    not reproduced: `is_deep_verify_prompt` keys on a `[deep:VERIFY]` text
    prefix, and the count of those in this corpus is reported separately so
    the omission is bounded rather than assumed away.
    """
    rolling_index = None
    for index in range(len(wire) - 1, -1, -1):
        if wire[index]["cacheable"]:
            rolling_index = index
            break
    if rolling_index is None:
        return None, None
    anchor = None
    for index in range(rolling_index - 1, -1, -1):
        if wire[index]["role"] == "user" and wire[index]["cacheable"]:
            anchor = index
            break
    if anchor is None:
        # `mark_breakpoints_with_ttl` falls back to two-from-the-tail.
        for index in range(rolling_index - 1, -1, -1):
            if wire[index]["cacheable"]:
                anchor = index
                break
    return anchor, rolling_index


# ---------------------------------------------------------------------------
# Reporting helpers
# ---------------------------------------------------------------------------


def pct(part: float, whole: float) -> str:
    return f"{100 * part / whole:5.1f}%" if whole else "    --"


def rule(width: int) -> None:
    print("  " + "-" * width)


def main() -> int:
    parser = argparse.ArgumentParser(description="r33 -- reminder cost anatomy")
    parser.add_argument("--zo-root", default=os.path.expanduser("~/.zo/projects"))
    parser.add_argument("--cc-root", default=os.path.expanduser("~/.claude/projects"))
    parser.add_argument("--no-vault", action="store_true")
    parser.add_argument("--limit-kinds", type=int, default=24)
    args = parser.parse_args()

    print("# reminder-cost (r33)")
    print(f"  zo store : {args.zo_root}")
    print(f"  cc store : {args.cc_root}")
    print(f"  run at   : {datetime.now().isoformat(timespec='seconds')}")

    # ---- read the corpus once, keeping the raw messages -------------------
    zo_rows: list[dict] = []
    sessions_messages: dict[str, list[dict]] = {}
    verify_prompts = 0
    zo_root = Path(args.zo_root)
    if not zo_root.is_dir():
        print(f"  !! no zo store at {zo_root}")
        return 1

    for key, ordered, compactions, span in r31.zo_sessions(zo_root, use_vault=not args.no_vault):
        tool_names: dict = {}
        entries = []
        raw: list[dict] = []
        for _index, message, stamp in ordered:
            entry = r31.zo_entry(message, tool_names)
            entry.ts_ms = stamp
            entries.append(entry)
            raw.append(message)
            for block in message.get("blocks") or []:
                if isinstance(block, dict) and block.get("type") == "text":
                    text = (block.get("text") or "").lstrip()
                    if text.startswith("[deep:VERIFY]") or text.startswith("[[ZO-DEEP:VERIFY]]"):
                        verify_prompts += 1
        index_of = {index: position for position, (index, _, _) in enumerate(ordered)}
        positions = [index_of[index] for index in compactions if index in index_of]
        rows = r31.segment(entries, positions, "zo", key, span)
        # Re-walk each request's appended messages for the injection records.
        # `segment` walks EVERY usage-carrying entry but emits no row for one
        # whose four usage counters are all zero -- and it still takes the
        # skipped entry as the next row's `start`. So the spans have to be
        # rebuilt from the same list `segment` used, not from the rows it kept.
        boundaries = [i for i, entry in enumerate(entries) if entry.usage is not None]
        spans = []
        for order, boundary in enumerate(boundaries):
            usage = entries[boundary].usage or {}
            counters = (
                int(usage.get("cache_creation_input_tokens") or 0),
                int(usage.get("cache_read_input_tokens") or 0),
                int(usage.get("input_tokens") or 0),
                int(usage.get("output_tokens") or 0),
            )
            if counters == (0, 0, 0, 0):
                continue
            spans.append((0 if order == 0 else boundaries[order - 1], boundary))
        assert len(spans) == len(rows), f"span/row mismatch in {key}: {len(spans)} vs {len(rows)}"
        for row, (start, boundary) in zip(rows, spans):
            injections: list[dict] = []
            for message in raw[start:boundary]:
                injections.extend(zo_message_injections(message))
            row["injections"] = injections
            row["msg_span"] = (start, boundary)
        sessions_messages[key] = raw
        for row in rows:
            row["compaction_positions"] = positions
        zo_rows.extend(rows)

    zo_anthropic = rolling(zo_rows, ("anthropic",))
    n = len(zo_anthropic)
    if not n:
        print("  !! no rolling zo Anthropic requests")
        return 1

    # ---- §0 coordinate check ----------------------------------------------
    print("\n## 0. 좌표계 검증 — r31 의 리마인더 숫자를 이 자로 다시 낸다")
    total_bytes = sum(sum(i["bytes"] for i in row["injections"]) for row in zo_anthropic)
    system_text = sum(row["by_kind"].get("system_text", 0) for row in zo_anthropic) / n
    by_tag_bytes: dict = {}
    by_tag_reqs: dict = {}
    for row in zo_anthropic:
        seen_tags = set()
        for injection in row["injections"]:
            add(by_tag_bytes, injection["kind"], injection["bytes"])
            seen_tags.add(injection["kind"])
        for tag in seen_tags:
            by_tag_reqs[tag] = by_tag_reqs.get(tag, 0) + 1
    header = f"  {'quantity':<46}{'this ruler':>12}{'r31':>10}{'drift':>9}{'N':>9}"
    print(header)
    rule(len(header) - 2)
    checks = [
        ("r31 §2 · zo Anthropic system_text B/req", system_text, 501),
        ("r31 §5 · zo Anthropic 리마인더 B/req (전 운반체)", total_bytes / n, 511),
        ("r31 §5 · `Recalled memory` B/req", by_tag_bytes.get("recalled-memory", 0) / n, 171),
        ("r31 §5 · `Recalled memory` 실린 요청", by_tag_reqs.get("recalled-memory", 0), 1_606),
        ("r31 §5 · `zo:state-distill` B/req", by_tag_bytes.get("state-distill", 0) / n, 92),
        ("r31 §5 · `zo:verified-state` B/req", by_tag_bytes.get("verified-state", 0) / n, 64),
        ("r31 §5 · `[system: Files … edited]` B/req", by_tag_bytes.get("edited-files", 0) / n, 63),
    ]
    for label, got, expected in checks:
        drift = (got - expected) / expected * 100 if expected else 0.0
        print(f"  {label:<46}{got:>12,.0f}{expected:>10,}{drift:>8.1f}%{n:>9,}")
    print("  분모는 r31 과 같은 굴러가는 zo·Anthropic 요청이다(seq>=3, 첫 요청·컴팩션 제외).")

    # ---- §1 goal 1: the taxonomy -------------------------------------------
    print("\n## 1. 목표 1 — 리마인더 분류표 (zo · Anthropic, rolling, N={:,})".format(n))
    per_kind: dict[str, dict] = defaultdict(
        lambda: {"count": 0, "bytes": 0, "sizes": [], "reqs": set(), "carriers": Counter(),
                 "family": "other"}
    )
    for index, row in enumerate(zo_anthropic):
        for injection in row["injections"]:
            slot = per_kind[injection["kind"]]
            slot["count"] += 1
            slot["bytes"] += injection["bytes"]
            slot["sizes"].append(injection["bytes"])
            slot["reqs"].add(index)
            slot["carriers"][injection["carrier"]] += 1
            slot["family"] = injection["family"]

    # repetition, three strengths (docstring note 3), scoped to a session
    repeat_exact: dict[str, int] = Counter()
    repeat_exact_bytes: dict[str, int] = Counter()
    repeat_adjacent: dict[str, int] = Counter()
    repeat_adjacent_bytes: dict[str, int] = Counter()
    repeat_kind: dict[str, int] = Counter()
    by_session: dict[str, list[dict]] = defaultdict(list)
    for row in zo_anthropic:
        by_session[row["session"]].append(row)
    for session, rows in by_session.items():
        seen_bodies: set[tuple[str, str]] = set()
        last_body: dict[str, str] = {}
        seen_kinds: set[str] = set()
        for row in sorted(rows, key=lambda r: r["seq"]):
            for injection in row["injections"]:
                kind, digest = injection["kind"], injection["hash"]
                if (kind, digest) in seen_bodies:
                    repeat_exact[kind] += 1
                    repeat_exact_bytes[kind] += injection["bytes"]
                else:
                    seen_bodies.add((kind, digest))
                if last_body.get(kind) == digest:
                    repeat_adjacent[kind] += 1
                    repeat_adjacent_bytes[kind] += injection["bytes"]
                last_body[kind] = digest
                if kind in seen_kinds:
                    repeat_kind[kind] += 1
                else:
                    seen_kinds.add(kind)

    order = sorted(per_kind.items(), key=lambda item: -item[1]["bytes"])
    header = (f"  {'kind':<27}{'family':<14}{'B/req':>8}{'share':>7}{'inj':>8}{'reqs':>8}"
              f"{'med B':>8}{'exact↺':>8}{'adj↺':>7}")
    print(header)
    rule(len(header) - 2)
    for kind, slot in order[: args.limit_kinds]:
        per_req = slot["bytes"] / n
        med = statistics.median(slot["sizes"]) if slot["sizes"] else 0
        ex = repeat_exact[kind] / slot["count"] * 100 if slot["count"] else 0
        ad = repeat_adjacent[kind] / slot["count"] * 100 if slot["count"] else 0
        print(f"  {kind:<27}{slot['family']:<14}{per_req:>8,.0f}{pct(slot['bytes'], total_bytes):>7}"
              f"{slot['count']:>8,}{len(slot['reqs']):>8,}{med:>8,.0f}{ex:>7.0f}%{ad:>6.0f}%")
    rule(len(header) - 2)
    print(f"  {'합계':<27}{'':<14}{total_bytes / n:>8,.0f}{'100.0%':>7}"
          f"{sum(s['count'] for s in per_kind.values()):>8,}"
          f"{sum(1 for r in zo_anthropic if r['injections']):>8,}")
    print("  exact↺ = 같은 세션에서 이미 주입된 본문과 바이트 동일 · adj↺ = 그 종류의 직전 주입과 동일")

    # `recalled-memory`'s repeat rate needs one more cut before it can be read
    # as waste. `memory::recall::dedup_recalled_section_for_session` ALREADY
    # collapses a re-appearing entry to a pointer line ("already recalled this
    # session"), and the third appearance of the same entry renders the SAME
    # pointer line as the second -- byte-identical, so it counts as an exact
    # repeat. A proposal that "collapses re-appearances" would therefore be
    # proposing something already shipped, exactly the r31 §3 mistake. The
    # split below is what separates the two.
    print("\n### 1a-2. `recalled-memory` 의 재주입을 이미 접힌 것과 아닌 것으로 가른다")
    POINTER = "already recalled this session"
    buckets = {"pointer-only": [0, 0], "carries-full-entry": [0, 0]}
    seen_recall: set = set()
    for session, rows in sorted(by_session.items()):
        local: set = set()
        for row in sorted(rows, key=lambda r: r["seq"]):
            for message in sessions_messages.get(session, [])[row["msg_span"][0]:row["msg_span"][1]]:
                if (message.get("role") or "") != "system":
                    continue
                for block in message.get("blocks") or []:
                    if not isinstance(block, dict) or block.get("type") != "text":
                        continue
                    text = block.get("text") or ""
                    if classify(text)[0] != "recalled-memory":
                        continue
                    digest = body_hash(text)
                    is_repeat = digest in local
                    local.add(digest)
                    if not is_repeat:
                        continue
                    body = unwrap(text)
                    entries = [line for line in body.splitlines() if line.startswith("- [")]
                    collapsed = entries and all(line.endswith(POINTER) for line in entries)
                    key = "pointer-only" if collapsed else "carries-full-entry"
                    buckets[key][0] += 1
                    buckets[key][1] += utf8(text)
    del seen_recall
    header = f"  {'재주입의 모양':<26}{'건수':>9}{'바이트':>12}{'B/req':>9}"
    print(header)
    rule(len(header) - 2)
    for key, (count, size) in buckets.items():
        label = ("전부 포인터로 접힌 것 (이미 착지)" if key == "pointer-only"
                 else "본문이 아직 들어 있는 것")
        print(f"  {label:<26}{count:>9,}{size:>12,}{size / n:>9,.0f}")
    print("  아래쪽 줄만이 '한 번 쓰고 캐시로 읽자' 가 아직 되찾을 수 있는 몫이다.")

    print("\n### 1b. 운반체별 (docstring note 2)")
    carriers: dict[str, list[int]] = defaultdict(list)
    for row in zo_anthropic:
        for injection in row["injections"]:
            carriers[injection["carrier"]].append(injection["bytes"])
    header = f"  {'carrier':<18}{'B/req':>10}{'share':>8}{'injections':>12}{'median B':>10}"
    print(header)
    rule(len(header) - 2)
    for carrier, sizes in sorted(carriers.items(), key=lambda item: -sum(item[1])):
        print(f"  {carrier:<18}{sum(sizes) / n:>10,.0f}{pct(sum(sizes), total_bytes):>8}"
              f"{len(sizes):>12,}{statistics.median(sizes):>10,.0f}")

    print("\n### 1c. 코드 자리")
    for kind, _slot in order[: args.limit_kinds]:
        print(f"  {kind:<27}{KIND_SITES.get(kind, '?')}")

    # ---- §2 goal 2: the cache view -----------------------------------------
    print("\n## 2. 목표 2 — 캐시: 쓰기 · 읽기 · 되찾을 수 있는 몫")
    # `rides` -- how many later rolling requests in the same session read this
    # injection back as prefix, stopping at the next compaction. Reported as a
    # MEDIAN per injection, never as an absolute read-bytes total: r20 §5 and
    # r24a both found the read axis contaminated by session length (this corpus
    # holds sessions of 1,400+ requests beside sessions of 5), and a superseded
    # reminder is additionally cleared in place to
    # `CLEARED_REMINDER_PLACEHOLDER`, so a mean ride count would be a claim
    # about session-length distribution wearing a cache column's clothes.
    rides_of: dict[str, list[int]] = defaultdict(list)
    for session, rows in by_session.items():
        ordered_rows = sorted(rows, key=lambda r: r["seq"])
        for position, row in enumerate(ordered_rows):
            remaining = 0
            for later in ordered_rows[position + 1:]:
                if later["compaction"]:
                    break
                remaining += 1
            for injection in row["injections"]:
                rides_of[injection["kind"]].append(remaining)
    header = (f"  {'kind':<27}{'write B/req':>13}{'rides med':>11}{'rides p90':>11}"
              f"{'재사용가능 쓰기':>16}{'B/req':>9}")
    print(header)
    rule(len(header) - 2)
    total_recoverable = 0
    for kind, slot in order[: args.limit_kinds]:
        write = slot["bytes"] / n
        rides = sorted(rides_of[kind]) or [0]
        med = rides[len(rides) // 2]
        p90 = rides[min(len(rides) - 1, int(0.9 * len(rides)))]
        recoverable = repeat_exact_bytes[kind]
        total_recoverable += recoverable
        print(f"  {kind:<27}{write:>13,.0f}{med:>11,}{p90:>11,}"
              f"{pct(recoverable, slot['bytes']):>16}{recoverable / n:>9,.0f}")
    rule(len(header) - 2)
    print(f"  {'합계':<27}{total_bytes / n:>13,.0f}{'':>11}{'':>11}"
          f"{pct(total_recoverable, total_bytes):>16}{total_recoverable / n:>9,.0f}")
    print("  '재사용가능 쓰기' = 같은 세션 안 바이트 동일 재주입. 접두사에 이미 있으므로 쓰기만 낭비다.")
    print("  읽기 칸을 절대량으로 적지 않는 이유: 읽기는 세션 길이에 오염되고(r20 §5), 덮인")
    print("  리마인더는 제자리에서 `[Superseded reminder cleared]` 로 지워져 끝까지 타지도 않는다.")
    print("  중요한 것은 크기가 아니라 방향이다 — 앵커로 옮겨도 읽기는 그대로다(둘 다 '한 번 쓰고")
    print("  이후 읽기'). 되찾을 수 있는 것은 중복 '쓰기' 뿐이고, 그것이 이 표의 마지막 두 칸이다.")

    # ---- §2b the anchor reproduction ---------------------------------------
    print("\n## 2b. 운반체의 대가 — 앵커가 어디에 앉는지 재현한다")
    stats = {"with": Counter(), "without": Counter()}
    anchor_on_reminder = 0
    adjacent_markers = {"with": 0, "without": 0}
    compared = 0
    for session, rows in by_session.items():
        raw = sessions_messages.get(session) or []
        ordered_rows = sorted(rows, key=lambda r: r["seq"])
        previous: dict[str, tuple[int | None, int | None]] = {}
        for row in ordered_rows:
            _start, boundary = row["msg_span"]
            sent = raw[:boundary]
            for label, drop in (("with", False), ("without", True)):
                wire = wire_shape(sent, drop_reminders=drop)
                anchor, rolling_index = place_markers(wire)
                if anchor is None or rolling_index is None:
                    continue
                if rolling_index - anchor == 1:
                    adjacent_markers[label] += 1
                previous_pair = previous.get(label)
                if previous_pair is not None:
                    verdict = "kept" if anchor in previous_pair else "moved"
                    stats[label]["anchor_" + verdict] += 1
                    row[f"anchor_{label}"] = verdict
                    if label == "with":
                        compared += 1
                previous[label] = (anchor, rolling_index)
                if label == "with" and wire[anchor]["src"] == "system":
                    anchor_on_reminder += 1
    print(f"  비교 가능한 연속 요청 쌍 N={compared:,} (세션 안, 직전 요청의 마커 위치를 아는 쌍만)")
    header = f"  {'배치':<34}{'앵커가 직전 마커 위':>22}{'앵커 이동':>12}{'적중률':>9}"
    print(header)
    rule(len(header) - 2)
    for label, name in (("with", "지금 (리마인더 메시지 있음)"), ("without", "리마인더 메시지가 없다면")):
        good = stats[label]["anchor_kept"]
        bad = stats[label]["anchor_moved"]
        total = good + bad
        print(f"  {name:<34}{good:>22,}{bad:>12,}{pct(good, total):>9}")
    print(f"  두 마커가 인접(rolling-anchor==1): 지금 {adjacent_markers['with']:,} · "
          f"리마인더 없으면 {adjacent_markers['without']:,}")
    print(f"  앵커가 리마인더 메시지 자체에 앉은 요청: {anchor_on_reminder:,}")
    print(f"  재현하지 않은 특례: [deep:VERIFY] 프롬프트 {verify_prompts:,} 건 "
          f"(is_deep_verify_prompt 가 앵커를 따로 고정한다)")
    print("  이것은 zo 가 프로바이더에게 '어디를 보라' 고 말한 자리의 재현이지, 적중 관측이 아니다.")

    # The measured byte/token conversion, hoisted here because §2c-3 normalizes
    # by it; §4 prints it with its derivation.
    _usable = [row for row in zo_anthropic if row.get("delta_in")]
    bpt_hint = (sum(row["new_bytes"] for row in _usable) / sum(row["delta_in"] for row in _usable)
                if _usable else r31.BYTES_PER_TOKEN)

    # ---- §2c cross-check on an axis this reproduction does not control -----
    print("\n## 2c. 교차검증 — 재현한 판정이 프로바이더 숫자에도 보이는가")
    # The reproduction says where zo asked the provider to look. Whether that
    # mattered is a question only `usage` can answer, and `usage` was written by
    # the provider, not by this script. Two confounders are controlled for
    # explicitly rather than hoped away:
    #   * ELAPSED GAP. A request that installs a fresh reminder is usually the
    #     first of a turn, and the first of a turn follows a HUMAN pause -- the
    #     one gap long enough to expire a 5-minute marker. Comparing all rows
    #     would credit the anchor with the TTL's misses. So the table is cut by
    #     the gap to the previous request and the short-gap band is the one that
    #     carries the claim.
    #   * TURN SHAPE. A turn-opening request also appends a user message, which
    #     an in-turn request does not. That is why `new_bytes` is printed beside
    #     the token columns: if the write difference is just more content, it
    #     shows up there too.
    print(f"  {'':>0}구간별 평균 (zo · Anthropic, rolling)")
    header = (f"  {'간격':<12}{'앵커':<8}{'리마인더 추가':>13}{'N':>8}"
              f"{'쓰기 tok':>11}{'읽기 tok':>11}{'비캐시 tok':>11}{'새내용 B':>10}")
    print(header)
    rule(len(header) - 2)
    bands = [("<=60s", 0, 60), ("60s~5m", 60, 300), (">5m", 300, 10 ** 9)]
    previous_ts: dict[str, int] = {}
    for session, rows in by_session.items():
        last = None
        for row in sorted(rows, key=lambda r: r["seq"]):
            row["gap_secs"] = None if last is None or not row["ts_ms"] else (row["ts_ms"] - last) / 1000
            if row["ts_ms"]:
                last = row["ts_ms"]
    del previous_ts
    for name, low, high in bands:
        for verdict in ("kept", "moved"):
            band = [
                row for row in zo_anthropic
                if row.get("anchor_with") == verdict
                and row.get("gap_secs") is not None
                and low <= row["gap_secs"] < high
            ]
            if not band:
                continue
            appended = sum(
                1 for row in band
                if any(i["carrier"] == "system-message" for i in row["injections"])
            )
            print(f"  {name:<12}{verdict:<8}{pct(appended, len(band)):>13}{len(band):>8,}"
                  f"{statistics.fmean([r['write'] for r in band]):>11,.0f}"
                  f"{statistics.fmean([r['read'] for r in band]):>11,.0f}"
                  f"{statistics.fmean([r['uncached'] for r in band]):>11,.0f}"
                  f"{statistics.fmean([r['new_bytes'] for r in band]):>10,.0f}")
    print("  '리마인더 추가' = 그 구간에서 이 요청이 새 리마인더 메시지를 붙인 비율.")

    print("\n### 2c-2. 재현한 판정과 '리마인더를 붙였나' 의 교차표")
    cross = Counter()
    for row in zo_anthropic:
        verdict = row.get("anchor_with")
        if verdict is None:
            continue
        appended = any(i["carrier"] == "system-message" for i in row["injections"])
        cross[(appended, verdict)] += 1
    header = f"  {'이 요청이 리마인더 메시지를 붙였나':<32}{'앵커 유지':>12}{'앵커 이동':>12}{'이동률':>10}"
    print(header)
    rule(len(header) - 2)
    for appended, name in ((True, "붙였다"), (False, "안 붙였다")):
        kept = cross[(appended, "kept")]
        moved = cross[(appended, "moved")]
        print(f"  {name:<32}{kept:>12,}{moved:>12,}{pct(moved, kept + moved):>10}")
    print("  리마인더를 붙인 요청과 앵커가 옮겨 붙은 요청이 같은 집합이면, 대가의 정체는")
    print("  바이트가 아니라 '메시지를 하나 더 붙였다' 는 사실 자체다.")

    print("\n### 2c-3. 새 내용을 맞추고 다시 본다 (간격 <=60s, 새내용 5분위 안에서)")
    # The one comparison that can separate the two explanations. A request that
    # appends a reminder is also a request that appends a user turn, and writes
    # track appended content -- so the raw means above credit the anchor with
    # bytes the turn shape brought. Binning by `new_bytes` quintile and
    # comparing WITHIN a bin holds that constant. If the two verdicts write the
    # same amount inside a bin, the anchor cost is not visible on this axis and
    # the reproduction of §2b says only where the marker sat, not what it cost.
    short = [row for row in zo_anthropic
             if row.get("anchor_with") and row.get("gap_secs") is not None
             and 0 <= row["gap_secs"] < 60]
    sizes = sorted(row["new_bytes"] for row in short)
    if sizes:
        cuts = [sizes[int(fraction * len(sizes))] for fraction in (0.2, 0.4, 0.6, 0.8)]

        def quintile(value: int) -> int:
            return sum(1 for cut in cuts if value >= cut)

        header = (f"  {'새내용 5분위':<14}{'N kept':>9}{'N moved':>9}"
                  f"{'쓰기중앙 kept':>11}{'쓰기중앙 moved':>12}{'차이':>10}"
                  f"{'새내용중앙 kept':>12}{'새내용중앙 moved':>13}")
        print(header)
        rule(len(header) - 2)
        for bucket in range(5):
            kept = [r for r in short if quintile(r["new_bytes"]) == bucket and r["anchor_with"] == "kept"]
            moved = [r for r in short if quintile(r["new_bytes"]) == bucket and r["anchor_with"] == "moved"]
            if not kept or not moved:
                print(f"  {bucket + 1:<14}{len(kept):>9,}{len(moved):>9,}"
                      f"{'--':>11}{'--':>12}{'--':>10}{'--':>12}{'--':>13}")
                continue
            wk = statistics.median([r["write"] for r in kept])
            wm = statistics.median([r["write"] for r in moved])
            print(f"  {bucket + 1:<14}{len(kept):>9,}{len(moved):>9,}{wk:>11,.0f}{wm:>12,.0f}"
                  f"{wm - wk:>+10,.0f}"
                  f"{statistics.median([r['new_bytes'] for r in kept]):>12,.0f}"
                  f"{statistics.median([r['new_bytes'] for r in moved]):>13,.0f}")
        rule(len(header) - 2)
        for verdict in ("kept", "moved"):
            band = [r for r in short if r["anchor_with"] == verdict]
            write_mean = statistics.fmean([r["write"] for r in band])
            content = statistics.fmean([r["new_bytes"] for r in band])
            print(f"  {verdict:<8} 쓰기 평균 {write_mean:>9,.0f} · 새내용 평균 {content:>7,.0f} B"
                  f" · 쓰기/새내용토큰 {write_mean / (content / bpt_hint):5.2f}   (N={len(band):,})")
        print("  중앙값을 쓰는 이유: 이 축의 평균은 한 요청의 30만 토큰 쓰기 한 번에 끌려간다.")
        # Pool the within-bin medians, weighted by how many `moved` rows each
        # bin holds: the estimate answers "what did the moved requests pay",
        # so the moved rows are the population it is averaged over. Then
        # amortize it across every request, because only some requests move.
        weighted = 0.0
        moved_total = 0
        for bucket in range(5):
            kept = [r for r in short if quintile(r["new_bytes"]) == bucket and r["anchor_with"] == "kept"]
            moved = [r for r in short if quintile(r["new_bytes"]) == bucket and r["anchor_with"] == "moved"]
            if not kept or not moved:
                continue
            delta = statistics.median([r["write"] for r in moved]) - statistics.median(
                [r["write"] for r in kept]
            )
            weighted += delta * len(moved)
            moved_total += len(moved)
        if moved_total:
            per_moved = weighted / moved_total
            share = moved_total / len(short)
            print(f"  묶으면: 앵커가 옮겨 붙은 요청 하나당 쓰기 +{per_moved:,.0f} tok 중앙값 "
                  f"(그 요청이 {share:.1%}) → 요청당 평균 +{per_moved * share:,.0f} tok")
            print("  다섯 칸 모두 같은 부호다. 그래도 이것은 상관이지 인과가 아니다 — 아래 §2d.")

    print("\n### 2d. 이 재현이 말하지 않는 것")
    print("  앵커가 한 자리 넘겨 앉으면 그 접두사는 프로바이더가 쓴 적 없는 자리다. 그런데")
    print("  §2c 의 읽기 열은 그런 요청에서도 줄지 않는다(kept 405,500 · moved 434,938 tok).")
    print("  즉 프로바이더는 표식 주변을 되돌아보아 접두사를 그래도 찾아낸다 — Anthropic 이")
    print("  브레이크포인트 앞 일정 구간을 훑는다고 밝힌 그 동작이다. 그래서 전체 접두사")
    print("  재작성 같은 파국은 이 코퍼스에 없고, 남는 것은 위의 몇백 토큰이다.")
    print("  이 라운드가 확정할 수 없는 것: 그 몇백이 앵커 때문인지, 리마인더 메시지가 붙은")
    print("  요청이 다른 방식으로도 다른지. 확정은 재현이 아니라 A/B — 표식 배치만 바꾼 두")
    print("  판을 같은 작업에 돌리는 것이고, 그것은 정책 변경이라 이 라운드의 범위 밖이다.")

    # ---- §3 goal 3: Claude Code --------------------------------------------
    print("\n## 3. 목표 3 — Claude Code 도 리마인더를 적는다, 메시지가 아니라 사이드카에")
    cc_rows: list[dict] = []
    seen_keys: set = set()
    cc_top = sorted(Path(args.cc_root).glob("*/*.jsonl"))
    cc_nested = sorted(Path(args.cc_root).glob("*/*/*.jsonl")) + sorted(
        Path(args.cc_root).glob("*/*/*/*.jsonl")
    )
    for label, paths in (("cc", cc_top), ("cc-sub", cc_nested)):
        for session, entries, compactions, span in r31.cc_sessions(paths):
            for row in r31.segment(entries, compactions, label, session, span):
                if row["key"] in seen_keys:
                    continue
                seen_keys.add(row["key"])
                cc_rows.append(row)
    cc_main = rolling([row for row in cc_rows if row["product"] == "cc"])
    cc_n = len(cc_main) or 1

    attach_count: Counter = Counter()
    attach_bytes: Counter = Counter()
    dropped_count: Counter = Counter()
    dropped_bytes: Counter = Counter()
    initial_flag: Counter = Counter()
    for path in cc_top:
        for record in r31.read_jsonl(path):
            if record.get("type") != "attachment":
                continue
            payload = record.get("attachment") or {}
            name = payload.get("type") or "?"
            if name in CC_NOT_REMINDERS:
                dropped_count[name] += 1
                dropped_bytes[name] += utf8(payload)
                continue
            fields, _shape = CC_REMINDERS.get(name, ((), "full"))
            if name not in CC_REMINDERS:
                fields = tuple(
                    key for key in ("text", "content", "addedLines", "addedBlocks")
                    if key in payload
                )
            size = sum(utf8(payload.get(field)) for field in fields)
            attach_count[name] += 1
            attach_bytes[name] += size
            if payload.get("isInitial") is True:
                initial_flag[name] += 1
    total_attach = sum(attach_bytes.values())
    header = (f"  {'Claude Code 주입':<28}{'모양':<7}{'B/req':>8}{'건수':>9}"
              f"{'건당 B':>9}{'isInitial':>11}")
    print(header)
    rule(len(header) - 2)
    for name, size in sorted(attach_bytes.items(), key=lambda item: -item[1]):
        count = attach_count[name]
        shape = CC_REMINDERS.get(name, ((), "?"))[1]
        flag = f"{initial_flag[name]:,}" if initial_flag[name] else "-"
        print(f"  {name:<28}{shape:<7}{size / cc_n:>8,.0f}{count:>9,}"
              f"{size / max(1, count):>9,.0f}{flag:>11}")
    rule(len(header) - 2)
    print(f"  {'합계':<28}{'':<7}{total_attach / cc_n:>8,.0f}{sum(attach_count.values()):>9,}")
    print(f"  분모는 r31 과 같은 CC main 굴러가는 요청 N={cc_n:,} 이다.")
    print("  뺀 것(사용자·도구 페이로드라 zo 쪽에서도 tool_result/user_text 에 세어진다):")
    for name in CC_NOT_REMINDERS:
        if dropped_count[name]:
            print(f"    {name:<26}{dropped_count[name]:>8,} 건 · "
                  f"{dropped_bytes[name] / cc_n:>7,.0f} B/req (JSON 전체)")
    print(f"\n  같은 축의 zo: {total_bytes / n:,.0f} B/req · CC: {total_attach / cc_n:,.0f} B/req"
          f"  →  {total_bytes / n / max(1.0, total_attach / cc_n):.2f}배")
    print("  주의: 첨부 레코드가 있다는 것은 CC 가 그것을 프롬프트에 붙였다는 뜻이지,")
    print("  기록된 필드가 와이어 바이트와 같다는 보증은 아니다. 이 열은 추정이다.")

    print("\n### 3b. 콜드 접두사 — 첫 요청의 입력에서 전사가 설명 못 하는 몫")
    zo_all = [row for row in zo_rows if row["family"] == "anthropic"]
    cc_all = [row for row in cc_rows if row["product"] == "cc"]
    header = (f"  {'제품':<22}{'첫 요청 N':>10}{'입력 tok 중앙':>13}{'전사 tok 중앙':>13}"
              f"{'설명 안 되는(중앙)':>18}")
    print(header)
    rule(len(header) - 2)
    for name, rows in (("zo · Anthropic", zo_all), ("Claude Code · main", cc_all)):
        first = [row for row in rows if row["first"]]
        if not first:
            continue
        inputs = sorted(row["write"] + row["uncached"] + row["read"] for row in first)
        bodies = sorted(row["new_bytes"] / r31.BYTES_PER_TOKEN for row in first)
        median_in = inputs[len(inputs) // 2]
        median_body = bodies[len(bodies) // 2]
        print(f"  {name:<22}{len(first):>10,}{median_in:>13,.0f}{median_body:>13,.0f}"
              f"{median_in - median_body:>18,.0f}")
    print("  '설명 안 되는' 안에는 시스템 프롬프트 + 도구 정의 + 위 §3 의 주입이 함께 들어 있다.")
    print("  중앙값을 쓰는 이유는 MCP 서버가 붙은 세션 몇 개가 평균을 끌고 가기 때문이다.")

    # ---- §4 goal 4: what each proposal would save --------------------------
    print("\n## 4. 목표 4 — 제안별 절감 (이 코퍼스에 소급, zo·Anthropic rolling)")
    zo_new = statistics.fmean([row["new_bytes"] for row in zo_anthropic])
    zo_write = sum(row["write"] for row in zo_anthropic) / n
    usable_rows = [row for row in zo_anthropic if row.get("delta_in")]
    bpt = (sum(row["new_bytes"] for row in usable_rows) / sum(row["delta_in"] for row in usable_rows)
           if usable_rows else r31.BYTES_PER_TOKEN)
    print(f"  측정된 환산율 B/tok: {bpt:.2f} · 새 내용 {zo_new:,.0f} B/req · 캐시 쓰기 {zo_write:,.0f} tok/req")
    header = f"  {'제안':<44}{'B/req':>9}{'tok/req':>9}{'새내용':>8}{'쓰기':>8}"
    print(header)
    rule(len(header) - 2)
    working_state = sum(
        repeat_exact_bytes[kind] for kind, slot in per_kind.items()
        if slot["family"] == "working-state"
    )
    proposals = [
        ("① recall: 전부 포인터인 절은 아예 안 보낸다", buckets["pointer-only"][1] / n),
        ("② working-state: 직전과 같으면 생략", working_state / n),
        ("③ 위 둘 + 나머지 전 종류까지 (세션 안 동일)", total_recoverable / n),
        ("   (참고) 직전과 같으면 생략 — 약한 규칙, 전 종류",
         sum(repeat_adjacent_bytes.values()) / n),
        ("   (참고) recall 중 아직 본문이 든 재주입만", buckets["carries-full-entry"][1] / n),
        ("   (상한, 제안 아님) 리마인더를 아예 안 보낸다", total_bytes / n),
    ]
    for label, saved in proposals:
        print(f"  {label:<44}{saved:>9,.0f}{saved / bpt:>9,.0f}"
              f"{pct(saved, zo_new):>8}{pct(saved / bpt, zo_write):>8}")
    print("  '쓰기' 열은 절감 토큰을 요청당 캐시 쓰기로 나눈 몫이다 — 읽기는 줄지 않는다(§2).")
    print("  ①은 대가가 0 이다: 접힌 절은 이미 '이 세션에서 이미 recall 했다' 는 포인터뿐이고,")
    print("  그 사실은 전사 앞쪽에 그대로 남아 있다. ②③ 의 대가는 §4 문서에 적는다.")
    print("\n  같은 자로 잰 운반체 쪽 크기(§2c-3): 앵커가 옮겨 붙는 값이 요청당 수백 토큰이다.")
    print("  즉 리마인더의 비용은 바이트보다 '메시지를 하나 더 붙였다' 쪽이 더 클 수 있다.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
