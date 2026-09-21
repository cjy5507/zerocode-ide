#!/usr/bin/env python3
"""How many bytes does each product APPEND to the conversation per request?

r20 left exactly one axis where zo loses: cache WRITE per request, 8,334 for
zo's Anthropic path against Claude Code's 3,668. Two stories fit that number
and they demand opposite fixes:

  (a) VOLUME  -- zo appends more new content per request (tool-output policy).
  (b) REWRITE -- zo re-writes prefix it had already cached (marker placement,
      compaction, in-place mutation of an already-sent message).

This script measures (a) directly from both products' own transcripts, and
computes the ruler that separates (a) from (b):

    write tokens per new-content token = cache_creation / (new_bytes / 4)

  ~1  the request wrote roughly what it appended        -> volume
  >>1 the request wrote far more than it appended       -> rewrite

Run:

    python3 tools/new-content-bytes.py
    python3 tools/new-content-bytes.py --rows /tmp/rows.jsonl
    python3 tools/new-content-bytes.py --no-vault   # see note 3

Input is the two live stores, NOT anything in this repo:

    zo:           ~/.zo/projects/*/sessions/session-*.jsonl (+ .rot-*, + .vault)
    Claude Code:  ~/.claude/projects/*/*.jsonl        (top level = main agent)
                  ~/.claude/projects/*/*/*.jsonl      (nested   = sub-agents)

Definitions -- every one of these is a decision, so it is named here rather
than left implicit (the mistake `cache-remeasure-r22.md` §1 had to reverse
engineer out of r21):

1. A REQUEST is one provider model call. Both products stamp `usage` on the
   assistant message they got back, so an assistant message carrying usage IS
   a request. Claude Code writes one transcript row per content BLOCK of a
   single response, repeating `usage` on each; rows sharing
   `(message.id, requestId)` are folded into one request (r19 §2).

2. NEW CONTENT of request k is every message the conversation gained between
   request k-1 and request k -- that is, the assistant answer of k-1 plus the
   tool results, user turns and reminder blocks appended after it. Formally
   the messages at indices [boundary(k-1), boundary(k)). This is exactly the
   span the provider cannot have cached from request k-1's prefix, so it is
   what `cache_creation` for request k should be paying for.

   The FIRST request of a session has no predecessor: its "new content" is the
   whole cold prefix. Those are reported separately and never mixed into the
   rolling distribution -- r22 §3 got 4 requests that were all session-firsts
   and could not answer the rolling question with them.

3. zo's persisted transcript is NOT a faithful record of what each live request
   carried. `microcompact_session` rewrites old tool-result bodies to
   "[Old tool result content cleared]" IN PLACE and persists that, and full
   compaction evicts messages entirely. Both seal the originals into a sibling
   `*.vault.jsonl` keyed by `vault_seq` (same index domain as `turn_index`), so
   this script restores from the vault by default and reports how many bytes
   that recovered. `--no-vault` shows the un-restored number; the gap between
   the two is the size of the hazard. Segments that still contain a cleared
   placeholder after restoration are counted but flagged.

4. BYTES are payload bytes, not serialized-envelope bytes: the two products
   spell the same content in different JSON (zo `blocks`/`output`, Claude Code
   `content`/`content`), and comparing envelopes would measure the spelling.
   Per block: text -> the text; thinking -> thinking + signature; tool_use ->
   name + input JSON; tool_result -> the flattened result text; image -> the
   base64 payload, kept in its OWN column because images do not tokenize at
   ~4 bytes/token and would corrupt the token conversion.

5. TOKENS are bytes/4, the repo's own rough estimator (the same one
   `wire_prefix_stability_measure.rs` quotes). It is wrong per-request in both
   directions -- Korean text in this corpus runs ~1.5-3 bytes/token as UTF-8,
   English prose nearer 4, JSON and code nearer 3 -- so a ratio built on it is
   an order-of-magnitude instrument, never a 10% one.

Known asymmetry this script CANNOT remove, stated so the reader discounts it
in the right direction: zo compresses tool results again on the way to the
wire (`context_compression::wire_tool_output`, called from `convert_messages`)
and the transcript keeps the PRE-compression body. Claude Code has no such
seam -- its tools truncate at the source and the transcript is what was sent.
So zo's byte column is an UPPER bound on what zo actually put on the wire, and
Claude Code's is the wire itself. Any conclusion of the form "zo appends less"
is therefore safe; "zo appends more" needs the wire discount measured before
it is trusted.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import statistics
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

# ---------------------------------------------------------------------------
# Definitions
# ---------------------------------------------------------------------------

# The repo's own byte->token estimator. See docstring note 5.
BYTES_PER_TOKEN = 4

# Session-relative index at or above which a request is "rolling": it is not
# the cold first call, and not the second one that is still filling a nearly
# empty conversation. r22 §3 named this axis after finding that a sample of
# session-firsts cannot answer a question about rolling markers.
ROLLING_MIN_SEQ = 3

# In-place mutation markers. A segment containing one of these was rewritten
# after it was sent, so its byte count is a floor, not a measurement.
ZO_CLEARED_MARKERS = (
    "[Old tool result content cleared]",
    "[Superseded reminder cleared]",
    "[Old pasted image cleared to save context",
)

ZO_ROT_SUFFIX = re.compile(r"\.rot-\d+\.jsonl$")

# `session-<epoch_ms>-<n>`. Only some zo session ids carry a clock at all
# (r22 §7: the rest are `agent-N` or a UUID), which is why the period split
# below falls back through several sources instead of trusting this one.
ZO_SESSION_EPOCH_MS = re.compile(r"session-(\d{13})-\d+$")

# The anchor fix landed 2026-08-27; r20 §1 splits its table there and reports
# 8,334 write/req after it against 33,881 for the whole period. Splitting the
# transcript corpus at the same instant is what makes the two tables comparable.
DEFAULT_SINCE = "2026-08-27T00:00"


def tokens(byte_count: float) -> float:
    return byte_count / BYTES_PER_TOKEN


def model_family(model: str | None) -> str:
    name = (model or "").lower()
    if name.startswith("claude"):
        return "anthropic"
    if name.startswith(("gpt", "o1", "o3", "codex")):
        return "openai"
    if name.startswith("gemini"):
        return "google"
    return "unknown"


# ---------------------------------------------------------------------------
# One normalized message, shared by both readers
# ---------------------------------------------------------------------------


@dataclass
class Entry:
    """A conversation message in the one shape both readers produce."""

    role: str
    text_bytes: int = 0
    image_bytes: int = 0
    # Payload bytes split by what produced them, so a gap can be attributed.
    by_kind: dict = field(default_factory=dict)
    # tool_name -> [CHARACTERS of each individual result], for the policy
    # table. A LIST, not a sum: the policy question is "how big does ONE result
    # get", and a request holding four results of the same tool would otherwise
    # report their total as a single observation.
    #
    # Characters, not bytes, because every cap in the code is denominated in
    # `chars().count()` (`TruncationConfig::default_max_chars`,
    # `MAX_TOOL_ERROR_CHARS`, `AGENT_RESULT_RELAY_CHARS`). Measuring this table
    # in bytes made 30,000-char Korean results look like 50,000-"char" cap
    # violations -- UTF-8 spends 3 bytes on a Hangul syllable, and this corpus
    # is full of them.
    tool_bytes: dict = field(default_factory=dict)
    cleared: bool = False
    # Present only on request boundaries.
    usage: dict | None = None
    model: str | None = None
    key: object = None
    ts_ms: int | None = None


def add(mapping: dict, name: str, value: int) -> None:
    if value:
        mapping[name] = mapping.get(name, 0) + value


def append(mapping: dict, name: str, value: int) -> None:
    mapping.setdefault(name, []).append(value)


def utf8(value) -> int:
    if value is None:
        return 0
    if not isinstance(value, str):
        value = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return len(value.encode("utf-8", errors="replace"))


def flatten_result(content) -> str:
    """Claude Code's tool_result content is a string OR a block list."""
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for block in content:
            if isinstance(block, dict):
                parts.append(block.get("text") or "")
            else:
                parts.append(str(block))
        return "".join(parts)
    return json.dumps(content, ensure_ascii=False, separators=(",", ":"))


# ---------------------------------------------------------------------------
# zo reader
# ---------------------------------------------------------------------------


def zo_entry(message: dict, tool_names: dict) -> Entry:
    entry = Entry(role=message.get("role") or "?")
    for block in message.get("blocks") or []:
        if not isinstance(block, dict):
            continue
        kind = block.get("type")
        if kind == "text":
            text = block.get("text") or ""
            size = utf8(text)
            entry.text_bytes += size
            add(entry.by_kind, "assistant_text" if entry.role == "assistant" else f"{entry.role}_text", size)
            if any(marker in text for marker in ZO_CLEARED_MARKERS):
                entry.cleared = True
        elif kind == "thinking":
            size = utf8(block.get("thinking")) + utf8(block.get("signature"))
            entry.text_bytes += size
            add(entry.by_kind, "thinking", size)
        elif kind == "tool_use":
            name = block.get("name") or "?"
            tool_names[block.get("id")] = name
            size = utf8(name) + utf8(block.get("input"))
            entry.text_bytes += size
            add(entry.by_kind, "tool_use", size)
        elif kind == "tool_result":
            name = block.get("tool_name") or tool_names.get(block.get("tool_use_id")) or "?"
            output = block.get("output") or ""
            size = utf8(output)
            entry.text_bytes += size
            add(entry.by_kind, "tool_result", size)
            append(entry.tool_bytes, name, len(output))
            if any(marker in output for marker in ZO_CLEARED_MARKERS):
                entry.cleared = True
        elif kind == "image":
            size = utf8(block.get("data") or (block.get("source") or {}).get("data"))
            entry.image_bytes += size
    if message.get("role") == "assistant" and message.get("usage"):
        entry.usage = message["usage"]
        entry.model = message.get("model")
    return entry


def iso_ms(text: str | None) -> int | None:
    """Claude Code stamps every row with an ISO-8601 UTC timestamp."""
    if not text:
        return None
    try:
        from datetime import datetime

        return int(datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp() * 1000)
    except (ValueError, TypeError):
        return None


def zo_payload_size(message) -> int:
    """Crude size of one raw zo message, used only to pick between two copies
    of the same index (live vs vault). The larger copy is the un-cleared one."""
    if not isinstance(message, dict):
        return 0
    return len(json.dumps(message, ensure_ascii=False).encode("utf-8", errors="replace"))


def zo_sessions(root: Path, use_vault: bool):
    """Yield (session_key, ordered [(index, message, ts_ms)], compaction_indices,
    (session_start_ms, session_end_ms))."""
    groups: dict[str, dict] = {}
    for path in sorted(root.glob("*/sessions/*.jsonl")):
        name = path.name
        if name.endswith(".vault.jsonl"):
            base, kind = name[: -len(".vault.jsonl")], "vault"
        elif ZO_ROT_SUFFIX.search(name):
            base, kind = ZO_ROT_SUFFIX.sub("", name), "rot"
        else:
            base, kind = name[: -len(".jsonl")], "live"
        key = f"{path.parent.parent.name}/{base}"
        groups.setdefault(key, {"live": [], "rot": [], "vault": []})[kind].append(path)

    for key, files in sorted(groups.items()):
        by_index: dict[int, dict] = {}
        stamps: dict[int, int] = {}
        compactions: list[int] = []
        clock: list[int] = []
        match = ZO_SESSION_EPOCH_MS.search(key)
        if match:
            clock.append(int(match.group(1)))
        for path in files["live"] + files["rot"] + files["vault"]:
            try:
                clock.append(int(path.stat().st_mtime * 1000))
            except OSError:
                pass
        # Rotated segments first (older), then the live tail. Where both carry
        # the same index the larger copy wins -- a rotation can be written
        # before an in-place clear and then be the ONLY intact copy.
        for path in sorted(files["rot"]) + sorted(files["live"]):
            for record in read_jsonl(path):
                kind = record.get("type")
                if kind == "compaction":
                    index = record.get("first_kept_message_index")
                    if isinstance(index, int):
                        compactions.append(index)
                    continue
                if kind != "message":
                    continue
                index = record.get("turn_index")
                message = record.get("message")
                if not isinstance(index, int) or not isinstance(message, dict):
                    continue
                stamp = record.get("updated_at_ms")
                if isinstance(stamp, int) and stamp > 0:
                    stamps[index] = stamp
                previous = by_index.get(index)
                if previous is None or zo_payload_size(message) > zo_payload_size(previous):
                    by_index[index] = message
        if use_vault:
            # The vault is append-only and may hold a seq twice (see
            # `seal_evicted_to_vault`'s doc comment). Dedup by taking the
            # LARGEST copy rather than the last: a message cleared and then
            # evicted is sealed twice, and only the first seal is raw.
            vault: dict[int, dict] = {}
            for path in sorted(files["vault"]):
                for record in read_jsonl(path):
                    index = record.get("vault_seq")
                    message = record.get("message")
                    if not isinstance(index, int) or not isinstance(message, dict):
                        continue
                    previous = vault.get(index)
                    if previous is None or zo_payload_size(message) > zo_payload_size(previous):
                        vault[index] = message
            for index, message in vault.items():
                previous = by_index.get(index)
                if previous is None or zo_payload_size(message) > zo_payload_size(previous):
                    by_index[index] = message
        if not by_index:
            continue
        ordered = [(index, by_index[index], stamps.get(index)) for index in sorted(by_index)]
        clock.extend(stamps.values())
        span = (min(clock), max(clock)) if clock else (None, None)
        yield key, ordered, sorted(compactions), span


def read_jsonl(path: Path):
    try:
        handle = path.open(encoding="utf-8", errors="replace")
    except OSError:
        return
    with handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


# ---------------------------------------------------------------------------
# Claude Code reader
# ---------------------------------------------------------------------------


def cc_entry_from_rows(rows: list, tool_names: dict) -> Entry:
    """Fold the rows of one logical message into a single Entry."""
    first = rows[0]
    message = first.get("message") or {}
    entry = Entry(role=message.get("role") or first.get("type") or "?")
    for row in rows:
        block_list = (row.get("message") or {}).get("content")
        if isinstance(block_list, str):
            size = utf8(block_list)
            entry.text_bytes += size
            add(entry.by_kind, f"{entry.role}_text", size)
            continue
        for block in block_list or []:
            if not isinstance(block, dict):
                continue
            kind = block.get("type")
            if kind == "text":
                size = utf8(block.get("text"))
                entry.text_bytes += size
                add(entry.by_kind, "assistant_text" if entry.role == "assistant" else f"{entry.role}_text", size)
            elif kind == "thinking":
                size = utf8(block.get("thinking")) + utf8(block.get("signature"))
                entry.text_bytes += size
                add(entry.by_kind, "thinking", size)
            elif kind == "redacted_thinking":
                size = utf8(block.get("data"))
                entry.text_bytes += size
                add(entry.by_kind, "thinking", size)
            elif kind == "tool_use":
                name = block.get("name") or "?"
                tool_names[block.get("id")] = name
                size = utf8(name) + utf8(block.get("input"))
                entry.text_bytes += size
                add(entry.by_kind, "tool_use", size)
            elif kind == "tool_result":
                name = tool_names.get(block.get("tool_use_id")) or "?"
                flat = flatten_result(block.get("content"))
                size = utf8(flat)
                entry.text_bytes += size
                add(entry.by_kind, "tool_result", size)
                append(entry.tool_bytes, name, len(flat))
            elif kind == "image":
                entry.image_bytes += utf8((block.get("source") or {}).get("data"))
    entry.ts_ms = iso_ms(first.get("timestamp"))
    usage = message.get("usage")
    if entry.role == "assistant" and usage:
        entry.usage = usage
        entry.model = message.get("model")
        entry.key = (message.get("id"), first.get("requestId"))
    return entry


def cc_sessions(paths: list[Path]):
    """Yield (session_key, [Entry], compaction_positions).

    One file is one session. Rows that are not `user`/`assistant` messages --
    `attachment`, `ai-title`, `queue-operation`, hook `system` records -- never
    reach the model and are skipped; the `compact_boundary` system record is
    kept only as a position marker.
    """
    for path in paths:
        entries: list[Entry] = []
        compactions: list[int] = []
        tool_names: dict = {}
        pending: list = []
        pending_key = None

        def flush():
            nonlocal pending, pending_key
            if pending:
                entries.append(cc_entry_from_rows(pending, tool_names))
                pending = []
                pending_key = None

        for record in read_jsonl(path):
            kind = record.get("type")
            if kind == "system" and record.get("subtype") == "compact_boundary":
                flush()
                compactions.append(len(entries))
                continue
            if kind not in ("assistant", "user"):
                continue
            message = record.get("message")
            if not isinstance(message, dict):
                continue
            if kind == "assistant":
                key = (message.get("id"), record.get("requestId"))
                if pending and key == pending_key:
                    pending.append(record)
                    continue
                flush()
                pending, pending_key = [record], key
                continue
            flush()
            entries.append(cc_entry_from_rows([record], tool_names))
        flush()
        if entries:
            known = [entry.ts_ms for entry in entries if entry.ts_ms]
            span = (min(known), max(known)) if known else (None, None)
            yield str(path), entries, compactions, span


# ---------------------------------------------------------------------------
# Segmentation: turn a message list into per-request rows
# ---------------------------------------------------------------------------


def segment(
    entries: list[Entry],
    compaction_positions: list[int],
    product: str,
    session: str,
    span: tuple = (None, None),
):
    """Emit one row per request. See docstring note 2 for the boundary rule.

    `period` is how confidently the row can be placed on one side of a cutoff:
    ``exact`` from the request's own stamp, ``session`` when only the session's
    whole span is known and it lies entirely on one side, ``straddle`` when the
    session crosses the cutoff and the request carries no stamp of its own.
    r20 assigned straddling sessions silently to "before"; naming the bucket is
    what stops that from happening again.
    """
    boundaries = [index for index, entry in enumerate(entries) if entry.usage is not None]
    rows = []
    compaction_set = set(compaction_positions)
    for order, boundary in enumerate(boundaries):
        usage = entries[boundary].usage or {}
        write = int(usage.get("cache_creation_input_tokens") or 0)
        read = int(usage.get("cache_read_input_tokens") or 0)
        uncached = int(usage.get("input_tokens") or 0)
        output = int(usage.get("output_tokens") or 0)
        if (write, read, uncached, output) == (0, 0, 0, 0):
            # Claude Code's 0-token stream scaffold (r19 §2). Not a request.
            continue
        first = order == 0
        start = 0 if first else boundaries[order - 1]
        appended = entries[start:boundary]
        text_bytes = sum(entry.text_bytes for entry in appended)
        image_bytes = sum(entry.image_bytes for entry in appended)
        by_kind: dict = {}
        tool_bytes: dict = {}
        for entry in appended:
            for name, value in entry.by_kind.items():
                add(by_kind, name, value)
            for name, values in entry.tool_bytes.items():
                tool_bytes.setdefault(name, []).extend(values)
        rows.append(
            {
                "product": product,
                "session": session,
                "seq": order + 1,
                "first": first,
                "model": entries[boundary].model,
                "family": model_family(entries[boundary].model) if product == "zo" else "anthropic",
                "new_bytes": text_bytes,
                "image_bytes": image_bytes,
                "by_kind": by_kind,
                "tool_bytes": tool_bytes,
                "cleared": any(entry.cleared for entry in appended),
                "compaction": any(position in compaction_set for position in range(start, boundary + 1)),
                "write": write,
                "read": read,
                "uncached": uncached,
                "output": output,
                "total_in": write + read + uncached,
                "key": entries[boundary].key,
                "ts_ms": entries[boundary].ts_ms,
                "session_start_ms": span[0],
                "session_end_ms": span[1],
            }
        )
    # The provider's OWN count of what this request newly carried.
    #
    # `total_in` = write + read + uncached is the whole input of one request,
    # system prompt and tool schemas included. Those are constant between two
    # consecutive requests of one session, so the DIFFERENCE cancels them and
    # leaves exactly the tokens the conversation gained -- measured by the
    # tokenizer that bills them, with no bytes/4 estimator anywhere in it.
    #
    # It is also blind to the thing we are hunting, and that is the point: a
    # marker that moves, or a prefix rewritten in place, changes how the input
    # is SPLIT between write and read without changing the total. So
    # `write / delta_in` separates volume from rewrite on two independent axes.
    #
    # Left as None where it cannot mean that:
    #
    # * the session's first request, and any step where the total did not GROW
    #   -- a compaction, a retry, a rebuilt prefix;
    # * a step where the MODEL CHANGED. Two providers count the same
    #   conversation with two different tokenizers, so their difference is not
    #   a token count of anything. Measured, this is not a rounding concern:
    #   zo routes mid-session (`gpt-5.6-sol` -> `claude-opus-5` inside one
    #   transcript) and 198 such switches produced deltas of 300k-800k
    #   "tokens", 69% of the whole corpus total, purely from re-counting the
    #   SAME prefix on the other side.
    for index, row in enumerate(rows):
        row["delta_in"] = None
        if index == 0 or row["first"]:
            continue
        previous = rows[index - 1]
        if row["model"] != previous["model"]:
            continue
        delta = row["total_in"] - previous["total_in"]
        if delta > 0 and not row["compaction"]:
            row["delta_in"] = delta
    return rows


def period_of(row: dict, since_ms: int) -> str:
    """Which side of `since_ms` this request is on, and how well we know it."""
    if row["ts_ms"]:
        return "after" if row["ts_ms"] >= since_ms else "before"
    start, end = row["session_start_ms"], row["session_end_ms"]
    if end is not None and end < since_ms:
        return "before"
    if start is not None and start >= since_ms:
        return "after"
    return "straddle"


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def quantiles(values: list[float]) -> dict:
    if not values:
        return {"n": 0, "median": 0.0, "p90": 0.0, "mean": 0.0, "p99": 0.0, "max": 0.0, "sum": 0.0}
    ordered = sorted(values)
    count = len(ordered)

    def at(fraction: float) -> float:
        return ordered[min(count - 1, int(fraction * count))]

    return {
        "n": count,
        "median": ordered[count // 2],
        "p90": at(0.90),
        "p99": at(0.99),
        "max": ordered[-1],
        "mean": statistics.fmean(ordered),
        "sum": sum(ordered),
    }


def print_distribution(title: str, groups: list[tuple[str, list[dict]]]) -> None:
    print(f"\n## {title}")
    header = (
        f"{'bucket':<26}{'N':>8}{'median B':>11}{'p90 B':>11}{'mean B':>11}"
        f"{'mean tok':>10}{'write/req':>11}{'write/new':>11}"
    )
    print(header)
    print("-" * len(header))
    for label, rows in groups:
        values = [row["new_bytes"] for row in rows]
        stats = quantiles(values)
        if not stats["n"]:
            print(f"{label:<26}{0:>8}")
            continue
        write_total = sum(row["write"] for row in rows)
        write_per_request = write_total / stats["n"]
        new_tokens_total = tokens(stats["sum"])
        ratio = write_total / new_tokens_total if new_tokens_total else 0.0
        print(
            f"{label:<26}{stats['n']:>8}{stats['median']:>11,.0f}{stats['p90']:>11,.0f}"
            f"{stats['mean']:>11,.0f}{tokens(stats['mean']):>10,.0f}"
            f"{write_per_request:>11,.0f}{ratio:>11,.2f}"
        )


def print_provider_counted(title: str, groups: list[tuple[str, list[dict]]]) -> None:
    """The same question asked without the bytes/4 estimator.

    `delta_in` is the provider's own count of the tokens one request added to
    the conversation (see `segment`). `write/new` here is therefore two
    provider-counted numbers divided by each other -- the estimator appears
    nowhere in it, and `B/tok` is the estimator's error, measured rather than
    assumed: it is what one new-content token actually cost in transcript bytes
    for this bucket's real content mix.
    """
    print(f"\n## {title}")
    header = (
        f"{'bucket':<26}{'N':>8}{'median':>10}{'p90':>10}{'mean':>10}"
        f"{'write/req':>11}{'write/new':>11}{'B/tok':>8}"
    )
    print(header)
    print("-" * len(header))
    for label, rows in groups:
        usable = [row for row in rows if row.get("delta_in")]
        if not usable:
            print(f"{label:<26}{0:>8}")
            continue
        deltas = [row["delta_in"] for row in usable]
        stats = quantiles(deltas)
        write_total = sum(row["write"] for row in usable)
        byte_total = sum(row["new_bytes"] for row in usable)
        print(
            f"{label:<26}{stats['n']:>8}{stats['median']:>10,.0f}{stats['p90']:>10,.0f}"
            f"{stats['mean']:>10,.0f}{write_total / stats['n']:>11,.0f}"
            f"{write_total / stats['sum']:>11,.2f}{byte_total / stats['sum']:>8,.2f}"
        )


def print_hit_miss(title: str, groups: list[tuple[str, list[dict]]]) -> None:
    """Where the cache writes actually come from.

    A request that READ something reused its prefix and wrote only what was
    appended. A request that read NOTHING re-wrote the whole prefix. The two
    populations differ by orders of magnitude, so a mean over both is a mean
    over a mixture and says nothing about either. This table separates them and
    prints how much of the per-request write average each side contributes --
    which is what decides whether the fix is a tool-output cap (volume) or a
    cache-miss cause (rewrite).
    """
    print(f"\n## {title}")
    header = (
        f"{'bucket':<26}{'N':>8}{'miss':>7}{'miss%':>7}{'hit write':>11}{'miss write':>12}"
        f"{'from miss':>11}{'hit w/new':>11}"
    )
    print(header)
    print("-" * len(header))
    for label, rows in groups:
        if not rows:
            print(f"{label:<26}{0:>8}")
            continue
        hits = [row for row in rows if row["read"] > 0]
        misses = [row for row in rows if row["read"] == 0]
        write_total = sum(row["write"] for row in rows)
        miss_write = sum(row["write"] for row in misses)
        hit_write = sum(row["write"] for row in hits)
        hit_delta = sum(row["delta_in"] for row in hits if row.get("delta_in"))
        hit_delta_write = sum(row["write"] for row in hits if row.get("delta_in"))
        print(
            f"{label:<26}{len(rows):>8}{len(misses):>7}{100 * len(misses) / len(rows):>6.1f}%"
            f"{(hit_write / len(hits) if hits else 0):>11,.0f}"
            f"{(miss_write / len(misses) if misses else 0):>12,.0f}"
            f"{(100 * miss_write / write_total if write_total else 0):>10.1f}%"
            f"{(hit_delta_write / hit_delta if hit_delta else 0):>11,.2f}"
        )


def print_kind_table(title: str, groups: list[tuple[str, list[dict]]]) -> None:
    print(f"\n## {title}")
    kinds = ["tool_result", "assistant_text", "thinking", "tool_use", "user_text", "system_text"]
    header = f"{'bucket':<26}" + "".join(f"{kind:>16}" for kind in kinds)
    print(header)
    print("-" * len(header))
    for label, rows in groups:
        if not rows:
            continue
        totals = {kind: 0 for kind in kinds}
        other = 0
        for row in rows:
            for name, value in row["by_kind"].items():
                if name in totals:
                    totals[name] += value
                else:
                    other += value
        grand = sum(totals.values()) + other
        cells = "".join(
            f"{totals[kind] / len(rows):>10,.0f}" + f"{100 * totals[kind] / grand:>5.0f}%" if grand else f"{0:>16}"
            for kind in kinds
        )
        print(f"{label:<26}{cells}")
        if other:
            print(f"{'':<26}(other kinds: {other / len(rows):,.0f} B/req)")


def print_tool_table(title: str, rows: list[dict], limit: int) -> None:
    print(f"\n## {title}")
    per_tool: dict = {}
    for row in rows:
        for name, values in row["tool_bytes"].items():
            per_tool.setdefault(name, []).extend(values)
    header = f"{'tool':<40}{'results':>9}{'median':>10}{'p90':>10}{'p99':>11}{'max':>12}{'share':>8}"
    print(header)
    print("-" * len(header))
    grand = sum(sum(values) for values in per_tool.values())
    for name, values in sorted(per_tool.items(), key=lambda item: -sum(item[1]))[:limit]:
        stats = quantiles(values)
        print(
            f"{name[:40]:<40}{stats['n']:>9}{stats['median']:>10,.0f}{stats['p90']:>10,.0f}"
            f"{stats['p99']:>11,.0f}{stats['max']:>12,.0f}{100 * stats['sum'] / grand:>7.1f}%"
        )
    print("  Unit: CHARACTERS, the unit every cap in the code is written in.")
    print("  N is individual tool results, not requests. `max` is the largest single")
    print("  result this corpus ever carried -- read it against the cap table in the")
    print("  report: far above a tool's documented cap means the cap does not apply")
    print("  on that path.")


def rolling(rows: list[dict], families: tuple[str, ...] | None = None) -> list[dict]:
    return [
        row
        for row in rows
        if not row["first"]
        and not row["compaction"]
        and row["seq"] >= ROLLING_MIN_SEQ
        and (families is None or row["family"] in families)
    ]


# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--zo-root", default=os.path.expanduser("~/.zo/projects"))
    parser.add_argument("--cc-root", default=os.path.expanduser("~/.claude/projects"))
    parser.add_argument(
        "--no-vault",
        action="store_true",
        help="do not restore microcompacted/evicted bodies from *.vault.jsonl (see note 3)",
    )
    parser.add_argument("--rows", help="write every per-request row to this JSONL path")
    parser.add_argument("--tools", type=int, default=22, help="rows in the per-tool table")
    parser.add_argument(
        "--since",
        default=DEFAULT_SINCE,
        help=f"ISO local time splitting before/after (default: {DEFAULT_SINCE}, the anchor fix)",
    )
    args = parser.parse_args()

    since = datetime.fromisoformat(args.since)
    since_ms = int(since.timestamp() * 1000)

    zo_rows: list[dict] = []
    zo_root = Path(args.zo_root)
    if zo_root.is_dir():
        for key, ordered, compactions, span in zo_sessions(zo_root, use_vault=not args.no_vault):
            tool_names: dict = {}
            entries = []
            for _, message, stamp in ordered:
                entry = zo_entry(message, tool_names)
                entry.ts_ms = stamp
                entries.append(entry)
            index_of = {index: position for position, (index, _, _) in enumerate(ordered)}
            positions = [index_of[index] for index in compactions if index in index_of]
            zo_rows.extend(segment(entries, positions, "zo", key, span))

    cc_top = sorted(Path(args.cc_root).glob("*/*.jsonl"))
    cc_nested = sorted(Path(args.cc_root).glob("*/*/*.jsonl")) + sorted(
        Path(args.cc_root).glob("*/*/*/*.jsonl")
    )
    cc_rows: list[dict] = []
    seen_keys: set = set()
    for label, paths in (("cc", cc_top), ("cc-sub", cc_nested)):
        for session, entries, compactions, span in cc_sessions(paths):
            for row in segment(entries, compactions, label, session, span):
                # A resumed or forked Claude Code session copies its parent's
                # rows into a new file. Deduplicate on the provider's own
                # request identity so one model call is counted once (r19 §2).
                if row["key"] in seen_keys:
                    continue
                seen_keys.add(row["key"])
                cc_rows.append(row)

    print(f"# new-content-bytes  ({BYTES_PER_TOKEN} bytes/token estimator)")
    print(f"  zo store : {zo_root}")
    print(f"  cc store : {args.cc_root}   ({len(cc_top)} top-level, {len(cc_nested)} nested files)")
    print(f"  vault restoration: {'OFF (--no-vault)' if args.no_vault else 'ON'}")
    print()
    for label, rows in (("zo", zo_rows), ("cc", [r for r in cc_rows if r['product'] == 'cc']),
                        ("cc-sub", [r for r in cc_rows if r['product'] == 'cc-sub'])):
        firsts = sum(1 for row in rows if row["first"])
        compacted = sum(1 for row in rows if row["compaction"])
        dirty = sum(1 for row in rows if row["cleared"])
        sessions = len({row["session"] for row in rows})
        print(
            f"  {label:<7} requests {len(rows):>7,}  sessions {sessions:>5,}"
            f"  session-first {firsts:>6,}  across-compaction {compacted:>5,}"
            f"  still-cleared {dirty:>6,}"
        )

    # Coordinate check before any new number is read (r22 §1: a ruler that
    # cannot reproduce the old figures cannot be trusted with new ones). These
    # two are r20 §3's rows, and they come out of a DIFFERENT corpus than r20
    # used -- r20 read the prompt-cache store's `stats.json`, this reads the
    # transcripts -- so agreement here is two independent ledgers agreeing.
    print("\n## 좌표계 검증 — r20 §3 을 전사 코퍼스에서 다시 낸다")
    for label, rows, expected in (
        ("Claude Code top-level  write/req", [r for r in cc_rows if r["product"] == "cc"], 3_668),
        ("zo Anthropic 전체기간  write/req", [r for r in zo_rows if r["family"] == "anthropic"], 33_881),
    ):
        if not rows:
            continue
        got = sum(row["write"] for row in rows) / len(rows)
        drift = 100 * (got - expected) / expected
        print(f"  {label}: {got:>9,.0f}   (r20: {expected:,} · {drift:+.1f}%)   N={len(rows):,}")
    print("  r20 은 얼어 있지 않은 저장소를 다른 시각에 읽었고 스윕이 옛 세션을 지운다 —")
    print("  차이는 그 표류지 자의 차이가 아니다. 부호와 자릿수를 본다.")

    zo_anthropic = rolling(zo_rows, ("anthropic",))
    zo_openai = rolling(zo_rows, ("openai",))
    zo_google = rolling(zo_rows, ("google",))
    cc_main = rolling([row for row in cc_rows if row["product"] == "cc"])
    cc_sub = rolling([row for row in cc_rows if row["product"] == "cc-sub"])

    groups = [
        ("zo · Anthropic", zo_anthropic),
        ("zo · OpenAI-compat", zo_openai),
        ("zo · Google", zo_google),
        ("Claude Code · main", cc_main),
        ("Claude Code · sub-agent", cc_sub),
    ]
    print_distribution(
        f"요청당 새 내용 바이트 — rolling requests only (seq >= {ROLLING_MIN_SEQ}, "
        "no session-first, no compaction boundary)",
        groups,
    )
    print("\n  write/new = cache_creation tokens per new-content token (bytes/4).")
    print("  ~1 means the request wrote what it appended (VOLUME); >>1 means it")
    print("  rewrote prefix it had already paid for (REWRITE). A value far BELOW 1")
    print("  means the bytes never reached the wire at that size -- for zo that is")
    print("  the `wire_tool_output` compression seam the transcript does not record.")

    print_provider_counted(
        "요청당 새 내용 토큰 — 프로바이더가 직접 센 값 (delta of total input; no bytes/4)",
        groups,
    )

    print_hit_miss("쓰기의 분해 — 캐시 적중 요청 대 실패 요청", groups)
    print(
        "\n  hit w/new = cache_creation per newly-appended token, on requests that\n"
        "  reused their prefix. 1.00 means the provider wrote exactly what the\n"
        "  conversation gained and nothing more."
    )

    print_kind_table("새 내용의 구성 — mean bytes/request and share", groups)

    # The decisive slice. r20's 8,334-vs-3,668 is measured AFTER the anchor fix;
    # the whole-period 33,881 is dominated by the broken window before it. If
    # the write gap after the fix is volume, `write/new` must converge on both
    # sides of the comparison while `write/req` stays apart.
    period_groups = [
        (f"zo · Anthropic · {name}", [row for row in zo_anthropic if period_of(row, since_ms) == name])
        for name in ("before", "after", "straddle")
    ] + [
        (f"Claude Code · main · {name}", [row for row in cc_main if period_of(row, since_ms) == name])
        for name in ("before", "after", "straddle")
    ]
    print_distribution(f"앵커 수정 기준 분할 (since {args.since}) — rolling requests", period_groups)
    print_provider_counted(
        f"앵커 수정 기준 분할 — 프로바이더가 센 새 내용 토큰 (since {args.since})", period_groups
    )
    print_hit_miss(f"앵커 수정 기준 분할 — 적중/실패 분해 (since {args.since})", period_groups)

    print_distribution(
        "대조: 세션 첫 요청 (cold prefix, never comparable to the rolling rows)",
        [
            ("zo · Anthropic", [r for r in zo_rows if r["first"] and r["family"] == "anthropic"]),
            ("Claude Code · main", [r for r in cc_rows if r["first"] and r["product"] == "cc"]),
        ],
    )

    print_tool_table("도구별 tool_result 바이트 — zo (rolling requests)", zo_anthropic + zo_openai + zo_google, args.tools)
    print_tool_table("도구별 tool_result 바이트 — Claude Code main (rolling requests)", cc_main, args.tools)

    if args.rows:
        with open(args.rows, "w", encoding="utf-8") as handle:
            for row in zo_rows + cc_rows:
                handle.write(json.dumps({k: v for k, v in row.items() if k != "key"}, ensure_ascii=False) + "\n")
        print(f"\n  wrote {len(zo_rows) + len(cc_rows):,} rows to {args.rows}")

    print("\n## 읽는 법")
    print("  · zo 바이트는 전사(轉寫) 기준이라 와이어 압축 전 값이다 — 상한이다.")
    print("    Claude Code 바이트는 전송된 그대로다. 두 열은 같은 자가 아니다.")
    print("  · write/new 가 1 근처면 볼륨, 훨씬 크면 재작성이다.")
    print("  · 분모는 이 표에서 요청(assistant+usage)이고, stats.json 이 아니다 —")
    print("    캐시 원장과 겹치지만 코퍼스가 달라 숫자가 정확히 같지는 않다.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
