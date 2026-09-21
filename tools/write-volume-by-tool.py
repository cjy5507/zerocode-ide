#!/usr/bin/env python3
"""r31 -- WHICH tool, and WHICH kind, fills zo's 8,057 bytes per request?

r24a (`docs/analysis/write-gap-r24.md` §1) closed the volume-or-rewrite
question: the 2.30x cache-write gap is VOLUME. zo appends 8,057 B of new
content per rolling request against Claude Code's 3,499 B -- a gap of
+4,558 B/request -- and after the anchor fix zo writes only what it appends
(hit `write/new` = 0.98). So the remaining lever is the appended bytes
themselves, and the next question is which tool and which kind of block they
belong to. r27 capped `read_file` bodies and MCP success results; that is one
tool on one path, and this script measures how much of the gap it can reach.

Run:

    python3 tools/write-volume-by-tool.py
    python3 tools/write-volume-by-tool.py --rows /tmp/r31.jsonl
    python3 tools/write-volume-by-tool.py --since-r27 2026-08-30T18:14

Input is the two live stores, never anything in this repo:

    zo:           ~/.zo/projects/*/sessions/session-*.jsonl (+ .rot-*, + .vault)
    Claude Code:  ~/.claude/projects/*/*.jsonl        (top level = main agent)
                  ~/.claude/projects/*/*/*.jsonl      (nested   = sub-agents)

DEFINITIONS. Every one is a decision, so it lives in this file rather than in
prose someone must reverse engineer later (`cache-remeasure-r22.md` §1). The
first five are r24a's, copied verbatim in behaviour so the two rulers share one
coordinate system -- §1 below reproduces r24a's 8,057/3,499 and r19's 3,668
before any new number is printed.

1. A REQUEST is one provider model call: an assistant message carrying `usage`.
   Claude Code writes one transcript row per content BLOCK of a single
   response, repeating `usage` on each; rows sharing `(message.id, requestId)`
   fold into one request (r19 §2).

2. NEW CONTENT of request k is every message the conversation gained between
   request k-1 and k -- indices [boundary(k-1), boundary(k)). A session's FIRST
   request has no predecessor and is reported separately, never mixed in.

3. ROLLING requests are seq >= 3, not session-first, not straddling a
   compaction boundary. This is r24a's denominator.

4. zo's persisted transcript is not what the wire carried:
   `microcompact_session` rewrites old tool-result bodies in place and full
   compaction evicts messages, both sealing the original into a sibling
   `*.vault.jsonl` keyed by `vault_seq`. Restored from the vault by default.

5. BYTES are payload bytes (UTF-8 of the content itself), not serialized
   envelope bytes: the two products spell the same content in different JSON
   (zo `blocks`/`output`, CC `content`/`content`) and comparing envelopes would
   measure the spelling.

And three that are new here, because "which tool" is a question r24a's row
shape could not answer:

6. A tool RESULT is measured twice, in CHARACTERS and in BYTES, and the two
   are not interchangeable. Every cap in the code is denominated in
   `chars().count()` (`TruncationConfig::default_max_chars`,
   `MAX_TOOL_ERROR_CHARS`, `AGENT_RESULT_RELAY_CHARS`), so a policy question is
   asked in characters. The gap this round explains is r24a's byte gap, so the
   contribution arithmetic is in bytes. This corpus is full of Hangul, which
   UTF-8 spends 3 bytes on, and mixing the two units turns a 30,000-char result
   into a 50,000-"char" cap violation.

7. CALLS PER REQUEST and BYTES PER CALL are counted separately, because
   "zo calls the tool more often" and "zo gets a bigger answer back" are
   different defects with different fixes. Their product is bytes/request, and
   a gap in that product is split by the symmetric (Shapley) decomposition

       zo_c*zo_s - cc_c*cc_s = (zo_c - cc_c) * (zo_s + cc_s)/2      <- COUNT
                             + (zo_s - cc_s) * (zo_c + cc_c)/2      <- SIZE

   which is exact, not an approximation, and does not privilege either factor
   by evaluating it at the other product's baseline. Where one product never
   calls the tool at all (`session_recall` has no Claude Code counterpart) the
   split is meaningless and the whole gap is charged to COUNT: the difference
   is that the tool exists, not that its answers are bigger.

8. A tool's CANONICAL FAMILY maps the two products' names onto one axis
   (zo `bash` <-> CC `Bash`, `read_file` <-> `Read`, `grep_search` <-> `Grep`,
   `edit_file` <-> `Edit`/`MultiEdit`, `Agent` <-> `Task`, any `mcp__*` -> mcp).
   The raw per-product tables are printed too, so the mapping can be audited
   rather than trusted.

9. A zo tool result that is a JSON ENVELOPE is opened and charged field by
   field. `edit_file`, `write_file`, `MultiEdit` and `read_file` all answer
   with `{...}`, and the fields inside are not equivalent: `content`,
   `oldString` and `newString` are the text the MODEL JUST SENT in the same
   turn's `tool_use` input, returned to it verbatim, while `structuredPatch`
   is a second copy of the same lines with +/- prefixes. Counting the envelope
   as one opaque blob hides the only defect here that costs nothing to fix.

10. THINKING is split into how OFTEN a request carries any (`presence`) and how
    BIG it is when it does. A mean over requests that mostly have none is a
    mean over a mixture -- Claude Code's median thinking is 0 bytes and zo's is
    1,157, so the two products' 1,153-vs-2,860 means are not the same shape.

11. REMINDERS are counted across the two products' different spellings. zo
    carries them as their own `role: "system"` message; Claude Code embeds
    `<system-reminder>...</system-reminder>` inside a user message's text.
    r24a §6.6 warned its `system_text` column must NOT be read as "zo sends
    more reminders" for exactly this reason. This script settles it by pulling
    the embedded spans out of Claude Code's user text and comparing like with
    like. `by_kind` is left untouched so §2 still reproduces r24a's table; the
    reminder accounting is a separate column.

12. THINKING TEXT IS NOT IN CLAUDE CODE'S TRANSCRIPT. Every `thinking` block
    Claude Code persists has `thinking: ""` and keeps only `signature`;
    zo persists both. So the byte axis compares zo's reasoning text plus
    signature against Claude Code's signature alone, and r24a §1's
    "thinking 2,864 vs 1,153" is not a like-for-like row. This script measures
    the ratio of thinking text to signature on zo's own Anthropic blocks and
    uses it to put a bound on what Claude Code's transcript is not showing --
    then checks that bound against the provider-counted token axis, which
    never had the problem because the tokenizer billed what was actually sent.

13. THE WIRE VIEW, RECOMPUTED. zo does not send its transcript. `convert_messages`
    runs `context_compression::wire_tool_output` over every tool result, and for
    six tools (`read_file`, `bash`, `grep_search`, `glob_search`, `edit_file`,
    `write_file`) that pass rewrites the body. Two of those rewrites drop exactly
    the duplication this script found -- `compress_edit_file` drops `oldString`
    and `newString`, `compress_write_file` drops `content` -- so a proposal to
    "stop echoing the input back" would be a proposal to ship what is already
    shipped. This script therefore reproduces three of those transforms
    (`compress_edit_file`, `compress_write_file`, and `compress_read_file`'s
    lossless unwrap, with `MIN_SAVINGS_CHARS = 24` and its `pick_smaller` rule)
    and reports the corrected bytes beside the transcript ones. `bash`,
    `grep_search`, `glob_search` and read_file's two elided views are NOT
    reproduced, so even the corrected zo column stays an upper bound.

CAVEAT this script cannot remove, stated so the reader discounts it in the
right direction: zo compresses tool results again on the way to the wire
(`context_compression::wire_tool_output`, called from `convert_messages`) and
the transcript keeps the PRE-compression body. Claude Code truncates at the
source and its transcript IS the wire. So every zo byte column here is an UPPER
bound and every Claude Code one is exact. A saving estimated on zo's transcript
bytes is therefore also an upper bound.
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
# Definitions (see docstring)
# ---------------------------------------------------------------------------

BYTES_PER_TOKEN = 4
ROLLING_MIN_SEQ = 3

ZO_CLEARED_MARKERS = (
    "[Old tool result content cleared]",
    "[Superseded reminder cleared]",
    "[Old pasted image cleared to save context",
)

ZO_ROT_SUFFIX = re.compile(r"\.rot-\d+\.jsonl$")
ZO_SESSION_EPOCH_MS = re.compile(r"session-(\d{13})-\d+$")

# The anchor fix (r20 §1) and the r27 output-cap deploy (`e2371aab`, landed
# 17:58, shipped to /Applications 18:14). Goal 3 asks for the slice after the
# second one.
DEFAULT_SINCE = "2026-08-27T00:00"
DEFAULT_SINCE_R27 = "2026-08-30T18:14"

# The recovery notice `truncate_tool_output` appends when it cuts. Measured in
# the corpus as the 187 characters that make a 30,000-char cap show up as
# 30,187 (r24a §3). A cap saves (size - cap - NOTICE_CHARS) characters, not
# (size - cap).
NOTICE_CHARS = 187

# docstring note 8. Names on the left are zo's, on the right Claude Code's.
TOOL_FAMILY = {
    "bash": "shell",
    "Bash": "shell",
    "BashOutput": "shell",
    "KillShell": "shell",
    "KillBash": "shell",
    "read_file": "read",
    "Read": "read",
    "NotebookRead": "read",
    "grep_search": "grep",
    "Grep": "grep",
    "glob_search": "glob",
    "Glob": "glob",
    "edit_file": "edit",
    "Edit": "edit",
    "MultiEdit": "edit",
    "NotebookEdit": "edit",
    "write_file": "write",
    "Write": "write",
    "Agent": "subagent",
    "Task": "subagent",
    "session_recall": "recall",
    "todo_write": "todo",
    "TodoWrite": "todo",
    "web_fetch": "web",
    "WebFetch": "web",
    "web_search": "web",
    "WebSearch": "web",
    "MCPTool": "mcp",
    "SpawnMultiAgent": "subagent",
    "codegraph": "codegraph",
    "Skill": "skill",
    "ToolSearch": "toolsearch",
}

# docstring note 9. Fields a zo JSON-envelope result spends its bytes on.
# `echo` are fields whose content the model itself sent in the same turn's
# tool_use input; `derived` is generated from them and duplicates them again.
ENVELOPE_TOOLS = ("edit_file", "write_file", "MultiEdit", "read_file")
ENVELOPE_ECHO = ("content", "oldString", "newString", "edits")
ENVELOPE_DERIVED = ("structuredPatch", "gitDiff", "patch")

# docstring note 13: `context_compression::MIN_SAVINGS_CHARS`. A rewrite is
# taken only when it saves at least this many CHARACTERS.
MIN_SAVINGS_CHARS = 24
# `compress_read_file` switches to an elided view above this; those views are
# not reproduced here, so results past it are counted at their lossless size.
OUTLINE_THRESHOLD_CHARS = 30_000

# docstring note 11.
REMINDER_SPAN = re.compile(r"<system-reminder>(.*?)</system-reminder>", re.DOTALL)
REMINDER_TAG = re.compile(r"\[(zo:[a-z0-9-]+|system:[^\]]{0,28})\]")

FAMILY_ORDER = [
    "shell",
    "read",
    "edit",
    "grep",
    "glob",
    "write",
    "mcp",
    "subagent",
    "recall",
    "codegraph",
    "web",
    "todo",
    "skill",
    "toolsearch",
    "other",
]


def tool_family(name: str) -> str:
    if not name:
        return "other"
    if name.startswith("mcp__"):
        return "mcp"
    return TOOL_FAMILY.get(name, "other")


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
    role: str
    text_bytes: int = 0
    image_bytes: int = 0
    by_kind: dict = field(default_factory=dict)
    # tool name -> [[chars, bytes], ...] for each INDIVIDUAL result. A list,
    # not a sum: docstring notes 6 and 7 both need the per-call distribution.
    tool_results: dict = field(default_factory=dict)
    # tool name -> [bytes, ...] of each tool_use block (name + input JSON).
    tool_uses: dict = field(default_factory=dict)
    # "<tool>/<field>" -> bytes, for JSON-envelope results (docstring note 9).
    envelope: dict = field(default_factory=dict)
    # reminder tag -> bytes, however the product spells reminders (note 11).
    reminders: dict = field(default_factory=dict)
    thinking_blocks: int = 0
    thinking_bytes: int = 0
    signature_bytes: int = 0
    # (thinking text bytes, signature bytes) of each individual block, for the
    # ratio fit in docstring note 12.
    thinking_pairs: list = field(default_factory=list)
    cleared: bool = False
    usage: dict | None = None
    model: str | None = None
    key: object = None
    ts_ms: int | None = None


def add(mapping: dict, name: str, value: int) -> None:
    if value:
        mapping[name] = mapping.get(name, 0) + value


def append(mapping: dict, name: str, value) -> None:
    mapping.setdefault(name, []).append(value)


def utf8(value) -> int:
    if value is None:
        return 0
    if not isinstance(value, str):
        value = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return len(value.encode("utf-8", errors="replace"))


def reminder_tag(text: str) -> str:
    """A short stable name for one reminder, for ranking them by volume."""
    body = text.replace("<system-reminder>", " ").strip()
    match = REMINDER_TAG.search(body[:400])
    if match:
        return match.group(1)[:30]
    for line in body.splitlines():
        line = line.strip().lstrip("#").strip()
        if line:
            return line[:30]
    return "(unnamed)"


def account_reminders(text: str, entry: Entry, whole: bool = False) -> None:
    """Charge a text block's reminder bytes to their tag (docstring note 11).

    `whole=True` for zo, whose reminder IS the message; otherwise only the
    `<system-reminder>` spans inside the text are taken, which is how Claude
    Code spells the same thing.
    """
    if whole:
        add(entry.reminders, reminder_tag(text), utf8(text))
        return
    for match in REMINDER_SPAN.finditer(text):
        add(entry.reminders, reminder_tag(match.group(1)), utf8(match.group(0)))


def account_envelope(tool: str, output: str, entry: Entry) -> None:
    """Open a JSON-envelope result and charge its bytes field by field.

    Silently does nothing when the result is not an object -- an error string,
    a truncated body, a tool that answers in plain text. Those bytes stay
    counted in `tool_result`; they are just not attributable to a field.
    """
    if tool not in ENVELOPE_TOOLS:
        return
    head = output.lstrip()[:1]
    if head != "{":
        return
    try:
        parsed = json.loads(output)
    except (json.JSONDecodeError, ValueError):
        return
    if not isinstance(parsed, dict):
        return
    for name, value in parsed.items():
        if isinstance(value, dict):
            for inner, nested in value.items():
                add(entry.envelope, f"{tool}/{name}.{inner}", utf8(nested))
            continue
        add(entry.envelope, f"{tool}/{name}", utf8(value))


def render_patch_hunks(hunks) -> str:
    """`context_compression::render_patch_hunks`."""
    out = []
    for hunk in hunks or []:
        if not isinstance(hunk, dict):
            continue
        out.append(
            f"@@ -{hunk.get('oldStart', 0)},{hunk.get('oldLines', 0)}"
            f" +{hunk.get('newStart', 0)},{hunk.get('newLines', 0)} @@\n"
        )
        for line in hunk.get("lines") or []:
            if isinstance(line, str):
                out.append(line + "\n")
    return "".join(out)


def append_tool_feedback(view: str, obj: dict) -> str:
    feedback = obj.get("toolFeedback")
    if not isinstance(feedback, str) or not feedback.strip():
        return view
    if not view.endswith("\n"):
        view += "\n"
    return view + feedback.rstrip() + "\n"


def pick_smaller(raw: str, candidate: str) -> str:
    """`CompressionOutcome::pick_smaller` -- characters, not bytes."""
    if len(candidate) + MIN_SAVINGS_CHARS <= len(raw):
        return candidate
    return raw


def wire_view(tool: str, output: str, hypothetical: bool = False) -> str:
    """Reproduce zo's wire rewrite for the three envelope tools (note 13).

    Returns `output` unchanged for every tool this does not reproduce, which
    is why the corrected figure remains an upper bound rather than the wire.

    `MultiEdit` is deliberately NOT in the compressed set: `compress_tool_output_with`
    dispatches on `canonical.as_str()` after `to_ascii_lowercase()`, and its arms
    are `"edit" | "edit_file"`. `tools::aliases::canonical_tool_name` maps
    `multi_edit` to `MultiEdit`, which lowercases to `multiedit` and matches
    neither -- so MultiEdit results reach the wire uncompressed. `hypothetical=True`
    applies the edit_file transform to it anyway, to size that hole.
    """
    compressed = ("edit_file", "write_file", "read_file")
    if hypothetical:
        compressed = compressed + ("MultiEdit",)
    if tool not in compressed:
        return output
    if output.lstrip()[:1] != "{":
        return output
    try:
        obj = json.loads(output)
    except (json.JSONDecodeError, ValueError):
        return output
    if not isinstance(obj, dict):
        return output
    if tool in ("edit_file", "MultiEdit"):
        path = obj.get("filePath")
        hunks = obj.get("structuredPatch")
        if not isinstance(path, str) or not isinstance(hunks, list):
            return output
        view = f"[edit] {path} · applied"
        if obj.get("replaceAll") is True:
            view += " · replace_all"
        if obj.get("userModified") is True:
            view += " · user_modified"
        view += "\n" + render_patch_hunks(hunks)
        return pick_smaller(output, append_tool_feedback(view, obj))
    if tool == "write_file":
        path = obj.get("filePath")
        if not isinstance(path, str):
            return output
        view = f"[write] {path} · written"
        content = obj.get("content")
        if isinstance(content, str):
            view += f" · {len(content.splitlines())} lines"
        view += "\n"
        patch = obj.get("structuredPatch")
        if isinstance(patch, list):
            view += render_patch_hunks(patch)
        return pick_smaller(output, append_tool_feedback(view, obj))
    payload = obj.get("file")
    if not isinstance(payload, dict) or not isinstance(payload.get("content"), str):
        return output
    start = payload.get("startLine", 0)
    end = start + max(0, payload.get("numLines", 0) - 1)
    view = f"[file] {payload.get('filePath')} · lines {start}-{end} of {payload.get('totalLines')}\n"
    notice = payload.get("notice")
    if isinstance(notice, str):
        view += f"[note] {notice}\n"
    view += payload["content"]
    return pick_smaller(output, view)


def flatten_result(content) -> str:
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
            if entry.role == "system":
                account_reminders(text, entry, whole=True)
            elif "<system-reminder>" in text:
                account_reminders(text, entry)
            if any(marker in text for marker in ZO_CLEARED_MARKERS):
                entry.cleared = True
        elif kind == "thinking":
            signature = utf8(block.get("signature"))
            size = utf8(block.get("thinking")) + signature
            entry.text_bytes += size
            add(entry.by_kind, "thinking", size)
            entry.thinking_blocks += 1
            entry.thinking_bytes += size
            entry.signature_bytes += signature
            entry.thinking_pairs.append((size - signature, signature))
        elif kind == "tool_use":
            name = block.get("name") or "?"
            tool_names[block.get("id")] = name
            size = utf8(name) + utf8(block.get("input"))
            entry.text_bytes += size
            add(entry.by_kind, "tool_use", size)
            append(entry.tool_uses, name, size)
        elif kind == "tool_result":
            name = block.get("tool_name") or tool_names.get(block.get("tool_use_id")) or "?"
            output = block.get("output") or ""
            size = utf8(output)
            entry.text_bytes += size
            add(entry.by_kind, "tool_result", size)
            wire = wire_view(name, output)
            hypo = wire_view(name, output, hypothetical=True) if name == "MultiEdit" else wire
            append(entry.tool_results, name, [len(output), size, utf8(wire), utf8(hypo)])
            account_envelope(name, output, entry)
            if "<system-reminder>" in output:
                account_reminders(output, entry)
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
    if not text:
        return None
    try:
        return int(datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp() * 1000)
    except (ValueError, TypeError):
        return None


def zo_payload_size(message) -> int:
    if not isinstance(message, dict):
        return 0
    return len(json.dumps(message, ensure_ascii=False).encode("utf-8", errors="replace"))


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


def zo_sessions(root: Path, use_vault: bool):
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


# ---------------------------------------------------------------------------
# Claude Code reader
# ---------------------------------------------------------------------------


def cc_entry_from_rows(rows: list, tool_names: dict) -> Entry:
    first = rows[0]
    message = first.get("message") or {}
    entry = Entry(role=message.get("role") or first.get("type") or "?")
    for row in rows:
        block_list = (row.get("message") or {}).get("content")
        if isinstance(block_list, str):
            size = utf8(block_list)
            entry.text_bytes += size
            add(entry.by_kind, f"{entry.role}_text", size)
            if "<system-reminder>" in block_list:
                account_reminders(block_list, entry)
            continue
        for block in block_list or []:
            if not isinstance(block, dict):
                continue
            kind = block.get("type")
            if kind == "text":
                text = block.get("text") or ""
                size = utf8(text)
                entry.text_bytes += size
                add(entry.by_kind, "assistant_text" if entry.role == "assistant" else f"{entry.role}_text", size)
                if "<system-reminder>" in text:
                    account_reminders(text, entry)
            elif kind == "thinking":
                signature = utf8(block.get("signature"))
                size = utf8(block.get("thinking")) + signature
                entry.text_bytes += size
                add(entry.by_kind, "thinking", size)
                entry.thinking_blocks += 1
                entry.thinking_bytes += size
                entry.signature_bytes += signature
                entry.thinking_pairs.append((size - signature, signature))
            elif kind == "redacted_thinking":
                size = utf8(block.get("data"))
                entry.text_bytes += size
                add(entry.by_kind, "thinking", size)
                entry.thinking_blocks += 1
                entry.thinking_bytes += size
            elif kind == "tool_use":
                name = block.get("name") or "?"
                tool_names[block.get("id")] = name
                size = utf8(name) + utf8(block.get("input"))
                entry.text_bytes += size
                add(entry.by_kind, "tool_use", size)
                append(entry.tool_uses, name, size)
            elif kind == "tool_result":
                name = tool_names.get(block.get("tool_use_id")) or "?"
                flat = flatten_result(block.get("content"))
                size = utf8(flat)
                entry.text_bytes += size
                add(entry.by_kind, "tool_result", size)
                append(entry.tool_results, name, [len(flat), size, size, size])
                if "<system-reminder>" in flat:
                    account_reminders(flat, entry)
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
# Segmentation
# ---------------------------------------------------------------------------


def segment(entries, compaction_positions, product, session, span=(None, None)):
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
            continue
        first = order == 0
        start = 0 if first else boundaries[order - 1]
        appended = entries[start:boundary]
        by_kind: dict = {}
        tool_results: dict = {}
        tool_uses: dict = {}
        envelope: dict = {}
        reminders: dict = {}
        thinking_blocks = 0
        thinking_bytes = 0
        signature_bytes = 0
        thinking_pairs: list = []
        for entry in appended:
            for name, value in entry.by_kind.items():
                add(by_kind, name, value)
            for name, values in entry.tool_results.items():
                tool_results.setdefault(name, []).extend(values)
            for name, values in entry.tool_uses.items():
                tool_uses.setdefault(name, []).extend(values)
            for name, value in entry.envelope.items():
                add(envelope, name, value)
            for name, value in entry.reminders.items():
                add(reminders, name, value)
            thinking_blocks += entry.thinking_blocks
            thinking_bytes += entry.thinking_bytes
            signature_bytes += entry.signature_bytes
            thinking_pairs.extend(entry.thinking_pairs)
        rows.append(
            {
                "product": product,
                "session": session,
                "seq": order + 1,
                "first": first,
                "model": entries[boundary].model,
                "family": model_family(entries[boundary].model) if product == "zo" else "anthropic",
                "new_bytes": sum(entry.text_bytes for entry in appended),
                "image_bytes": sum(entry.image_bytes for entry in appended),
                "by_kind": by_kind,
                "tool_results": tool_results,
                "tool_uses": tool_uses,
                "envelope": envelope,
                "reminders": reminders,
                "thinking_blocks": thinking_blocks,
                "thinking_bytes": thinking_bytes,
                "signature_bytes": signature_bytes,
                "thinking_pairs": thinking_pairs,
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
    if row["ts_ms"]:
        return "after" if row["ts_ms"] >= since_ms else "before"
    start, end = row["session_start_ms"], row["session_end_ms"]
    if end is not None and end < since_ms:
        return "before"
    if start is not None and start >= since_ms:
        return "after"
    return "straddle"


def rolling(rows, families=None):
    return [
        row
        for row in rows
        if not row["first"]
        and not row["compaction"]
        and row["seq"] >= ROLLING_MIN_SEQ
        and (families is None or row["family"] in families)
    ]


def quantiles(values):
    if not values:
        return {"n": 0, "median": 0.0, "p90": 0.0, "mean": 0.0, "p99": 0.0, "max": 0.0, "sum": 0.0}
    ordered = sorted(values)
    count = len(ordered)

    def at(fraction):
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


# ---------------------------------------------------------------------------
# Goal 1 -- new content per request by kind and by tool
# ---------------------------------------------------------------------------

KINDS = ["tool_result", "thinking", "tool_use", "system_text", "assistant_text", "user_text"]


def kind_profile(rows):
    """mean bytes/request per kind, plus the per-request distribution."""
    count = len(rows) or 1
    totals = {kind: 0 for kind in KINDS}
    other = 0
    per_request = {kind: [] for kind in KINDS}
    for row in rows:
        for kind in KINDS:
            value = row["by_kind"].get(kind, 0)
            totals[kind] += value
            per_request[kind].append(value)
        for name, value in row["by_kind"].items():
            if name not in totals:
                other += value
    grand = sum(totals.values()) + other
    return {
        "n": len(rows),
        "mean": {kind: totals[kind] / count for kind in KINDS},
        "share": {kind: (100 * totals[kind] / grand if grand else 0) for kind in KINDS},
        "dist": {kind: quantiles(per_request[kind]) for kind in KINDS},
        "other_mean": other / count,
        "grand_mean": grand / count,
    }


def print_kind_table(title, groups):
    print(f"\n## {title}")
    header = f"{'bucket':<24}{'N':>8}" + "".join(f"{kind[:13]:>17}" for kind in KINDS)
    print(header)
    print("-" * len(header))
    for label, rows in groups:
        if not rows:
            print(f"{label:<24}{0:>8}")
            continue
        profile = kind_profile(rows)
        cells = "".join(
            f"{profile['mean'][kind]:>11,.0f}{profile['share'][kind]:>5.0f}%" for kind in KINDS
        )
        print(f"{label:<24}{profile['n']:>8}{cells}")
        if profile["other_mean"]:
            print(f"{'':<24}{'':>8}(other kinds: {profile['other_mean']:,.0f} B/req)")


def print_kind_dist(title, groups):
    print(f"\n## {title}")
    header = f"{'bucket':<24}{'kind':<16}{'median':>10}{'p90':>10}{'mean':>10}{'p99':>11}"
    print(header)
    print("-" * len(header))
    for label, rows in groups:
        if not rows:
            continue
        profile = kind_profile(rows)
        for kind in KINDS:
            stats = profile["dist"][kind]
            print(
                f"{label:<24}{kind:<16}{stats['median']:>10,.0f}{stats['p90']:>10,.0f}"
                f"{stats['mean']:>10,.0f}{stats['p99']:>11,.0f}"
            )


def tool_profile(rows, by_family=True):
    """name -> calls, result bytes/chars, tool_use bytes; all per request."""
    count = len(rows) or 1
    acc: dict = {}

    def slot(name):
        return acc.setdefault(
            name,
            {"calls": 0, "res_bytes": 0, "res_chars": 0, "wire_bytes": 0, "use_bytes": 0,
             "uses": 0, "per_call_chars": [], "per_call_bytes": []},
        )

    for row in rows:
        for name, values in row["tool_results"].items():
            key = tool_family(name) if by_family else name
            cell = slot(key)
            for chars, size, wire, _hypo in values:
                cell["calls"] += 1
                cell["res_chars"] += chars
                cell["res_bytes"] += size
                cell["wire_bytes"] += wire
                cell["per_call_chars"].append(chars)
                cell["per_call_bytes"].append(size)
        for name, values in row["tool_uses"].items():
            key = tool_family(name) if by_family else name
            cell = slot(key)
            cell["uses"] += len(values)
            cell["use_bytes"] += sum(values)
    for cell in acc.values():
        cell["calls_per_req"] = cell["calls"] / count
        cell["bytes_per_call"] = cell["res_bytes"] / cell["calls"] if cell["calls"] else 0.0
        cell["bytes_per_req"] = cell["res_bytes"] / count
        cell["wire_per_req"] = cell["wire_bytes"] / count
        cell["wire_per_call"] = cell["wire_bytes"] / cell["calls"] if cell["calls"] else 0.0
        cell["use_bytes_per_req"] = cell["use_bytes"] / count
        cell["chars_per_call"] = cell["res_chars"] / cell["calls"] if cell["calls"] else 0.0
        cell["q"] = quantiles(cell["per_call_chars"])
    return acc


def print_tool_side_by_side(title, zo_acc, cc_acc, zo_n, cc_n):
    print(f"\n## {title}")
    header = (
        f"{'family':<12}"
        f"{'zo calls/req':>13}{'zo B/call':>11}{'zo B/req':>10}{'zo wire':>9}"
        f"{'CC calls/req':>14}{'CC B/call':>11}{'CC B/req':>10}"
        f"{'gap B/req':>11}{'wire gap':>10}"
    )
    print(header)
    print("-" * len(header))
    names = [f for f in FAMILY_ORDER if f in zo_acc or f in cc_acc]
    names += sorted(set(zo_acc) | set(cc_acc) - set(names) - set(FAMILY_ORDER))
    seen = set()
    for name in names:
        if name in seen:
            continue
        seen.add(name)
        z = zo_acc.get(name)
        c = cc_acc.get(name)
        zc = z["calls_per_req"] if z else 0.0
        zs = z["bytes_per_call"] if z else 0.0
        zb = z["bytes_per_req"] if z else 0.0
        cc_c = c["calls_per_req"] if c else 0.0
        cs = c["bytes_per_call"] if c else 0.0
        cb = c["bytes_per_req"] if c else 0.0
        zw = z["wire_per_req"] if z else 0.0
        print(
            f"{name:<12}{zc:>13.3f}{zs:>11,.0f}{zb:>10,.0f}{zw:>9,.0f}"
            f"{cc_c:>14.3f}{cs:>11,.0f}{cb:>10,.0f}{zb - cb:>11,.0f}{zw - cb:>10,.0f}"
        )
    zt = sum(cell["bytes_per_req"] for cell in zo_acc.values())
    zwt = sum(cell["wire_per_req"] for cell in zo_acc.values())
    ct = sum(cell["bytes_per_req"] for cell in cc_acc.values())
    print("-" * len(header))
    print(
        f"{'TOTAL':<12}{sum(c['calls_per_req'] for c in zo_acc.values()):>13.3f}{'':>11}{zt:>10,.0f}{zwt:>9,.0f}"
        f"{sum(c['calls_per_req'] for c in cc_acc.values()):>14.3f}{'':>11}{ct:>10,.0f}"
        f"{zt - ct:>11,.0f}{zwt - ct:>10,.0f}"
    )
    print(f"  denominators: zo N={zo_n:,} rolling requests, Claude Code N={cc_n:,}")
    print("  `zo wire` re-runs zo's own `wire_tool_output` for edit_file/write_file/")
    print("  read_file (docstring note 13). bash·grep·glob are NOT reproduced, so their")
    print("  wire column equals their transcript column and the total stays an upper bound.")


def print_tool_raw(title, acc, limit):
    print(f"\n## {title}")
    header = f"{'tool':<44}{'results':>9}{'calls/req':>10}{'median c':>10}{'p90 c':>9}{'p99 c':>10}{'max c':>11}{'B/req':>9}"
    print(header)
    print("-" * len(header))
    for name, cell in sorted(acc.items(), key=lambda item: -item[1]["res_bytes"])[:limit]:
        stats = cell["q"]
        print(
            f"{name[:44]:<44}{cell['calls']:>9,}{cell['calls_per_req']:>10.3f}"
            f"{stats['median']:>10,.0f}{stats['p90']:>9,.0f}{stats['p99']:>10,.0f}"
            f"{stats['max']:>11,.0f}{cell['bytes_per_req']:>9,.0f}"
        )
    print("  `c` columns are CHARACTERS (the unit every cap is written in); B/req is bytes.")


# ---------------------------------------------------------------------------
# Goal 2 -- what explains the gap
# ---------------------------------------------------------------------------


def shapley(zo_calls, zo_size, cc_calls, cc_size):
    """Exact split of zo_c*zo_s - cc_c*cc_s into a COUNT and a SIZE term.

    Where one side never calls the tool the split is not meaningful (there is
    no 'size' to compare), so the whole gap is charged to COUNT -- see
    docstring note 7."""
    gap = zo_calls * zo_size - cc_calls * cc_size
    if zo_calls == 0 or cc_calls == 0:
        return gap, 0.0
    count_term = (zo_calls - cc_calls) * (zo_size + cc_size) / 2
    size_term = (zo_size - cc_size) * (zo_calls + cc_calls) / 2
    return count_term, size_term


def print_gap_ranking(title, zo_rows, cc_rows):
    print(f"\n## {title}")
    zo_profile = kind_profile(zo_rows)
    cc_profile = kind_profile(cc_rows)
    total_gap = zo_profile["grand_mean"] - cc_profile["grand_mean"]
    print(f"  zo {zo_profile['grand_mean']:,.0f} B/req  -  CC {cc_profile['grand_mean']:,.0f} B/req"
          f"  =  gap {total_gap:,.0f} B/req  ({zo_profile['grand_mean'] / cc_profile['grand_mean']:.2f}x)")

    print("\n  (a) by KIND")
    header = f"    {'kind':<16}{'zo B/req':>10}{'CC B/req':>10}{'gap':>10}{'of gap':>9}{'cum':>8}"
    print(header)
    print("    " + "-" * (len(header) - 4))
    contributions = []
    for kind in KINDS:
        gap = zo_profile["mean"][kind] - cc_profile["mean"][kind]
        contributions.append((kind, zo_profile["mean"][kind], cc_profile["mean"][kind], gap))
    other_gap = zo_profile["other_mean"] - cc_profile["other_mean"]
    if abs(other_gap) > 1:
        contributions.append(("(other)", zo_profile["other_mean"], cc_profile["other_mean"], other_gap))
    contributions.sort(key=lambda item: -item[3])
    cumulative = 0.0
    for kind, zo_value, cc_value, gap in contributions:
        cumulative += gap
        print(
            f"    {kind:<16}{zo_value:>10,.0f}{cc_value:>10,.0f}{gap:>10,.0f}"
            f"{100 * gap / total_gap:>8.1f}%{100 * cumulative / total_gap:>7.1f}%"
        )

    print("\n  (b) tool_result by TOOL FAMILY, split into COUNT (zo calls more)")
    print("      and SIZE (zo gets more back per call)")
    zo_acc = tool_profile(zo_rows)
    cc_acc = tool_profile(cc_rows)
    header = (
        f"    {'family':<12}{'gap B/req':>11}{'count':>10}{'size':>10}"
        f"{'of total gap':>14}{'cum':>8}{'driver':>9}"
    )
    print(header)
    print("    " + "-" * (len(header) - 4))
    tool_rows = []
    for name in set(zo_acc) | set(cc_acc):
        z = zo_acc.get(name)
        c = cc_acc.get(name)
        count_term, size_term = shapley(
            z["calls_per_req"] if z else 0.0,
            z["bytes_per_call"] if z else 0.0,
            c["calls_per_req"] if c else 0.0,
            c["bytes_per_call"] if c else 0.0,
        )
        tool_rows.append((name, count_term + size_term, count_term, size_term))
    tool_rows.sort(key=lambda item: -item[1])
    wire_gap = sum(cell["wire_per_req"] for cell in zo_acc.values()) - sum(
        cell["bytes_per_req"] for cell in cc_acc.values()
    )
    cumulative = 0.0
    for name, gap, count_term, size_term in tool_rows:
        cumulative += gap
        driver = "count" if abs(count_term) > abs(size_term) else "size"
        print(
            f"    {name:<12}{gap:>11,.0f}{count_term:>10,.0f}{size_term:>10,.0f}"
            f"{100 * gap / total_gap:>13.1f}%{100 * cumulative / total_gap:>7.1f}%{driver:>9}"
        )
    print(f"    tool_result 격차를 zo 의 와이어 뷰로 다시 재면 "
          f"{sum(cell['wire_per_req'] for cell in zo_acc.values()):,.0f} - "
          f"{sum(cell['bytes_per_req'] for cell in cc_acc.values()):,.0f} = {wire_gap:,.0f} B/req")
    return total_gap, contributions, tool_rows, wire_gap


# ---------------------------------------------------------------------------
# The three axes the ranking points at
# ---------------------------------------------------------------------------


def envelope_profile(rows):
    """"<tool>/<field>" -> bytes per request (docstring note 9)."""
    count = len(rows) or 1
    totals: dict = {}
    for row in rows:
        for name, value in row.get("envelope", {}).items():
            add(totals, name, value)
    return {name: value / count for name, value in totals.items()}


def print_envelope_table(title, rows, limit=18):
    print(f"\n## {title}")
    profile = envelope_profile(rows)
    header = f"{'tool/field':<34}{'B/req':>10}{'class':>10}"
    print(header)
    print("-" * len(header))
    echo = derived = other = 0.0
    for name, value in sorted(profile.items(), key=lambda item: -item[1])[:limit]:
        field_name = name.split("/", 1)[1].split(".")[-1]
        if field_name in ENVELOPE_ECHO and not name.startswith("read_file/"):
            kind = "echo"
        elif field_name in ENVELOPE_DERIVED:
            kind = "derived"
        elif field_name in ENVELOPE_ECHO:
            kind = "body"
        else:
            kind = "receipt"
        print(f"{name[:34]:<34}{value:>10,.0f}{kind:>10}")
    for name, value in profile.items():
        field_name = name.split("/", 1)[1].split(".")[-1]
        if field_name in ENVELOPE_ECHO and not name.startswith("read_file/"):
            echo += value
        elif field_name in ENVELOPE_DERIVED:
            derived += value
        else:
            other += value
    print("-" * len(header))
    print(f"{'ECHO (model sent it this turn)':<34}{echo:>10,.0f}")
    print(f"{'DERIVED (2nd copy of the same lines)':<34}{derived:>10,.0f}")
    print(f"{'receipt / metadata':<34}{other:>10,.0f}")
    print("  echo    = `content`/`oldString`/`newString`/`edits`: the text the model")
    print("            itself put in this turn's tool_use input, handed back verbatim.")
    print("  derived = `structuredPatch`/`gitDiff`: the same lines again, +/- prefixed.")
    print("  `read_file/file.content` is NOT echo -- the model did not send it. It is the")
    print("  read itself, and it is listed under receipt for that reason.")
    print("  이 표는 전사(轉寫)다. zo 의 와이어는 이미 echo 를 버린다 —")
    print("  `compress_edit_file` 가 oldString/newString 을, `compress_write_file` 가")
    print("  content 를 떨어뜨린다. 아래 §7b 가 그 변환을 다시 돌려 실제 크기를 낸다.")
    return echo, derived, other


def print_wire_recompute(title, zo_rows, cc_rows):
    """docstring note 13: what the wire actually carried, for the three tools
    whose transform this script reproduces."""
    print(f"\n## {title}")
    count = len(zo_rows) or 1
    header = f"{'tool':<16}{'results':>9}{'전사 B/req':>13}{'와이어 B/req':>14}{'절감':>10}{'절감률':>9}"
    print(header)
    print("-" * len(header))
    reproduced = ("edit_file", "write_file", "read_file")
    saved_total = 0.0
    for tool in reproduced:
        raw = wire = calls = 0
        for row in zo_rows:
            for chars, size, wired, _hypo in row["tool_results"].get(tool, []):
                calls += 1
                raw += size
                wire += wired
        if not calls:
            continue
        saved_total += (raw - wire) / count
        print(
            f"{tool:<16}{calls:>9,}{raw / count:>13,.0f}{wire / count:>14,.0f}"
            f"{(raw - wire) / count:>10,.0f}{100 * (raw - wire) / raw:>8.1f}%"
        )
    zo_new = statistics.fmean([row["new_bytes"] for row in zo_rows])
    cc_new = statistics.fmean([row["new_bytes"] for row in cc_rows])
    print("-" * len(header))
    print(f"{'재현한 셋 합':<16}{'':>9}{'':>13}{'':>14}{saved_total:>10,.0f}")
    print()
    print("  그리고 이 패스에 아예 안 들어가는 도구가 하나 있다:")
    raw = calls = hypo = 0
    for row in zo_rows:
        for chars, size, _wired, hypothetical in row["tool_results"].get("MultiEdit", []):
            calls += 1
            raw += size
            hypo += hypothetical
    print(f"{'MultiEdit':<16}{calls:>9,}{raw / count:>13,.0f}{raw / count:>14,.0f}"
          f"{0:>10,.0f}{0.0:>8.1f}%   ← 와이어 패스 없음")
    if calls:
        print(f"{'  (걸었다면)':<16}{calls:>9,}{raw / count:>13,.0f}{hypo / count:>14,.0f}"
              f"{(raw - hypo) / count:>10,.0f}{100 * (raw - hypo) / raw:>8.1f}%")
    print("  `compress_tool_output_with` 의 갈래는 소문자화 뒤 \"edit\" | \"edit_file\" 이고")
    print("  `canonical_tool_name` 은 multi_edit → MultiEdit → \"multiedit\" 를 낸다. 어느 쪽에도")
    print("  안 맞아 그대로 나간다 — 그 파일의 주석이 경고한 바로 그 모양이다")
    print("  (\"without this every Claude tool result fell through to `unchanged`\").")
    print(f"\n  zo 요청당 새 내용: 전사 {zo_new:,.0f} B → 이 셋만 와이어로 고쳐도 "
          f"{zo_new - saved_total:,.0f} B")
    print(f"  같은 자리 Claude Code {cc_new:,.0f} B → 비 {zo_new / cc_new:.2f}배 → "
          f"{(zo_new - saved_total) / cc_new:.2f}배")
    print("  bash·grep_search·glob_search 의 와이어 패스와 read_file 의 개요/경계 뷰는 재현하지")
    print("  않았다 — 그것들도 줄이므로 이 줄조차 상한이다. 하한은 프로바이더가 센 토큰 축이다.")
    return saved_total


def print_thinking_table(title, groups):
    """docstring note 10: how often vs how big."""
    print(f"\n## {title}")
    header = (
        f"{'bucket':<24}{'N':>8}{'with thinking':>15}{'blocks/req':>12}"
        f"{'B/req':>10}{'B when present':>16}{'B/block':>10}{'signature':>11}{'sig share':>11}"
    )
    print(header)
    print("-" * len(header))
    measured = {}
    for label, rows in groups:
        if not rows:
            continue
        present = [row for row in rows if row["thinking_bytes"] > 0]
        blocks = sum(row["thinking_blocks"] for row in rows)
        total = sum(row["thinking_bytes"] for row in rows)
        signature = sum(row["signature_bytes"] for row in rows)
        measured[label] = (blocks / len(rows), total / blocks if blocks else 0.0)
        print(
            f"{label:<24}{len(rows):>8}{100 * len(present) / len(rows):>14.1f}%"
            f"{blocks / len(rows):>12.2f}{total / len(rows):>10,.0f}"
            f"{(total / len(present) if present else 0):>16,.0f}"
            f"{(total / blocks if blocks else 0):>10,.0f}"
            f"{signature / len(rows):>11,.0f}{(100 * signature / total if total else 0):>10.1f}%"
        )
    labels = list(measured)
    if len(labels) >= 2:
        (zc, zs), (cc_c, cs) = measured[labels[0]], measured[labels[1]]
        count_term, size_term = shapley(zc, zs, cc_c, cs)
        print(
            f"  같은 분해를 thinking 에: 블록 수 {count_term:,.0f} B/req · 블록 크기 "
            f"{size_term:,.0f} B/req  (합 {count_term + size_term:,.0f})"
        )
    print("  signature 는 모델의 사고가 아니라 서명 blob 이다 — 블록마다 실리므로 블록 수에 비례한다.")


def print_hidden_thinking(title, zo_rows, cc_rows):
    """docstring note 12: bound what Claude Code's transcript does not show.

    zo persists `thinking` and `signature`; Claude Code persists only the
    signature. On zo's own Anthropic blocks the two travel together, so the
    ratio (thinking text bytes / signature bytes) measured there is the best
    available estimator of the text sitting behind a Claude Code signature.
    It is an ESTIMATE with one assumption named in the report -- that the two
    products' models summarize reasoning at a similar rate -- and it is checked
    against the provider-counted token axis, which never lost the bytes.
    """
    print(f"\n## {title}")
    pairs = [pair for row in zo_rows for pair in row["thinking_pairs"] if pair[1] > 0]
    if not pairs:
        print("  zo 쪽 서명 있는 thinking 블록이 없다 — 추정 불가.")
        return None
    text_total = sum(pair[0] for pair in pairs)
    sig_total = sum(pair[1] for pair in pairs)
    ratio = text_total / sig_total
    per_block = sorted(pair[0] / pair[1] for pair in pairs)
    median_ratio = per_block[len(per_block) // 2]
    cc_sig = sum(row["signature_bytes"] for row in cc_rows) / max(1, len(cc_rows))
    cc_text_seen = (
        sum(row["thinking_bytes"] - row["signature_bytes"] for row in cc_rows) / max(1, len(cc_rows))
    )
    zo_new = statistics.fmean([row["new_bytes"] for row in zo_rows])
    cc_new = statistics.fmean([row["new_bytes"] for row in cc_rows])
    print(f"  zo Anthropic thinking 블록 {len(pairs):,} 개: 본문 {text_total / len(pairs):,.0f} B ·"
          f" 서명 {sig_total / len(pairs):,.0f} B / 블록")
    print(f"  본문/서명 비 — 총량 {ratio:.3f} · 블록 중앙값 {median_ratio:.3f}")
    print(f"  Claude Code 전사에 실제로 남은 thinking 본문: {cc_text_seen:,.1f} B/req (0 이면 전부 벗겨진 것)")
    estimate = ratio * cc_sig
    print(f"  서명 {cc_sig:,.0f} B/req 뒤에 있었을 본문 추정: {estimate:,.0f} B/req")
    header = f"  {'축':<34}{'zo':>12}{'CC':>12}{'ratio':>9}"
    print(header)
    print("  " + "-" * (len(header) - 2))
    print(f"  {'전사 그대로 (r24a §1)':<34}{zo_new:>12,.0f}{cc_new:>12,.0f}{zo_new / cc_new:>9.2f}")
    print(f"  {'CC 에 추정 본문을 더해서':<34}{zo_new:>12,.0f}{cc_new + estimate:>12,.0f}"
          f"{zo_new / (cc_new + estimate):>9.2f}")
    zo_ex = zo_new - sum(row["thinking_bytes"] for row in zo_rows) / len(zo_rows)
    cc_ex = cc_new - sum(row["thinking_bytes"] for row in cc_rows) / len(cc_rows)
    print(f"  {'thinking 을 양쪽에서 빼고':<34}{zo_ex:>12,.0f}{cc_ex:>12,.0f}{zo_ex / cc_ex:>9.2f}")
    zo_usable = [row for row in zo_rows if row.get("delta_in")]
    cc_usable = [row for row in cc_rows if row.get("delta_in")]
    if zo_usable and cc_usable:
        zo_tok = statistics.fmean([row["delta_in"] for row in zo_usable])
        cc_tok = statistics.fmean([row["delta_in"] for row in cc_usable])
        print(f"  {'프로바이더가 센 토큰 (교차 검증)':<34}{zo_tok:>12,.0f}{cc_tok:>12,.0f}"
              f"{zo_tok / cc_tok:>9.2f}")
    print("  마지막 줄은 이 결함이 닿을 수 없는 축이다 — 과금 토크나이저는 전송된 것을 셌다.")
    return estimate


def print_reminder_table(title, zo_rows, cc_rows, limit=14):
    """docstring note 11: same question asked of both spellings."""
    print(f"\n## {title}")

    def profile(rows):
        count = len(rows) or 1
        totals: dict = {}
        hits: dict = {}
        for row in rows:
            for name, value in row.get("reminders", {}).items():
                add(totals, name, value)
                hits[name] = hits.get(name, 0) + 1
        return {name: (value / count, hits[name]) for name, value in totals.items()}, count

    zo_profile, zo_n = profile(zo_rows)
    cc_profile, cc_n = profile(cc_rows)
    zo_total = sum(value for value, _ in zo_profile.values())
    cc_total = sum(value for value, _ in cc_profile.values())
    print(f"  zo {zo_total:,.0f} B/req  vs  Claude Code {cc_total:,.0f} B/req"
          f"   (zo N={zo_n:,}, CC N={cc_n:,})")
    header = f"  {'tag':<34}{'zo B/req':>10}{'reqs':>8}   {'tag':<34}{'CC B/req':>10}{'reqs':>8}"
    print(header)
    print("  " + "-" * (len(header) - 2))
    zo_top = sorted(zo_profile.items(), key=lambda item: -item[1][0])[:limit]
    cc_top = sorted(cc_profile.items(), key=lambda item: -item[1][0])[:limit]
    for index in range(max(len(zo_top), len(cc_top))):
        left = right = f"  {'':<34}{'':>10}{'':>8}"
        if index < len(zo_top):
            name, (value, count) = zo_top[index]
            left = f"  {name[:34]:<34}{value:>10,.0f}{count:>8,}"
        if index < len(cc_top):
            name, (value, count) = cc_top[index]
            right = f"   {name[:34]:<34}{value:>10,.0f}{count:>8,}"
        else:
            right = ""
        print(left + right)
    print("  이 열은 다른 종류와 겹친다 — 리마인더 바이트는 이미 system_text/user_text/")
    print("  tool_result 안에 세어져 있다. 분할이 아니라 가로지르는 자다.")
    print("  zo 는 리마인더를 자기 role:system 메시지로, Claude Code 는 사용자 메시지 본문 안")
    print("  <system-reminder> 로 적는다 — 그런데 CC 전사에는 그 주입이 남지 않는다(위 열이")
    print("  ~0 인 이유다. 첫 사용자 메시지에도 없다). 그러므로 이 표는 r24a §6.6 의 주의를")
    print("  풀지 못하고 확정한다: 이 축은 전사로 비교할 수 없다. zo 열만 참이다.")
    return zo_total, cc_total


# ---------------------------------------------------------------------------
# Goal 4 -- what a cap would have saved on this corpus
# ---------------------------------------------------------------------------


def cap_saving(rows, predicate, cap_chars, notice=NOTICE_CHARS):
    """Bytes/request a `cap_chars` character cap would have removed.

    Applied per individual RESULT, the unit the cap actually acts on. Byte
    saving is prorated from the character saving by that result's own measured
    bytes/char, so Hangul results are charged their real 3 bytes and ASCII
    ones 1 -- the corpus mixes both inside one tool.
    """
    count = len(rows) or 1
    saved_bytes = 0.0
    saved_chars = 0
    hit = 0
    total = 0
    for row in rows:
        for name, values in row["tool_results"].items():
            if not predicate(name):
                continue
            for chars, size, _wire, _hypo in values:
                total += 1
                if chars <= cap_chars:
                    continue
                hit += 1
                cut = chars - cap_chars - notice
                if cut <= 0:
                    continue
                density = size / chars if chars else 1.0
                saved_chars += cut
                saved_bytes += cut * density
    return {
        "saved_bytes_per_req": saved_bytes / count,
        "saved_chars_per_req": saved_chars / count,
        "results_cut": hit,
        "results_seen": total,
    }


def print_cap_table(title, rows, candidates, baseline_bytes, write_per_req, bytes_per_token):
    print(f"\n## {title}")
    header = (
        f"{'policy':<40}{'cut/seen':>14}{'saved c/req':>13}{'saved B/req':>13}"
        f"{'saved tok':>11}{'of new':>8}{'of write':>10}"
    )
    print(header)
    print("-" * len(header))
    for label, predicate, cap in candidates:
        result = cap_saving(rows, predicate, cap)
        saved = result["saved_bytes_per_req"]
        saved_tokens = saved / bytes_per_token
        print(
            f"{label:<40}{result['results_cut']:>6,}/{result['results_seen']:<7,}"
            f"{result['saved_chars_per_req']:>13,.0f}{saved:>13,.0f}{saved_tokens:>11,.0f}"
            f"{100 * saved / baseline_bytes:>7.1f}%"
            + (f"{100 * saved_tokens / write_per_req:>9.2f}%" if write_per_req else f"{'—':>10}")
        )
    print(f"  `of new`  = share of this bucket's {baseline_bytes:,.0f} B/req of new content.")
    if not write_per_req:
        print("  `of write` is blank: this bucket writes no cache (OpenAI-compat records")
        print("    `cache_creation` = 0, r24a §0), so the share has no denominator.")
        return
    print(f"  `of write`= saved tokens as a share of this bucket's {write_per_req:,.0f} cache-write")
    print("    tokens/request. That denominator is dominated by cache MISSES (r24a §2:")
    print("    3.7% of requests carry 42% of the writes), which re-write the whole prefix")
    print("    and are not paid for by any one result -- so this column is a floor on a")
    print("    hit-only basis and does not add up across rows sharing a result.")


# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--zo-root", default=os.path.expanduser("~/.zo/projects"))
    parser.add_argument("--cc-root", default=os.path.expanduser("~/.claude/projects"))
    parser.add_argument("--no-vault", action="store_true")
    parser.add_argument("--rows", help="write every per-request row to this JSONL path")
    parser.add_argument("--tools", type=int, default=18)
    parser.add_argument("--since", default=DEFAULT_SINCE)
    parser.add_argument("--since-r27", default=DEFAULT_SINCE_R27)
    args = parser.parse_args()

    since_ms = int(datetime.fromisoformat(args.since).timestamp() * 1000)
    r27_ms = int(datetime.fromisoformat(args.since_r27).timestamp() * 1000)

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
                if row["key"] in seen_keys:
                    continue
                seen_keys.add(row["key"])
                cc_rows.append(row)

    print("# write-volume-by-tool (r31)")
    print(f"  zo store : {zo_root}")
    print(f"  cc store : {args.cc_root}   ({len(cc_top)} top-level, {len(cc_nested)} nested files)")
    print(f"  vault restoration: {'OFF (--no-vault)' if args.no_vault else 'ON'}")
    print(f"  run at   : {datetime.now().isoformat(timespec='seconds')}")

    zo_anthropic = rolling(zo_rows, ("anthropic",))
    zo_openai = rolling(zo_rows, ("openai",))
    cc_main = rolling([row for row in cc_rows if row["product"] == "cc"])
    cc_sub = rolling([row for row in cc_rows if row["product"] == "cc-sub"])

    # ---- §1 coordinate check ------------------------------------------------
    print("\n## 1. 좌표계 검증 — 새 자로 옛 숫자를 다시 낸다")
    header = f"  {'quantity':<44}{'this ruler':>13}{'expected':>11}{'drift':>9}{'N':>10}"
    print(header)
    print("  " + "-" * (len(header) - 2))
    cc_all = [row for row in cc_rows if row["product"] == "cc"]
    zo_all_anthropic = [row for row in zo_rows if row["family"] == "anthropic"]
    checks = [
        (
            "r19/r20 · CC top-level write/req (all requests)",
            sum(row["write"] for row in cc_all) / max(1, len(cc_all)),
            3_668,
            len(cc_all),
        ),
        (
            "r20 §3 · zo Anthropic write/req (all requests)",
            sum(row["write"] for row in zo_all_anthropic) / max(1, len(zo_all_anthropic)),
            33_881,
            len(zo_all_anthropic),
        ),
        (
            "r24a §1 · zo Anthropic new bytes/req (rolling)",
            statistics.fmean([row["new_bytes"] for row in zo_anthropic]) if zo_anthropic else 0,
            8_057,
            len(zo_anthropic),
        ),
        (
            "r24a §1 · CC main new bytes/req (rolling)",
            statistics.fmean([row["new_bytes"] for row in cc_main]) if cc_main else 0,
            3_499,
            len(cc_main),
        ),
        (
            "r24a §1 · zo Anthropic new TOKENS/req (rolling)",
            statistics.fmean([row["delta_in"] for row in zo_anthropic if row.get("delta_in")]),
            3_063,
            sum(1 for row in zo_anthropic if row.get("delta_in")),
        ),
        (
            "r24a §1 · CC main new TOKENS/req (rolling)",
            statistics.fmean([row["delta_in"] for row in cc_main if row.get("delta_in")]),
            1_676,
            sum(1 for row in cc_main if row.get("delta_in")),
        ),
    ]
    for label, got, expected, count in checks:
        drift = 100 * (got - expected) / expected
        print(f"  {label:<44}{got:>13,.0f}{expected:>11,}{drift:>8.1f}%{count:>10,}")
    print("  코퍼스는 얼어 있지 않다(r24a §6.4): 같은 자를 몇 시간 뒤에 돌리면 요청 수가 움직이고")
    print("  30일 스윕이 옛 세션을 지운다. 부호와 자릿수를 본다.")

    groups = [
        ("zo · Anthropic", zo_anthropic),
        ("zo · OpenAI-compat", zo_openai),
        ("Claude Code · main", cc_main),
        ("Claude Code · sub-agent", cc_sub),
    ]

    # ---- §2 goal 1 ----------------------------------------------------------
    print_kind_table("2. 목표 1 — 요청당 새 내용의 종류별 구성 (mean B/req · share)", groups)
    print_kind_dist("2b. 같은 축의 분포 (요청당 바이트)", [("zo · Anthropic", zo_anthropic),
                                                           ("Claude Code · main", cc_main)])

    zo_acc = tool_profile(zo_anthropic)
    cc_acc = tool_profile(cc_main)
    print_tool_side_by_side(
        "3. 목표 1 — tool_result 를 도구 계열별로: 많이 부르나 / 크게 받나",
        zo_acc, cc_acc, len(zo_anthropic), len(cc_main),
    )
    print_tool_raw("3b. zo · Anthropic 원본 도구 이름", tool_profile(zo_anthropic, by_family=False), args.tools)
    print_tool_raw("3c. Claude Code · main 원본 도구 이름", tool_profile(cc_main, by_family=False), args.tools)

    # ---- §4 goal 2 ----------------------------------------------------------
    total_gap, kind_ranking, tool_ranking, wire_gap = print_gap_ranking(
        "4. 목표 2 — 격차 설명 순위 (zo Anthropic 대 Claude Code main, rolling)",
        zo_anthropic, cc_main,
    )
    top3_kind = sum(item[3] for item in kind_ranking[:3])
    top3_tool = sum(item[1] for item in tool_ranking[:3])
    print(f"\n  상위 세 종류가 격차의 {100 * top3_kind / total_gap:.1f}% 를 설명한다.")
    print(f"  tool_result 안에서 상위 세 계열이 전체 격차의 {100 * top3_tool / total_gap:.1f}% 를 설명한다.")
    count_total = sum(item[2] for item in tool_ranking)
    size_total = sum(item[3] for item in tool_ranking)
    print(f"  tool_result 격차의 분해: 호출 수 {count_total:,.0f} B/req · 결과 크기 {size_total:,.0f} B/req.")

    # ---- §5 goal 3 ----------------------------------------------------------
    print(f"\n## 5. 목표 3 — r27 상한 배포({args.since_r27}) 이후 슬라이스")
    for label, rows in (("zo · Anthropic", zo_anthropic), ("Claude Code · main", cc_main)):
        buckets = {"before": [], "after": [], "straddle": []}
        for row in rows:
            buckets[period_of(row, r27_ms)].append(row)
        after = buckets["after"]
        line = f"  {label:<22} after N={len(after):<7,} before N={len(buckets['before']):<8,} straddle N={len(buckets['straddle']):<7,}"
        if after:
            line += f"  new B/req={statistics.fmean([r['new_bytes'] for r in after]):,.0f}"
        print(line)
    newest_zo = max((row["ts_ms"] or 0) for row in zo_rows) if zo_rows else 0
    newest_cc = max((row["ts_ms"] or 0) for row in cc_rows) if cc_rows else 0
    for label, stamp in (("zo", newest_zo), ("Claude Code", newest_cc)):
        when = datetime.fromtimestamp(stamp / 1000).isoformat(timespec="minutes") if stamp else "unknown"
        print(f"  {label} 코퍼스의 가장 최근 요청: {when}")

    # ---- §6 goal 4 ----------------------------------------------------------
    zo_new = statistics.fmean([row["new_bytes"] for row in zo_anthropic]) if zo_anthropic else 1
    zo_write = sum(row["write"] for row in zo_anthropic) / max(1, len(zo_anthropic))
    usable = [row for row in zo_anthropic if row.get("delta_in")]
    zo_bpt = (
        sum(row["new_bytes"] for row in usable) / sum(row["delta_in"] for row in usable)
        if usable
        else BYTES_PER_TOKEN
    )
    print(f"\n  측정된 환산율 B/tok (zo Anthropic, rolling): {zo_bpt:.2f}")

    def named(*names):
        return lambda tool: tool in names

    candidates = [
        ("read_file 30,000자 (r27, 이미 배포)", named("read_file"), 30_000),
        ("read_file 16,000자", named("read_file"), 16_000),
        ("read_file 8,000자", named("read_file"), 8_000),
        ("MCP 성공결과 30,000자 (r27, 이미 배포)", lambda t: tool_family(t) == "mcp", 30_000),
        ("MCP 성공결과 8,000자", lambda t: tool_family(t) == "mcp", 8_000),
        ("bash 16,384 → 8,000자", named("bash"), 8_000),
        ("bash 16,384 → 4,000자", named("bash"), 4_000),
        ("grep_search 30,000 → 8,000자", named("grep_search"), 8_000),
        ("grep_search 30,000 → 4,000자", named("grep_search"), 4_000),
        ("session_recall 30,000 → 8,000자", named("session_recall"), 8_000),
        ("edit_file 결과 4,000자", named("edit_file"), 4_000),
        ("전역 8,000자 (모든 도구)", lambda t: True, 8_000),
        ("전역 4,000자 (모든 도구)", lambda t: True, 4_000),
    ]
    print_cap_table(
        "6. 목표 4 — 문자 상한 후보와 절감 추정 (zo Anthropic, rolling, 이 코퍼스에 소급 적용)",
        zo_anthropic, candidates, zo_new, zo_write, zo_bpt,
    )

    zo_o_new = statistics.fmean([row["new_bytes"] for row in zo_openai]) if zo_openai else 1
    zo_o_write = sum(row["write"] for row in zo_openai) / max(1, len(zo_openai))
    print_cap_table(
        "6b. 같은 상한을 OpenAI-compat 경로에 (tool_result 가 81% 인 통 — 상한이 무는 자리)",
        zo_openai, candidates, zo_o_new, zo_o_write if zo_o_write > 1 else 0, zo_bpt,
    )

    # r27 §3 published one total for each of its two caps. Reproducing them
    # from this ruler is the second coordinate check of the run -- and it is
    # what shows that r27's "Anthropic 요청당" column was the WHOLE corpus's
    # saving over the Anthropic denominator, not the saving that fell on
    # Anthropic requests.
    print("\n## 6c. r27 §3 의 총 절감을 이 자로 재현하고, 통별로 가른다")
    zo_rolling = zo_anthropic + zo_openai + rolling(zo_rows, ("google", "unknown"))
    header = f"  {'상한':<26}{'zo 전체 절감(자)':>18}{'r27 §3':>12}{'표류':>8}{'Anthropic c/req':>17}{'OpenAI c/req':>14}"
    print(header)
    print("  " + "-" * (len(header) - 2))
    for label, predicate, published in (
        ("read_file 봉투 30,000자", lambda t: t == "read_file", 4_099_735),
        ("MCP 성공결과 30,000자", lambda t: tool_family(t) == "mcp", 4_638_082),
    ):
        whole = cap_saving(zo_rolling, predicate, 30_000)["saved_chars_per_req"] * len(zo_rolling)
        anthropic = cap_saving(zo_anthropic, predicate, 30_000)["saved_chars_per_req"]
        openai = cap_saving(zo_openai, predicate, 30_000)["saved_chars_per_req"]
        print(
            f"  {label:<26}{whole:>18,.0f}{published:>12,}{100 * (whole - published) / published:>7.1f}%"
            f"{anthropic:>17,.0f}{openai:>14,.0f}"
        )
    print(f"  분모: zo 굴러가는 요청 전체 {len(zo_rolling):,} · Anthropic {len(zo_anthropic):,}"
          f" · OpenAI-compat {len(zo_openai):,}")
    print("  r27 §3 의 'Anthropic 요청당' 열은 전체 절감을 Anthropic 분모로 나눈 값이었다.")
    print("  실제로 그 절감이 떨어진 자리는 OpenAI-compat 통이다 — 마지막 두 열이 그것이다.")

    # ---- §7 the envelope --------------------------------------------------
    echo, derived, receipt = print_envelope_table(
        "7. 도구 결과 봉투를 열어 필드별로 — 되돌아온 것은 방금 보낸 것이다 (zo Anthropic, rolling)",
        zo_anthropic,
    )
    total_gap_for_echo = zo_new - (statistics.fmean([row["new_bytes"] for row in cc_main]) if cc_main else 0)
    print(f"\n  echo+derived = {echo + derived:,.0f} B/req · 이 통 새 내용의 "
          f"{100 * (echo + derived) / zo_new:.1f}% · 격차 {total_gap_for_echo:,.0f} B/req 의 "
          f"{100 * (echo + derived) / total_gap_for_echo:.1f}%")
    print(f"  같은 자리에서 Claude Code 의 Edit/Write 결과는 도합 "
          f"{sum(cell['bytes_per_req'] for name, cell in cc_acc.items() if name in ('edit', 'write')):,.0f} B/req 다.")

    wire_saved = print_wire_recompute(
        "7b. 그 봉투는 와이어에 그대로 나가지 않는다 — 변환을 다시 돌려 실제 크기를 낸다",
        zo_anthropic, cc_main,
    )

    # ---- §8 thinking ------------------------------------------------------
    print_thinking_table(
        "8. 두 번째 기여자는 도구가 아니다 — thinking 을 빈도와 크기로 가른다",
        [("zo · Anthropic", zo_anthropic), ("Claude Code · main", cc_main),
         ("Claude Code · sub-agent", cc_sub)],
    )

    hidden = print_hidden_thinking(
        "8b. 그런데 Claude Code 전사에는 thinking 본문이 없다 — 그 행은 같은 자가 아니었다",
        zo_anthropic, cc_main,
    )

    # ---- §9 reminders -----------------------------------------------------
    print_reminder_table(
        "9. 세 번째 기여자 — 리마인더를 두 철자에서 같은 자로 (r24a §6.6 의 주의를 푼다)",
        zo_anthropic, cc_main,
    )

    # The same question asked of the COLD prefix. A reminder that rides in the
    # session's first request is paid once and then read from cache forever; one
    # re-injected mid-session is new content every time it appears. If the two
    # products differ here and not in the rolling table, the difference is WHEN
    # they inject, not how much they inject.
    zo_cold = [row for row in zo_rows if row["first"] and row["family"] == "anthropic"]
    cc_cold = [row for row in cc_rows if row["first"] and row["product"] == "cc"]
    for label, rows in (("zo · Anthropic", zo_cold), ("Claude Code · main", cc_cold)):
        if not rows:
            continue
        total = sum(sum(row.get("reminders", {}).values()) for row in rows) / len(rows)
        new_bytes = statistics.fmean([row["new_bytes"] for row in rows])
        print(f"  세션 첫 요청(콜드 접두사)의 리마인더 — {label:<20} {total:>9,.0f} B/req"
              f"   (첫 요청 전체 {new_bytes:,.0f} B, N={len(rows):,})")

    if args.rows:
        with open(args.rows, "w", encoding="utf-8") as handle:
            for row in zo_rows + cc_rows:
                handle.write(json.dumps({k: v for k, v in row.items() if k != "key"}, ensure_ascii=False) + "\n")
        print(f"\n  wrote {len(zo_rows) + len(cc_rows):,} rows to {args.rows}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
