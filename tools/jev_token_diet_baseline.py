#!/usr/bin/env python3
"""Jev token diet, step 0 — where zo's tokens and tool calls go, and how much a
judgment at each of seven seats could take back. Measurement only.

    tools/jev_token_diet_baseline.py                                # last 7 days
    tools/jev_token_diet_baseline.py --days 7 --json /tmp/diet.json
    tools/jev_token_diet_baseline.py --prompt-input pi.txt          # split the tool block

Every number in `docs/design/jev-token-diet-20260917.md` comes from this script.
It reads local stores only and sends nothing:

    transcripts   ~/.zo/projects/*/sessions/session-*.jsonl (+ .rot-*, + .vault)
    sub-agents    ~/.zo/projects/*/state/agents/agent-*.session.jsonl (+ .vault)
    requests      ~/.zo/cache/prompt-cache/*/requests.jsonl  (effort, TTL)
    timings       ~/.zo/projects/*/state/request-timings/timings.jsonl
    session prefs ~/.zo/projects/*/session-prefs/*.json      (a session's effort)
    Jev ledgers   ~/.zo/projects/*/state/smart-router/{decision,rerank}-shadow.jsonl
    prices        crates/model-prices/resources/model-prices.json

It prints counts, token sums and shares — never a message's words, a path, a
project's name, a key or an address. Conversation text is read to count its
bytes, to hash a tool result, and to see whether a word it introduced is used
later; none of it is printed or written.

Imported rather than copied: r31 `write-volume-by-tool.py` (which files make a
session, rotated segments and the vault, the wire view of a result), r33
`reminder-cost.py` (what an injected message is), r40 `tool-habits-r40.py`
(which shell commands read or wait, which tools delegate, the digest mark), and
`tools/decision_shadow_summary.py` (a Jev ledger's answered share and uncached
latency).

DEFINITIONS — each is a decision, so it lives here; the design doc quotes them.

1. SAMPLE. A session belongs if its transcript was written inside the window
   and its project is not a harness test or a probe (`SYNTHETIC_PROJECT_MARKS`:
   temp directories and agent scratchpads). A request belongs if its session
   does and its message stamp, when it has one, is inside the window.
   A SUB-AGENT's transcript is wherever its `state/agents/<id>/result*.json`
   points (usually a session file of its own under `sessions/`), or
   `state/agents/<id>.session.jsonl`; those sessions are sub-agents, never
   main sessions, and their model is the brief's effective model when the
   transcript does not name one.

2. TURN. A turn opens at a person's message, or at a harness message that
   starts work of its own (`TURN_OPENING_HARNESS`: a /loop iteration, a
   background-task notification). The turn-end gate and mid-turn steering
   continue the turn in flight.

3. REQUEST. One assistant message carrying `usage`. Its input is
   `input_tokens + cache_read + cache_creation` as the provider counted it.
   Output folds thinking in for every provider; only Anthropic reports
   `thinking_tokens` apart, so thinking is exact there and not separable
   elsewhere.

4. TOKENS BY KIND. The provider gives a request's input as one number, split in
   two steps. (a) A session's FIXED PREFIX — system prompt, tool schemas, skill
   index — is its first request's input less the calibrated size of the
   messages before it; the tool block's share comes from a provider-counted
   `zo --prompt-input` report when one is given, and the skill index's share
   from the session's own `.system-prompt.json`. (b) The rest is split over the
   messages in context by their calibrated size. The calibration is a
   least-squares fit, per provider family, of how much each request's input
   grew against what the conversation gained since the last one (prose bytes,
   tool payload bytes, thinking bytes, blocks), refit once without residuals
   past `OUTLIER_MADS`. The split is scaled so the kinds sum to the provider's
   number exactly; the scale's spread is printed as the error. A tool result
   the harness has since cleared in place is carried at its cleared size (the
   transcript's present view), so the tool-result share is a floor.

5. CARRY. A block appended before request k is carried by every later request
   of the session until a compaction drops it. It costs its tokens fresh
   (uncached or cache write) once, and again at every break after k; at the
   other carrying requests it is a cache read. A BREAK is a request whose cache
   read fell under `BREAK_READ_SHARE` of the previous request's whole input —
   the provider-neutral sign that the prefix was rewritten; a model that never
   reported a cache read in the session has no cache to break. A result the
   harness later cleared is carried only until the first break after it (the
   earliest point an in-place rewrite could have landed), so what is saved on
   it is a floor.

6. DUPLICATE. A read-only call (file read, search, listing, shell read) whose
   input equals an earlier one's in the same context. It is SAFE to block when
   its result is byte-identical to the earlier one (timing fields removed) —
   the label a perfect judgment would have; when the result differs, blocking
   would have served stale content. The RULE column is what a judgment-free
   rule would do: block when no edit (to that path, for a file read) and no
   non-read shell command ran since the earlier call. A NEAR-DUPLICATE shares
   only the coarse key (the same file whatever the range, the same pattern and
   root whatever the output options, the same command) and is safe when it is
   REDUNDANT: at least `REDUNDANT_LINE_SHARE` of its distinct lines were
   already in context.

7. USE. A tool result or an injection is USED when a word it introduced — an
   identifier of at least six characters or a Hangul word of three syllables,
   not in the call's own input, not in the person's words for the turn, not
   common to a fifth of the session's results, and never said by the assistant
   before — appears in an assistant message (text, tool input or thinking)
   later in the same turn. For eviction the question is whether any such word
   is said after the rewrite point, anywhere in the session. With fewer than
   `MIN_DISTINCT_TOKENS` such words a block is not judged. This is a lexical
   proxy: a read that only confirmed what the model believed, or a check whose
   exit status was all it gave, reads as unused — so every "unused" figure is
   an upper bound on what a perfect judgment could drop.

8. SEAT BOUND. For each seat, PERFECT is the cost of the items a correct
   judgment removes (result tokens × carry, whole requests, injections,
   thinking). At hit rate h the judgment is right with probability h on every
   case it decides: it keeps h × PERFECT, pays MISS for every case where acting
   would have been wrong (the seat's NEGATIVES) with probability 1 - h — or
   1 - h^n for a block judged again at each of n rewrite points while it is
   still needed — and pays the Jev call on every decision. A MISS is priced in
   the case's own session (its median request, or median turn), so token and
   dollar columns stay in one currency each. Seats overlap (a duplicate read is
   also an unused read); unions are taken item by item, never by adding seat
   totals.
"""

from __future__ import annotations

import argparse
import bisect
import hashlib
import importlib.util
import json
import os
import re
import statistics
import sys
import time
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
# The rulers this script imports rather than copies. They moved out of
# zo-ide/docs on 2026-09-21 — a ruler is code, and the notes do not ship.
ANALYSIS_TOOLS = REPO / "tools"
PRICE_TABLE = REPO / "crates" / "model-prices" / "resources" / "model-prices.json"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"this script needs the ruler beside it: {path}")
    module = importlib.util.module_from_spec(spec)
    # Registered before exec: a `@dataclass` looks its module up in sys.modules.
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


r33 = _load("reminder_cost", ANALYSIS_TOOLS / "reminder-cost.py")
r31 = r33.r31
r40 = _load("tool_habits_r40", ANALYSIS_TOOLS / "tool-habits-r40.py")
shadow = _load("decision_shadow_summary", REPO / "tools" / "decision_shadow_summary.py")

# ---------------------------------------------------------------------------
# Named decisions (see DEFINITIONS)
# ---------------------------------------------------------------------------

DEFAULT_DAYS = 7.0
# The brief's three "reasonable" hit rates.
DEFAULT_HIT_RATES = (0.70, 0.85, 0.95)

# 1. Project slugs a harness test or a probe leaves: temp directories and agent
#    scratchpads (a slug is the cwd with `/` turned to `-`, cut from the left).
SYNTHETIC_PROJECT_MARKS = ("-scratchpad-", "var-folders-", "-T-tmp", "-T-.tmp")

# 2. User messages the harness writes. The opening ones start work of their own.
TURN_OPENING_HARNESS = ("[zo:loop-iteration", "[task notification")
LOOP_MARK = "[zo:loop-iteration"
STEERING_MARK = "[User steering"
HARNESS_MARK = "[zo:"
GATE_MARK = "[zo:turn-end-gate]"
GATE_KIND = "turn-end-gate"

# 4. Calibration.
CALIBRATION_MIN_PAIRS = 50  # below this a family borrows the pooled fit
OUTLIER_MADS = 3.0
MAD_TO_SIGMA = 1.4826  # a normal distribution's σ per unit of median absolute deviation
CALIBRATION_FEATURES = ("text", "tool", "thinking", "blocks")
FAMILIES = ("anthropic", "openai", "google", "other")
# A compaction is in effect from the first request after its kept index whose
# input fell below this share of the request before it.
COMPACTION_DROP_SHARE = 0.6

# 5. A request whose cache read is below this share of the previous request's
#    whole input rewrote its prefix.
BREAK_READ_SHARE = 0.5

# 6. Duplicates.
VOLATILE_RESULT_KEYS = ("durationMs",)
MIN_LINE_CHARS = 12  # shorter lines (braces, blanks) say nothing about redundancy
REDUNDANT_LINE_SHARE = 0.9
READ_ONLY_KINDS = ("read", "search", "list", "shell_read")
CALL_KIND_BY_TOOL = {
    "read_file": "read",
    "grep_search": "search",
    "glob_search": "list",
    "edit_file": "edit",
    "MultiEdit": "edit",
    "write_file": "edit",
}
UNKEYED_ARGUMENTS = ("description", "timeout")  # arguments that do not change what a read returns

# 7. Use.
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]{5,}")
HANGUL_WORD = re.compile(r"[가-힣]{3,}")
MIN_DISTINCT_TOKENS = 3
COMMON_TOKEN_SHARE = 0.2
COMMON_TOKEN_MIN_RESULTS = 10  # a session with fewer results has no meaningful "common"

# Seat C: a result at least this large is a chunking candidate — half the bash
# cap (16 KiB), which is r41's digest budget (`DIGEST_BUDGET_DIVISOR`).
LARGE_RESULT_BYTES = 8 * 1024
# One Noul question per this many lines of a large result.
CHUNK_LINES = 20

# Seat A: injections a turn that calls no tool could go without, and the
# injections recall writes.
WORKING_STATE_FAMILY = "working-state"
RECALL_KINDS = ("recalled-memory", "recall-hint")

# Seat G: the check-shaped shell commands a completion claim is verified with.
CHECK_COMMAND = re.compile(r"\b(test|check|clippy|lint|build|verify|pytest|vitest|jest|tsc)\b")
FAILED_EXIT_PREFIX = "exit_code"  # `returnCodeInterpretation` on a non-zero exit

# Seat F: a sub-agent that made at most this many tool calls did work the
# parent could have done in one batch of its own.
SMALL_SUBAGENT_CALLS = 3

# A stamp below this is Unix seconds (1e12 ms is 2001-09-09).
SECONDS_STAMP_CEILING = 10**12

# Jev state sizes no ledger measures yet — assumptions, CLI options, printed.
DEFAULT_FILE_CANDIDATES = 20  # seat B: files put to Noul per edit turn
DEFAULT_TURN_PLAN_AXES = 5  # seat A: model tier, effort, tool need, task shape, recall need
DEFAULT_PRE_INJECT_FILES = 3  # seat B: top-k injected, all wasted when the pick is wrong
CLAIM_EVIDENCE_CANDIDATES = 2  # seat G: the claim and the edit/check record it rests on


# ---------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------


def iso_to_ms(text: str) -> int:
    return int(datetime.fromisoformat(text).timestamp() * 1000)


def ms_to_iso(ms: int) -> str:
    return datetime.fromtimestamp(ms / 1000).astimezone().isoformat(timespec="minutes")


def share(part: float, whole: float) -> float | None:
    return part / whole if whole else None


def quantile(values: list[float], q: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    # Nearest rank, the rule decision_shadow_summary.percentile keeps.
    rank = max(1, int(round(q * len(ordered))))
    return ordered[min(rank, len(ordered)) - 1]


def dist(values: list[float]) -> dict:
    return {
        "n": len(values),
        "mean": round(statistics.fmean(values), 1) if values else None,
        "p50": quantile(values, 0.5),
        "p90": quantile(values, 0.9),
        "sum": round(sum(values), 1),
    }


def parse_input(raw) -> dict:
    if isinstance(raw, dict):
        return raw
    try:
        value = json.loads(raw or "{}")
    except (TypeError, json.JSONDecodeError):
        return {}
    return value if isinstance(value, dict) else {}


def words(text: str) -> set[str]:
    return set(IDENTIFIER.findall(text)) | set(HANGUL_WORD.findall(text))


def fingerprint(text: str) -> str:
    return hashlib.blake2b(text.encode("utf-8", "replace"), digest_size=12).hexdigest()


def stable_result(text: str) -> str:
    """A result with its timing fields removed, so a repeat can be compared."""
    stripped = text.lstrip()
    if stripped.startswith("{"):
        try:
            value = json.loads(stripped)
        except json.JSONDecodeError:
            return text
        if isinstance(value, dict):
            for key in VOLATILE_RESULT_KEYS:
                value.pop(key, None)
            return json.dumps(value, ensure_ascii=False, sort_keys=True)
    return text


def result_body(tool: str, output: str) -> str:
    """What a result shows the model: the wire view, JSON envelopes opened."""
    wire = r31.wire_view(tool, output)
    value = parse_input(wire) if wire.lstrip().startswith("{") else None
    if not value:
        return wire
    parts = []
    file_part = value.get("file")
    if isinstance(file_part, dict) and isinstance(file_part.get("content"), str):
        parts.append(file_part["content"])
    for key in ("content", "stdout", "stderr", "output", "result"):
        if isinstance(value.get(key), str):
            parts.append(value[key])
    if isinstance(value.get("filenames"), list):
        parts.append("\n".join(str(name) for name in value["filenames"]))
    return "\n".join(parts) if parts else wire


def is_synthetic(project: str) -> bool:
    return any(mark in project for mark in SYNTHETIC_PROJECT_MARKS)


def provider_family(model: str | None) -> str:
    family = r31.model_family(model)
    return family if family in FAMILIES else "other"


def call_kind(name: str, arguments: dict) -> str:
    if name in CALL_KIND_BY_TOOL:
        return CALL_KIND_BY_TOOL[name]
    if name in r40.DELEGATION_TOOLS:
        return "delegate"
    if name == "bash":
        command = str(arguments.get("command") or "")
        if r40.is_wait(command):
            return "wait"
        if r40.first_program(command) in r40.READ_PROGRAMS:
            return "shell_read"
        return "shell"
    return "other"


def user_text_shape(text: str) -> str:
    """person | opening | steering | harness — who wrote a user message's text."""
    stripped = text.lstrip()
    if stripped.startswith(TURN_OPENING_HARNESS):
        return "opening"
    if stripped.startswith(STEERING_MARK):
        return "steering"
    if stripped.startswith(HARNESS_MARK):
        return "harness"
    return "person"


# ---------------------------------------------------------------------------
# Prices — a reader of the one table, for the ids this corpus holds
# ---------------------------------------------------------------------------


class Prices:
    """USD per million tokens from `model-prices.json`.

    Only the matching this corpus needs: an Anthropic id matches a row whose id
    it equals or extends with a date; an OpenAI id, tier suffix dropped, a row
    whose `ids` or `exact` it equals. Anything else is unpriced — never free —
    and the report says how much of the sample that leaves out.
    """

    MILLION = 1_000_000

    def __init__(self, path: Path):
        table = json.loads(path.read_text(encoding="utf-8"))
        self.anthropic = table["anthropic"]
        self.openai = table["openai"]
        self.jev = {row_id: row for row in table["typesafe"]["rows"] for row_id in row["ids"]}

    def row(self, model: str | None):
        name = re.sub(r"\[[^\]]*\]$", "", (model or "").lower())
        if name.startswith("claude-"):
            for row in self.anthropic["rows"]:
                if any(name == i or re.fullmatch(re.escape(i) + r"-\d{8}", name) for i in row["ids"]):
                    return "anthropic", row
        if name.startswith("gpt-"):
            for row in self.openai["rows"]:
                if name in row.get("ids", []) or name in row.get("exact", []):
                    return "openai", row
        return None, None

    def cost(self, model: str | None, fresh: float, read: float, output: float) -> float | None:
        kind, row = self.row(model)
        if row is None:
            return None
        if kind == "anthropic":
            read_rate = row.get("cache_read_rate", self.anthropic["cache_read_rate"])
            # Appends land on the rolling five-minute marker; an hour-long write
            # costs more, so fresh tokens are priced at their floor.
            write_rate = self.anthropic["cache_write_5m_rate"]
            return ((fresh * write_rate + read * read_rate) * row["input"] + output * row["output"]) / self.MILLION
        return (fresh * row["input"] + read * row["cached_input"] + output * row["output"]) / self.MILLION

    def jev_input(self) -> float:
        return self.jev["jev-latest"]["input"]


# ---------------------------------------------------------------------------
# The compact model of one session — numbers only; no text outlives its pass
# ---------------------------------------------------------------------------


@dataclass
class Message:
    index: int
    role: str
    turn: int
    text: int = 0  # prose bytes: a person, the assistant, injections
    tool: int = 0  # tool_use input + tool_result wire bytes
    thinking: int = 0
    blocks: int = 0
    kinds: dict = field(default_factory=dict)  # report kind -> bytes
    live_tool: int | None = None  # its results' wire bytes as the transcript holds them now
    cleared: bool = False


@dataclass
class Call:
    number: int
    name: str
    family: str
    kind: str
    request: int  # position in Session.requests, -1 when its message carried no usage
    turn: int
    batch: int
    path: str | None = None
    result_index: int | None = None
    result_bytes: int = 0
    is_error: bool = False
    check: bool = False
    failed_check: bool = False
    duplicate: str | None = None  # exact repeat: safe | changed
    near: str | None = None  # coarse repeat: safe | changed
    rule_would_block: bool = False
    redundant: bool = False
    used: bool | None = None  # None = not judgeable
    last_mention: int = -1  # last assistant message saying a word this result introduced
    judgeable: bool = False
    digest_raw_chars: int = 0
    large: bool = False
    chunks: int = 0
    chunks_needed: int = 0
    chunk_kept_share: float | None = None
    agent_id: str | None = None
    cleared: bool = False


@dataclass
class Injection:
    index: int
    turn: int
    kind: str
    family: str
    bytes: int
    used: bool | None = None


@dataclass
class Request:
    index: int
    model: str
    family: str
    turn: int
    read: int
    write: int
    uncached: int
    output: int
    thinking: int | None
    in_window: bool = True
    effort: str | None = None
    ttl: str | None = None
    ledger_broke: bool = False
    brk: bool = False
    ctx_start: int = 0
    calls: list = field(default_factory=list)
    kinds: dict = field(default_factory=dict)  # report kind -> tokens
    scale: float | None = None

    @property
    def total_in(self) -> int:
        return self.read + self.write + self.uncached


@dataclass
class Turn:
    index: int
    kind: str  # person | loop | notification | preamble
    start: int
    end: int = 0
    requests: list = field(default_factory=list)
    calls: list = field(default_factory=list)
    gates: list = field(default_factory=list)


@dataclass
class Session:
    key: str
    subagent: bool
    messages: list
    requests: list
    turns: list
    calls: list
    injections: list
    compactions: list
    skill_share: float
    effort_pref: str | None
    fixed: float = 0.0


# ---------------------------------------------------------------------------
# Reading
# ---------------------------------------------------------------------------


class AgentRoot:
    """A root whose glob answers with sub-agent transcripts, so r31's reader
    folds them (rotation, vault) exactly as it folds sessions."""

    def __init__(self, projects: Path):
        self.paths = sorted(projects.glob("*/state/agents/agent-*.session*.jsonl"))

    def glob(self, _pattern):
        return iter(self.paths)


@dataclass
class AgentLinks:
    """Where each sub-agent's transcript is, and what model it ran."""

    key_of: dict  # agent id -> session key
    model_of: dict  # agent id -> effective model

    @property
    def keys(self) -> set:
        return set(self.key_of.values())

    def agent_of(self, key: str) -> str | None:
        for agent, linked in self.key_of.items():
            if linked == key:
                return agent
        return None


def read_agent_links(projects: Path) -> AgentLinks:
    key_of, model_of = {}, {}
    for directory in projects.glob("*/state/agents/agent-*"):
        if not directory.is_dir():
            continue
        agent = directory.name
        brief = parse_input((directory / "brief.json").read_text(encoding="utf-8")
                            if (directory / "brief.json").exists() else "{}")
        model = ((brief.get("harness") or {}).get("model") or {}).get("effective") or brief.get("model")
        if model:
            model_of[agent] = model
        for result in sorted(directory.glob("result*.json")):
            transcript = parse_input(result.read_text(encoding="utf-8")).get("transcript")
            if transcript:
                path = Path(transcript)
                key_of[agent] = f"{path.parent.parent.name}/{path.name[: -len('.jsonl')]}"
        sidecar = directory.parent / f"{agent}.session.jsonl"
        if agent not in key_of and sidecar.exists():
            # r31 keys a transcript by its grandparent directory: `state` here.
            key_of[agent] = f"{sidecar.parent.parent.name}/{agent}.session"
    return AgentLinks(key_of=key_of, model_of=model_of)


def read_request_ledgers(root: Path, window) -> dict:
    """session or agent id -> {(read, write, uncached, output): row}."""
    since_ms, until_ms = window
    ledgers: dict = {}
    for path in root.glob("*/requests.jsonl"):
        rows = {}
        for row in r31.read_jsonl(path):
            stamp = row.get("ts_unix_ms") or 0
            if stamp and not since_ms <= stamp <= until_ms:
                continue
            rows[(row.get("cache_read", 0), row.get("cache_creation", 0), row.get("input_uncached", 0),
                  row.get("output", 0))] = row
        if rows:
            ledgers[path.parent.name] = rows
    return ledgers


def read_session_prefs(projects: Path) -> dict:
    prefs = {}
    for path in projects.glob("*/session-prefs/*.json"):
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(value, dict) and value.get("effort"):
            prefs[path.stem] = str(value["effort"])
    return prefs


SKILL_INDEX_HEADING = "# Available skills"


def skill_index_share(projects: Path, key: str) -> float:
    """The skill index's share of the session's system prompt bytes."""
    project, base = key.split("/", 1)
    path = projects / project / "sessions" / f"{base}.system-prompt.json"
    try:
        sections = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return 0.0
    if not isinstance(sections, list):
        return 0.0
    texts = [s for s in sections if isinstance(s, str)]
    total = sum(r31.utf8(s) for s in texts)
    skills = sum(r31.utf8(s) for s in texts if s.lstrip().startswith(SKILL_INDEX_HEADING))
    return skills / total if total else 0.0


def live_cleared(root, since_ms: int) -> dict:
    """(session key, message index) -> results' wire bytes as the transcript
    holds them now, for tool messages the harness cleared in place."""
    cleared = {}
    for key, ordered, _compactions, span in r31.zo_sessions(root, use_vault=False):
        if span[1] is None or span[1] < since_ms:
            continue
        for index, message, _stamp in ordered:
            if message.get("role") != "tool":
                continue
            size, hit = 0, False
            for block in message.get("blocks") or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    output = block.get("output") or ""
                    size += r31.utf8(r31.wire_view(block.get("tool_name") or "?", output))
                    hit = hit or any(marker in output for marker in r31.ZO_CLEARED_MARKERS)
            if hit:
                cleared[(key, index)] = size
    return cleared


class SessionBuilder:
    """One pass over a session's messages; everything textual is judged here."""

    def __init__(self, key, ordered, compactions, *, subagent, ledger, effort_pref, cleared, skill_share, window,
                 fallback_model=None):
        self.key = key
        self.fallback_model = fallback_model
        self.ordered = ordered
        self.subagent = subagent
        self.ledger = ledger
        self.cleared = cleared
        self.window = window
        self.session = Session(key=key, subagent=subagent, messages=[], requests=[], turns=[], calls=[],
                               injections=[], compactions=sorted(compactions), skill_share=skill_share,
                               effort_pref=effort_pref)
        self.assistant_words: list[tuple[int, set]] = []  # (message index, words said)
        self.person_words: dict[int, set] = defaultdict(set)
        self.result_bodies: dict[int, str] = {}  # call number -> body
        self.recall_texts: dict[int, str] = {}  # injection number -> text
        self.call_arguments: dict[int, dict] = {}

    def build(self) -> Session:
        session = self.session
        turn = Turn(index=0, kind="preamble", start=0)
        session.turns.append(turn)
        by_id: dict[str, Call] = {}
        for index, message, stamp in self.ordered:
            role = message.get("role") or "?"
            blocks = [b for b in message.get("blocks") or [] if isinstance(b, dict)]
            if role == "user":
                turn = self.open_turn(turn, index, blocks)
            msg = Message(index=index, role=role, turn=turn.index)
            carved_inline, carved_tool = self.account_injections(message, role, blocks, msg, turn)
            new_calls = []
            batch = sum(1 for b in blocks if b.get("type") == "tool_use")
            said = set()
            for block in blocks:
                msg.blocks += 1
                kind = block.get("type")
                if kind == "text" and role == "assistant":
                    text = block.get("text") or ""
                    self.add(msg, "conversation", r31.utf8(text), "text")
                    said |= words(text)
                elif kind == "text" and role == "user":
                    text = block.get("text") or ""
                    shape = user_text_shape(text)
                    own = max(0, r31.utf8(text) - carved_inline)
                    carved_inline = 0
                    if shape in ("person", "steering"):
                        self.add(msg, "user_input", own, "text")
                        self.person_words[turn.index] |= words(text)
                    else:
                        self.add(msg, "reminders", own, "text")
                    if text.lstrip().startswith(GATE_MARK):
                        note_gate(turn, index)
                elif kind == "text" and role == "tool":
                    self.add(msg, "reminders", r31.utf8(block.get("text") or ""), "text")
                elif kind == "thinking":
                    thought = block.get("thinking") or ""
                    self.add(msg, "conversation", r31.utf8(thought) + r31.utf8(block.get("signature")), "thinking")
                    said |= words(thought)
                elif kind == "tool_use":
                    name = block.get("name") or "?"
                    arguments = parse_input(block.get("input"))
                    self.add(msg, "conversation", r31.utf8(name) + r31.utf8(block.get("input")), "tool")
                    said |= words(json.dumps(arguments, ensure_ascii=False))
                    call = Call(number=len(session.calls), name=name, family=r31.tool_family(name),
                                kind=call_kind(name, arguments), request=len(session.requests), turn=turn.index,
                                batch=batch)
                    if call.kind in ("shell", "shell_read"):
                        call.check = bool(CHECK_COMMAND.search(str(arguments.get("command") or "")))
                    if call.kind in ("read", "edit"):
                        call.path = os.path.normpath(str(arguments.get("path") or ""))
                    self.call_arguments[call.number] = arguments
                    session.calls.append(call)
                    new_calls.append(call.number)
                    turn.calls.append(call.number)
                    by_id[block.get("id") or f"{index}:{call.number}"] = call
                elif kind == "tool_result":
                    self.account_result(block, msg, by_id, carved_tool)
                    carved_tool = 0
            if said:
                self.assistant_words.append((index, said))
            if (self.key, index) in self.cleared:
                msg.cleared = True
                msg.live_tool = self.cleared[(self.key, index)]
                for call in by_id.values():
                    if call.result_index == index:
                        call.cleared = True
            self.account_request(message, role, index, stamp, turn, new_calls)
            turn.end = index
            session.messages.append(msg)
        mark_breaks_and_contexts(session)
        self.judge_use()
        judge_duplicates(session, self.result_bodies, self.call_arguments)
        return session

    @staticmethod
    def add(msg: Message, kind: str, size: int, axis: str) -> None:
        if size <= 0:
            return
        msg.kinds[kind] = msg.kinds.get(kind, 0) + size
        setattr(msg, axis, getattr(msg, axis) + size)

    def open_turn(self, turn: Turn, index: int, blocks: list) -> Turn:
        opening_kind = None
        for block in blocks:
            if block.get("type") == "image":
                opening_kind = "person"
                break
            if block.get("type") == "text":
                shape = user_text_shape(block.get("text") or "")
                if shape == "person":
                    opening_kind = "person"
                elif shape == "opening":
                    opening_kind = "loop" if (block.get("text") or "").lstrip().startswith(LOOP_MARK) else "notification"
                break
        if opening_kind is None:
            return turn
        if turn.kind == "preamble" and not turn.requests:
            turn.kind, turn.start = opening_kind, index
            return turn
        fresh = Turn(index=len(self.session.turns), kind=opening_kind, start=index)
        self.session.turns.append(fresh)
        return fresh

    def account_injections(self, message: dict, role: str, blocks: list, msg: Message, turn: Turn) -> tuple[int, int]:
        """Charge a message's injections to their kinds; return the bytes carved out
        of its user text and of its tool results, so those are not charged twice."""
        injected = r33.zo_message_injections(message)
        system_texts = [b.get("text") or "" for b in blocks if b.get("type") == "text"] if role == "system" else []
        for number, item in enumerate(injected):
            kind = "recall" if item["kind"] in RECALL_KINDS else "reminders"
            self.add(msg, kind, item["bytes"], "text")
            record = Injection(index=msg.index, turn=turn.index, kind=item["kind"], family=item["family"],
                               bytes=item["bytes"])
            self.session.injections.append(record)
            if item["kind"] in RECALL_KINDS and item["carrier"] == "system-message" and number < len(system_texts):
                self.recall_texts[len(self.session.injections) - 1] = system_texts[number]
            if item["kind"] == GATE_KIND:
                note_gate(turn, msg.index)
        return (sum(i["bytes"] for i in injected if i["carrier"] == "inline-text"),
                sum(i["bytes"] for i in injected if i["carrier"] == "tool-result"))

    def account_result(self, block: dict, msg: Message, by_id: dict, carved: int) -> None:
        name = block.get("tool_name") or "?"
        output = block.get("output") or ""
        size = max(0, r31.utf8(r31.wire_view(name, output)) - carved)
        self.add(msg, f"tool:{r31.tool_family(name)}", size, "tool")
        call = by_id.get(block.get("tool_use_id"))
        if call is None:
            return
        call.result_index = msg.index
        call.result_bytes = size
        call.is_error = bool(block.get("is_error"))
        call.large = size >= LARGE_RESULT_BYTES
        if call.kind in ("shell", "shell_read") and call.check:
            interpretation = str(parse_input(output).get("returnCodeInterpretation") or "")
            call.failed_check = call.is_error or interpretation.startswith(FAILED_EXIT_PREFIX)
        mark = output.find(r40.DIGEST_MARK)
        if mark >= 0:
            match = re.search(r"read \d+ lines \((\d+) chars\)", output[mark:mark + 200])
            if match:
                call.digest_raw_chars = int(match.group(1))
        if call.kind == "delegate":
            call.agent_id = parse_input(output).get("agentId")
        self.result_bodies[call.number] = result_body(name, output)

    def account_request(self, message: dict, role: str, index: int, stamp, turn: Turn, new_calls: list) -> None:
        session = self.session
        usage = message.get("usage") if role == "assistant" else None
        values = None
        if usage:
            values = (int(usage.get("cache_read_input_tokens") or 0), int(usage.get("cache_creation_input_tokens") or 0),
                      int(usage.get("input_tokens") or 0), int(usage.get("output_tokens") or 0))
        if not values or values == (0, 0, 0, 0):
            for number in new_calls:
                session.calls[number].request = -1
            return
        details = usage.get("output_tokens_details")
        row = self.ledger.get(values)
        # A sub-agent transcript may not name its model; its request ledger row does.
        model = message.get("model") or (row or {}).get("model") or self.fallback_model or "?"
        since_ms, until_ms = self.window
        request = Request(index=index, model=model, family=provider_family(model), turn=turn.index,
                          read=values[0], write=values[1], uncached=values[2], output=values[3],
                          thinking=details.get("thinking_tokens") if isinstance(details, dict) else None,
                          in_window=stamp is None or since_ms <= stamp <= until_ms, calls=new_calls)
        if row:
            request.effort = row.get("effort")
            request.ttl = row.get("ttl")
            request.ledger_broke = bool(row.get("broke"))
        session.requests.append(request)
        turn.requests.append(len(session.requests) - 1)

    def judge_use(self) -> None:
        """DEFINITION 7 for every result, recall injection and chunk."""
        session = self.session
        mentions: dict[str, list[int]] = defaultdict(list)
        for index, said in self.assistant_words:
            for word in said:
                mentions[word].append(index)
        frequency: Counter = Counter()
        bodies = {number: words(body) for number, body in self.result_bodies.items()}
        for found in bodies.values():
            frequency.update(found)
        common: set = set()
        if len(bodies) >= COMMON_TOKEN_MIN_RESULTS:
            common = {w for w, n in frequency.items() if n > COMMON_TOKEN_SHARE * len(bodies)}

        def introduced(found: set, at: int, exclude: set) -> set:
            return {w for w in found - exclude - common if not mentions.get(w) or mentions[w][0] > at}

        def said_within(own: set, after: int, until: int) -> bool:
            return any(mentions.get(w) and after < mentions[w][0] <= until for w in own)

        turn_end = {turn.index: turn.end for turn in session.turns}
        for call in session.calls:
            if call.result_index is None or call.number not in bodies:
                continue
            arguments = words(json.dumps(self.call_arguments.get(call.number, {}), ensure_ascii=False))
            own = introduced(bodies[call.number], call.result_index, arguments | self.person_words[call.turn])
            if len(own) < MIN_DISTINCT_TOKENS:
                continue
            call.judgeable = True
            end = turn_end.get(call.turn, call.result_index)
            call.used = said_within(own, call.result_index, end)
            call.last_mention = max((mentions[w][-1] for w in own if mentions.get(w)), default=-1)
            if call.large:
                lines = self.result_bodies[call.number].splitlines()
                chunks = [lines[i:i + CHUNK_LINES] for i in range(0, len(lines), CHUNK_LINES)] or [[]]
                kept = total = 0
                for chunk in chunks:
                    body = "\n".join(chunk)
                    size = r31.utf8(body)
                    total += size
                    if said_within(words(body) & own, call.result_index, end):
                        kept += size
                        call.chunks_needed += 1
                call.chunks = len(chunks)
                call.chunk_kept_share = kept / total if total else 1.0
        for number, text in self.recall_texts.items():
            injection = session.injections[number]
            own = introduced(words(text), injection.index, self.person_words[injection.turn])
            if len(own) >= MIN_DISTINCT_TOKENS:
                injection.used = said_within(own, injection.index, turn_end.get(injection.turn, injection.index))


def note_gate(turn: Turn, index: int) -> None:
    """One gate per message, however many carriers (the tag, a reminder span) name it."""
    if index not in turn.gates:
        turn.gates.append(index)


def mark_breaks_and_contexts(session: Session) -> None:
    """Breaks (DEFINITION 5) and each request's context start (DEFINITION 4)."""
    pending = list(session.compactions)
    start = 0
    caching = {r.model for r in session.requests if r.read > 0}
    for number, request in enumerate(session.requests):
        if number:
            previous = session.requests[number - 1]
            request.brk = request.model in caching and request.read < BREAK_READ_SHARE * previous.total_in
            while pending and pending[0] < request.index and request.total_in < COMPACTION_DROP_SHARE * previous.total_in:
                start = pending.pop(0)
        request.ctx_start = start


def coarse_key(call: Call, arguments: dict) -> str:
    """DEFINITION 6's coarse key: what a call asks for, whatever the window or output options."""
    if call.kind == "read":
        return f"{call.name}:{call.path}"
    if call.kind in ("search", "list"):
        return f"{call.name}:{arguments.get('pattern')}:{arguments.get('path')}:{arguments.get('glob')}:{arguments.get('type')}"
    return f"{call.name}:{arguments.get('command')}"


def judge_duplicates(session: Session, bodies: dict, arguments: dict) -> None:
    """DEFINITION 6: exact repeats (safe when the result is byte-identical),
    near repeats (safe when the result is redundant), and what a judgment-free
    rule would have blocked."""
    exact: dict[str, tuple[int, str]] = {}
    coarse: dict[str, int] = {}
    seen_lines: set = set()
    context = 0
    last_edit_of: dict[str, int] = {}
    last_edit = last_shell = -1
    for call in session.calls:
        start = session.requests[call.request].ctx_start if call.request >= 0 else context
        if start != context:
            exact, coarse, seen_lines, context = {}, {}, set(), start
        if call.kind == "edit":
            last_edit = call.number
            if call.path:
                last_edit_of[call.path] = call.number
        if call.kind in ("shell", "wait"):
            last_shell = call.number
        body = bodies.get(call.number)
        if call.kind not in READ_ONLY_KINDS or body is None:
            continue
        own_arguments = arguments.get(call.number, {})
        relevant = {k: v for k, v in own_arguments.items() if k not in UNKEYED_ARGUMENTS}
        key = call.name + ":" + json.dumps(relevant, ensure_ascii=False, sort_keys=True)
        printed = fingerprint(stable_result(body))
        lines = {line.strip() for line in body.splitlines() if len(line.strip()) >= MIN_LINE_CHARS}
        call.redundant = bool(lines) and len(lines & seen_lines) >= REDUNDANT_LINE_SHARE * len(lines)
        if key in exact:
            earlier, earlier_print = exact[key]
            call.duplicate = "safe" if earlier_print == printed else "changed"
        wide = coarse_key(call, own_arguments)
        if wide in coarse:
            earlier = coarse[wide]
            call.near = "safe" if (call.duplicate == "safe" or call.redundant) else "changed"
            edited = last_edit_of.get(call.path, -1) if call.kind == "read" else last_edit
            call.rule_would_block = edited < earlier and last_shell < earlier
        exact[key] = (call.number, printed)
        coarse[wide] = call.number
        seen_lines |= lines


def read_corpus(projects: Path, prompt_cache: Path, window) -> tuple[list[Session], dict, AgentLinks]:
    since_ms, until_ms = window
    ledgers = read_request_ledgers(prompt_cache, window)
    prefs = read_session_prefs(projects)
    links = read_agent_links(projects)
    agent_keys = links.keys
    sessions: list[Session] = []
    excluded: Counter = Counter()
    for sidecars, root in ((False, projects), (True, AgentRoot(projects))):
        cleared = live_cleared(root, since_ms)
        for key, ordered, compactions, span in r31.zo_sessions(root, use_vault=True):
            if span[1] is None or span[1] < since_ms or (span[0] is not None and span[0] > until_ms):
                continue
            project, base = key.split("/", 1)
            subagent = sidecars or key in agent_keys
            if not subagent and is_synthetic(project):
                excluded["synthetic_sessions"] += 1
                continue
            ledger_id = base[: -len(".session")] if base.endswith(".session") else base
            builder = SessionBuilder(
                key, ordered, compactions, subagent=subagent, ledger=ledgers.get(ledger_id, {}),
                effort_pref=prefs.get(base), cleared=cleared,
                skill_share=skill_index_share(projects, key), window=window,
                fallback_model=links.model_of.get(links.agent_of(key) or base.removesuffix(".session"))
                if subagent else None)
            session = builder.build()
            outside = sum(1 for r in session.requests if not r.in_window)
            excluded["requests_outside_window"] += outside
            if len(session.requests) > outside:
                sessions.append(session)
    return sessions, dict(excluded), links


# ---------------------------------------------------------------------------
# Calibration and attribution (DEFINITION 4)
# ---------------------------------------------------------------------------


def solve(rows: list[list[float]], ys: list[float]) -> list[float]:
    """Least squares with non-negative coefficients (drop the negative, refit)."""
    width = len(rows[0])
    active = list(range(width))
    while active:
        size = len(active)
        matrix = [[0.0] * (size + 1) for _ in range(size)]
        for row, y in zip(rows, ys):
            for a in range(size):
                xa = row[active[a]]
                if not xa:
                    continue
                for b in range(size):
                    matrix[a][b] += xa * row[active[b]]
                matrix[a][size] += xa * y
        for column in range(size):
            pivot = max(range(column, size), key=lambda r: abs(matrix[r][column]))
            matrix[column], matrix[pivot] = matrix[pivot], matrix[column]
            if abs(matrix[column][column]) < 1e-9:
                continue
            for r in range(size):
                if r != column and matrix[r][column]:
                    factor = matrix[r][column] / matrix[column][column]
                    for c in range(column, size + 1):
                        matrix[r][c] -= factor * matrix[column][c]
        solution = [matrix[a][size] / matrix[a][a] if abs(matrix[a][a]) > 1e-9 else 0.0 for a in range(size)]
        negative = {a for a, value in enumerate(solution) if value < 0}
        if not negative:
            coefficients = [0.0] * width
            for a, value in zip(active, solution):
                coefficients[a] = value
            return coefficients
        active = [feature for a, feature in enumerate(active) if a not in negative]
    return [0.0] * width


def gained_features(messages: list[Message]) -> list[float]:
    return [float(sum(m.text for m in messages)), float(sum(m.tool for m in messages)),
            float(sum(m.thinking for m in messages)), float(sum(m.blocks for m in messages))]


def calibrate(sessions: list[Session]) -> dict:
    pairs: dict[str, list] = defaultdict(list)
    for session in sessions:
        indices = [m.index for m in session.messages]
        for number in range(1, len(session.requests)):
            previous, request = session.requests[number - 1], session.requests[number]
            if previous.model != request.model or request.ctx_start != previous.ctx_start:
                continue
            lo = bisect.bisect_left(indices, previous.index)
            hi = bisect.bisect_left(indices, request.index)
            gained = session.messages[lo:hi]
            delta = request.total_in - previous.total_in
            if delta <= 0 or any(m.cleared for m in gained):
                continue
            pairs[request.family].append((gained_features(gained), float(delta)))
    fits = {}
    pooled = [pair for family_pairs in pairs.values() for pair in family_pairs]
    for family in FAMILIES:
        if len(pairs.get(family, [])) >= CALIBRATION_MIN_PAIRS:
            fits[family] = fit(pairs[family])
    fits["pooled"] = fit(pooled)
    return fits


def fit(data: list) -> dict:
    xs = [x for x, _ in data]
    ys = [y for _, y in data]
    beta = solve(xs, ys)
    residuals = [y - sum(b * v for b, v in zip(beta, x)) for x, y in zip(xs, ys)]
    center = statistics.median(residuals)
    spread = statistics.median(abs(r - center) for r in residuals) or 1.0
    kept = [(x, y) for (x, y), r in zip(data, residuals) if abs(r - center) <= OUTLIER_MADS * MAD_TO_SIGMA * spread]
    if len(kept) > len(beta):
        beta = solve([x for x, _ in kept], [y for _, y in kept])
    predicted = [sum(b * v for b, v in zip(beta, x)) for x, _ in kept]
    actual = [y for _, y in kept]
    mean = statistics.fmean(actual) if actual else 0.0
    total = sum((y - mean) ** 2 for y in actual) or 1.0
    error = sum((y - p) ** 2 for y, p in zip(actual, predicted))
    ape = [abs(y - p) / y for y, p in zip(actual, predicted) if y]
    return {"pairs": len(data), "kept": len(kept),
            "beta": dict(zip(CALIBRATION_FEATURES, (round(b, 4) for b in beta))),
            "r2": round(1 - error / total, 3), "median_ape": round(statistics.median(ape), 3) if ape else None}


def beta_for(fits: dict, family: str) -> dict:
    return fits.get(family, fits["pooled"])["beta"]


def message_tokens(message: Message, beta: dict) -> dict:
    """Calibrated tokens of one message as it is carried now, per report kind."""
    kinds = dict(message.kinds)
    if message.cleared and message.live_tool is not None:
        tool_kinds = [k for k in kinds if k.startswith("tool:")]
        original = sum(kinds[k] for k in tool_kinds)
        if original:
            for k in tool_kinds:
                kinds[k] = kinds[k] * message.live_tool / original
    out = {}
    for kind, size in kinds.items():
        if kind == "conversation":
            continue
        out[kind] = (beta["tool"] if kind.startswith("tool:") else beta["text"]) * size
    # The assistant's own blocks: prose, tool input and thinking at their own rates.
    conversation_tool = message.tool - sum(message.kinds.get(k, 0) for k in message.kinds if k.startswith("tool:"))
    conversation_text = message.text - sum(message.kinds.get(k, 0) for k in message.kinds
                                           if k in ("recall", "reminders", "user_input"))
    conversation = (beta["text"] * max(0, conversation_text) + beta["tool"] * max(0, conversation_tool)
                    + beta["thinking"] * message.thinking)
    if conversation:
        out["conversation"] = conversation
    blocks = beta["blocks"] * message.blocks
    if blocks and out:
        whole = sum(out.values())
        for kind in out:
            out[kind] += blocks * out[kind] / whole
    return out


def attribute(session: Session, fits: dict, tool_share: float | None) -> None:
    per_family: dict[str, list[dict]] = {}

    def tokens_of(family: str) -> list[dict]:
        if family not in per_family:
            beta = beta_for(fits, family)
            per_family[family] = [message_tokens(m, beta) for m in session.messages]
        return per_family[family]

    indices = [m.index for m in session.messages]
    first = session.requests[0]
    before = tokens_of(first.family)[: bisect.bisect_left(indices, first.index)]
    session.fixed = max(0.0, min(float(first.total_in), first.total_in - sum(sum(t.values()) for t in before)))
    tool_part = session.fixed * tool_share if tool_share is not None else 0.0
    skill_part = (session.fixed - tool_part) * session.skill_share
    fixed_kinds = {"fixed_tools": tool_part, "skill_index": skill_part,
                   "fixed_system": session.fixed - tool_part - skill_part}
    for request in session.requests:
        tokens = tokens_of(request.family)
        lo = bisect.bisect_left(indices, request.ctx_start)
        hi = bisect.bisect_left(indices, request.index)
        estimate: Counter = Counter()
        for item in tokens[lo:hi]:
            estimate.update(item)
        fixed = min(float(request.total_in), session.fixed)
        conversation = request.total_in - fixed
        guessed = sum(estimate.values())
        request.scale = conversation / guessed if guessed else None
        kinds = {kind: value * (request.scale or 0.0) for kind, value in estimate.items()}
        ratio = fixed / session.fixed if session.fixed else 0.0
        for kind, value in fixed_kinds.items():
            kinds[kind] = value * ratio
        request.kinds = kinds


# ---------------------------------------------------------------------------
# Carry and cost (DEFINITION 5)
# ---------------------------------------------------------------------------


@dataclass
class Cost:
    fresh: float = 0.0
    read: float = 0.0
    output: float = 0.0
    usd: float = 0.0

    def add(self, other: "Cost", times: float = 1.0) -> None:
        self.fresh += other.fresh * times
        self.read += other.read * times
        self.output += other.output * times
        self.usd += other.usd * times

    @property
    def tokens(self) -> float:
        return self.fresh + self.read + self.output

    def as_dict(self) -> dict:
        return {"fresh": round(self.fresh), "read": round(self.read), "output": round(self.output),
                "tokens": round(self.tokens), "usd": round(self.usd, 2)}


class Carry:
    """What carrying a block, or making a request, cost in one session."""

    def __init__(self, session: Session, prices: Prices):
        self.session = session
        self.prices = prices
        self.request_indices = [r.index for r in session.requests]
        self.breaks = [n for n, r in enumerate(session.requests) if r.brk]

    def priced(self, model: str, fresh: float, read: float, output: float) -> Cost:
        usd = self.prices.cost(model, fresh, read, output)
        return Cost(fresh=fresh, read=read, output=output, usd=usd or 0.0)

    def block(self, tokens: float, index: int, *, cleared: bool = False, within_turn: bool = False,
              from_request: int | None = None) -> Cost:
        """A block appended at message `index`, carried from `from_request` (default:
        the first request after it) until a compaction drops it or the session ends."""
        requests = self.session.requests
        start = bisect.bisect_right(self.request_indices, index) if from_request is None else from_request
        if start >= len(requests) or tokens <= 0:
            return Cost()
        end = len(requests) - 1
        for number in range(start + 1, len(requests)):
            if requests[number].ctx_start > index or (within_turn and requests[number].turn != requests[start].turn):
                end = number - 1
                break
        breaks = [b for b in self.breaks if start < b <= end]
        if cleared and breaks:
            end = breaks[0] - 1
            breaks = []
        carrying = end - start + 1
        fresh = tokens * (1 + len(breaks))
        read = tokens * max(0, carrying - 1 - len(breaks))
        return self.priced(requests[start].model, fresh, read, 0.0)

    def request(self, number: int) -> Cost:
        request = self.session.requests[number]
        return self.priced(request.model, request.write + request.uncached, request.read, request.output)


def result_tokens(session: Session, fits: dict, call: Call) -> float:
    family = session.requests[call.request].family if call.request >= 0 else "pooled"
    return beta_for(fits, family)["tool"] * call.result_bytes


# ---------------------------------------------------------------------------
# Baseline tables
# ---------------------------------------------------------------------------

REPORT_KINDS = ("fixed_system", "fixed_tools", "skill_index", "recall", "reminders", "user_input", "conversation")


def baseline(sessions: list[Session], fits: dict, prices: Prices, links: AgentLinks) -> dict:
    mains = [s for s in sessions if not s.subagent]
    agents = [s for s in sessions if s.subagent]
    requests = [(s, r) for s in mains for r in s.requests if r.in_window]
    turns = [(s, t) for s in mains for t in s.turns if any(s.requests[n].in_window for n in t.requests)]
    out: dict = {}

    families = Counter(r.family for _, r in requests)
    priced = sum(1 for _, r in requests if prices.row(r.model)[1])
    out["sample"] = {
        "sessions": len(mains), "subagent_sessions": len(agents), "turns": len(turns),
        "turn_kinds": dict(Counter(t.kind for _, t in turns)),
        "requests": len(requests), "subagent_requests": sum(len(a.requests) for a in agents),
        "families": dict(families.most_common()),
        "models": dict(Counter(r.model for _, r in requests).most_common()),
        "priced_request_share": round(share(priced, len(requests)) or 0.0, 3),
        "priced_input_share": round(share(sum(r.total_in for _, r in requests if prices.row(r.model)[1]),
                                          sum(r.total_in for _, r in requests)) or 0.0, 3),
        "compactions": sum(len(s.compactions) for s in mains),
        "effort_joined_requests": sum(1 for _, r in requests if r.effort),
    }

    kind_totals: Counter = Counter()
    for _, request in requests:
        kind_totals.update(request.kinds)
    input_total = sum(r.total_in for _, r in requests)
    tool_kinds = sorted((k for k in kind_totals if k.startswith("tool:")), key=lambda k: -kind_totals[k])
    per_turn = defaultdict(list)
    for session, turn in turns:
        sums: Counter = Counter()
        for number in turn.requests:
            sums.update(session.requests[number].kinds)
        for kind in list(REPORT_KINDS) + tool_kinds:
            per_turn[kind].append(sums.get(kind, 0.0))
    scales = [r.scale for _, r in requests if r.scale]
    exact = [r for _, r in requests if r.thinking is not None]
    out["tokens"] = {
        "input_total": input_total,
        "by_kind": {k: {"tokens": round(kind_totals[k]), "share": round(share(kind_totals[k], input_total) or 0.0, 4),
                        "per_turn_mean": round(statistics.fmean(per_turn[k])) if per_turn[k] else None}
                    for k in list(REPORT_KINDS) + tool_kinds},
        "tool_results_share": round(share(sum(kind_totals[k] for k in tool_kinds), input_total) or 0.0, 4),
        "per_turn_input": dist([sum(s.requests[n].total_in for n in t.requests) for s, t in turns]),
        "per_request_input": dist([r.total_in for _, r in requests]),
        "billing": {
            "uncached": sum(r.uncached for _, r in requests), "cache_read": sum(r.read for _, r in requests),
            "cache_write": sum(r.write for _, r in requests), "output": sum(r.output for _, r in requests),
            "breaks": sum(1 for _, r in requests if r.brk),
            "break_fresh_tokens": sum(r.uncached + r.write for _, r in requests if r.brk),
            "ledger_breaks": sum(1 for _, r in requests if r.ledger_broke),
        },
        "billing_by_family": {
            family: {"requests": sum(1 for _, r in requests if r.family == family),
                     "input": sum(r.total_in for _, r in requests if r.family == family),
                     "uncached": sum(r.uncached for _, r in requests if r.family == family),
                     "cache_read": sum(r.read for _, r in requests if r.family == family),
                     "cache_write": sum(r.write for _, r in requests if r.family == family),
                     "output": sum(r.output for _, r in requests if r.family == family),
                     "breaks": sum(1 for _, r in requests if r.family == family and r.brk)}
            for family in families
        },
        "thinking_exact": {"requests": len(exact), "thinking": sum(r.thinking or 0 for r in exact),
                           "output": sum(r.output for r in exact)},
        "attribution_scale": {"p10": quantile(scales, 0.1), "p50": quantile(scales, 0.5), "p90": quantile(scales, 0.9)},
        "fixed_prefix_per_session": {family: dist([s.fixed for s in mains if s.requests[0].family == family])
                                     for family in FAMILIES},
        "usd_priced": round(sum(Carry(s, prices).request(n).usd for s in mains for n, r in enumerate(s.requests)
                                if r.in_window), 2),
    }

    calls = [(s, c) for s in mains for c in s.calls if c.request < 0 or s.requests[c.request].in_window]
    read_only = [(s, c) for s, c in calls if c.kind in READ_ONLY_KINDS and c.result_index is not None]
    duplicates = [(s, c) for s, c in read_only if c.duplicate]
    safe = [(s, c) for s, c in duplicates if c.duplicate == "safe"]
    near = [(s, c) for s, c in read_only if c.near]
    near_safe = [(s, c) for s, c in near if c.near == "safe"]
    judged = [(s, c) for s, c in read_only if c.judgeable]
    unused = [(s, c) for s, c in judged if c.used is False]
    redundant = [(s, c) for s, c in read_only if c.redundant]
    edit_turns = [(s, t) for s, t in turns
                  if any(s.calls[i].kind == "edit" and not s.calls[i].is_error for i in t.calls)]
    before_calls, before_requests = [], []
    for session, turn in edit_turns:
        first_edit = next(i for i in turn.calls if session.calls[i].kind == "edit" and not session.calls[i].is_error)
        explore = [i for i in turn.calls if i < first_edit]
        before_calls.append(len(explore))
        before_requests.append(len({session.calls[i].request for i in explore}))
    large = [(s, c) for s, c in calls if c.large]
    digests = [(s, c) for s, c in calls if c.digest_raw_chars]

    def tokens_of(items):
        return round(sum(result_tokens(s, fits, c) for s, c in items))

    out["tools"] = {
        "calls": len(calls),
        "per_turn": dist([len(t.calls) for _, t in turns]),
        "per_session": dist([len(s.calls) for s in mains]),
        "by_family": dict(Counter(c.family for _, c in calls).most_common()),
        "by_kind": dict(Counter(c.kind for _, c in calls).most_common()),
        "errors": sum(1 for _, c in calls if c.is_error),
        "batch_width": dict(sorted(Counter(c.batch for _, c in calls).items())),
        "read_only": len(read_only), "read_only_tokens": tokens_of(read_only),
        "exact_repeats": len(duplicates), "exact_safe": len(safe), "exact_changed": len(duplicates) - len(safe),
        "exact_safe_tokens": tokens_of(safe),
        "near_repeats": len(near), "near_safe": len(near_safe), "near_changed": len(near) - len(near_safe),
        "near_safe_tokens": tokens_of(near_safe),
        "near_by_kind": dict(Counter(f"{c.kind}:{c.near}" for _, c in near).most_common()),
        "rule_blocks_near_safe": sum(1 for _, c in near_safe if c.rule_would_block),
        "rule_blocks_near_changed": sum(1 for _, c in near if c.near == "changed" and c.rule_would_block),
        "redundant": len(redundant), "redundant_tokens": tokens_of(redundant),
        "judged_for_use": len(judged), "unused_in_turn": len(unused), "unused_tokens": tokens_of(unused),
        "unused_by_kind": dict(Counter(c.kind for _, c in unused).most_common()),
        "judged_by_kind": dict(Counter(c.kind for _, c in judged).most_common()),
        "edit_turns": len(edit_turns),
        "calls_before_first_edit": dist(before_calls),
        "requests_before_first_edit": dist(before_requests),
        "cleared_in_place": sum(1 for _, c in calls if c.cleared),
        "large_results": len(large), "large_result_tokens": tokens_of(large),
        "digests": len(digests), "digest_raw_chars": sum(c.digest_raw_chars for _, c in digests),
        "digest_delivered_bytes": sum(c.result_bytes for _, c in digests),
    }

    tool_free = [(s, t) for s, t in turns if not t.calls]
    out["turns"] = {
        "turns": len(turns),
        "tool_free": len(tool_free), "tool_free_share": round(share(len(tool_free), len(turns)) or 0.0, 3),
        "tool_free_by_kind": dict(Counter(t.kind for _, t in tool_free)),
        "requests_per_turn": dist([len(t.requests) for _, t in turns]),
        "calls_per_turn_with_tools": dist([len(t.calls) for _, t in turns if t.calls]),
        "tool_free_input_share": round(share(sum(s.requests[n].total_in for s, t in tool_free for n in t.requests),
                                             input_total) or 0.0, 4),
        "gates": sum(len(t.gates) for _, t in turns),
    }

    agent_keys = {a.key for a in agents}
    spawns = [(s, c) for s, c in calls if c.kind == "delegate"]
    out["subagents"] = {
        "spawns": len(spawns), "by_tool": dict(Counter(c.name for _, c in spawns)),
        "linked_transcripts": sum(1 for _, c in spawns if links.key_of.get(c.agent_id or "") in agent_keys),
        "transcripts": len(agents),
        "requests_each": dist([len(a.requests) for a in agents]),
        "input_tokens_each": dist([sum(r.total_in for r in a.requests) for a in agents]),
        "output_tokens_each": dist([sum(r.output for r in a.requests) for a in agents]),
        "tool_calls_each": dist([len(a.calls) for a in agents]),
        "input_tokens_total": sum(r.total_in for a in agents for r in a.requests),
        "relay_tokens_each": dist([result_tokens(s, fits, c) for s, c in spawns]),
        "families": dict(Counter(r.family for a in agents for r in a.requests)),
    }

    by_effort = defaultdict(list)
    for session, request in requests + [(a, r) for a in agents for r in a.requests]:
        effort = request.effort or (f"pref:{session.effort_pref}" if session.effort_pref else "unknown")
        by_effort[(request.family, effort)].append(request)
    out["effort"] = {
        f"{family}/{effort}": {
            "requests": len(rows),
            "thinking": dist([r.thinking for r in rows if r.thinking is not None]),
            "thinking_share_of_output": round(share(sum(r.thinking or 0 for r in rows if r.thinking is not None),
                                                    sum(r.output for r in rows if r.thinking is not None)) or 0.0, 3),
            "output": dist([r.output for r in rows]),
        }
        for (family, effort), rows in sorted(by_effort.items(), key=lambda item: -len(item[1]))
    }
    return out


# ---------------------------------------------------------------------------
# Jev ledgers (the shared reader) and request timings
# ---------------------------------------------------------------------------

JEV_LEDGERS = ("decision-shadow.jsonl", "rerank-shadow.jsonl")


def jev_ledgers(projects: Path, window) -> dict:
    since_ms, until_ms = window
    out = {}
    for name in JEV_LEDGERS:
        rows = []
        for path in sorted(projects.glob(f"*/state/smart-router/{name}")):
            rows.extend(row for row in shadow.read_rows(path) if since_ms <= int(row.get("at") or 0) <= until_ms)
        summary = shadow.summarise(rows)
        answered = [r for r in rows if r.get("outcome") == shadow.ANSWERED]
        summary["tokens_per_candidate"] = dist([
            int(shadow.field(r, "input_tokens")) / int(r["candidates"]) for r in answered
            if shadow.field(r, "input_tokens") and r.get("candidates")])
        summary["axes_per_row"] = dist([len(r["jev"]) for r in answered if isinstance(r.get("jev"), dict) and r["jev"]])
        summary.pop("axes", None)
        out[name.split(".")[0]] = summary
    return out


def request_timings(projects: Path, window) -> dict:
    since_ms, until_ms = window
    wall = defaultdict(list)
    for path in projects.glob("*/state/request-timings/timings.jsonl"):
        if is_synthetic(path.parts[-4]):
            continue
        for row in r31.read_jsonl(path):
            recorded = int(row.get("recorded_at") or 0)
            # `recorded_at` is Unix seconds; a millisecond stamp passes through.
            stamp = recorded * 1000 if recorded < SECONDS_STAMP_CEILING else recorded
            if not since_ms <= stamp <= until_ms:
                continue
            total = int(row.get("ttfb_ms") or 0) + int(row.get("stream_ms") or 0)
            if total > 0:
                wall[provider_family(row.get("model"))].append(total)
    return {family: dist(values) for family, values in sorted(wall.items())}


# ---------------------------------------------------------------------------
# Seats (DEFINITION 8)
# ---------------------------------------------------------------------------


class Claims:
    """Removable items of one seat, keyed so seats can be unioned item by item."""

    def __init__(self):
        self.items: dict[str, tuple[float, Cost]] = {}

    def claim(self, key: str, value: Cost, fraction: float = 1.0) -> None:
        held = self.items.get(key)
        if held is None or fraction * value.tokens > held[0] * held[1].tokens:
            self.items[key] = (fraction, value)

    def total(self) -> Cost:
        cost = Cost()
        for fraction, value in self.items.values():
            cost.add(value, fraction)
        return cost


def union(parts: list[Claims]) -> Claims:
    merged = Claims()
    for part in parts:
        for key, (fraction, value) in part.items.items():
            merged.claim(key, value, fraction)
    return merged


def removed_requests(claims: Claims, by_key: dict, timings: dict) -> dict:
    """Whole requests the claims remove, and their wall time at the family's median."""
    count, wall_ms = 0, 0
    for key in claims.items:
        parts = key.rsplit(":", 2)
        if len(parts) != 3 or parts[1] != "request":
            continue
        session_key, _, number = parts
        count += 1
        family = by_key[session_key].requests[int(number)].family
        wall_ms += (timings.get(family) or {}).get("p50") or 0
    return {"requests": count, "wall_s_at_family_p50": round(wall_ms / 1000)}


def net_at(perfect: Cost, misses: list[tuple[Cost, int]], hit_rate: float) -> Cost:
    """DEFINITION 8: h × PERFECT less each negative's miss, paid with probability 1 - h^n."""
    net = Cost()
    net.add(perfect, hit_rate)
    for cost, chances in misses:
        net.add(cost, -(1 - hit_rate ** chances))
    return net


def turn_in_window(session: Session, turn: Turn) -> bool:
    return any(session.requests[n].in_window for n in turn.requests)


def call_in_window(session: Session, call: Call) -> bool:
    return call.request < 0 or session.requests[call.request].in_window


def median_cost(costs: list[Cost]) -> Cost:
    if not costs:
        return Cost()
    return sorted(costs, key=lambda c: c.tokens)[len(costs) // 2]


def seats(sessions: list[Session], fits: dict, prices: Prices, jev: dict, links: AgentLinks, timings: dict,
          args) -> tuple[dict, dict]:
    mains = [s for s in sessions if not s.subagent]
    by_key = {s.key: s for s in sessions}
    carries = {s.key: Carry(s, prices) for s in sessions}
    jev_price = prices.jev_input()
    routing, rerank = jev["decision-shadow"], jev["rerank-shadow"]
    per_axis = (routing["input_tokens"]["mean"] or 0) / (routing["axes_per_row"]["mean"] or 1)
    per_candidate = rerank["tokens_per_candidate"]["mean"] or 0

    median_request: dict[str, Cost] = {}
    median_turn: dict[str, Cost] = {}
    for session in mains:
        carry = carries[session.key]
        costs = [carry.request(n) for n in range(len(session.requests))]
        median_request[session.key] = median_cost(costs)
        turn_costs = []
        for turn in session.turns:
            if turn.requests:
                total = Cost()
                for number in turn.requests:
                    total.add(costs[number])
                turn_costs.append(total)
        median_turn[session.key] = median_cost(turn_costs)

    def jev_cost(decisions: int, tokens_each: float, ledger: dict) -> dict:
        tokens = decisions * tokens_each
        return {"decisions": decisions, "tokens_each": round(tokens_each), "input_tokens": round(tokens),
                "usd": round(tokens * jev_price / Prices.MILLION, 4),
                "latency_ms_each": {"p50": ledger["latency_ms"]["p50"], "p95": ledger["latency_ms"]["p95"]}}

    def seat(perfect: Claims, misses: list[tuple[Cost, int]], jev_row: dict, counts: dict) -> dict:
        total = perfect.total()
        every_miss = Cost()
        for cost, _ in misses:
            every_miss.add(cost)
        rows = {}
        for h in args.hit_rates:
            net = net_at(total, misses, h)
            rows[f"{round(100 * h)}%"] = {**net.as_dict(), "usd_after_jev": round(net.usd - jev_row["usd"], 2)}
        return {"perfect": total.as_dict(), "removed": removed_requests(perfect, by_key, timings), "negatives": len(misses),
                "miss_if_every_negative_hit": every_miss.as_dict(), "jev": jev_row, "counts": counts,
                "at_hit_rate": rows}

    out = {}
    claims: dict[str, Claims] = {}

    # A — one turn plan: recall need, working state for a tool-free turn, effort.
    a = Claims()
    counts: Counter = Counter()
    misses: list = []
    thinking_by = defaultdict(list)
    for session in sessions:
        for request in session.requests:
            if request.thinking is not None and request.effort:
                thinking_by[(request.model, request.effort)].append(request.thinking)
    think_p50 = {key: quantile(values, 0.5) or 0 for key, values in thinking_by.items()}
    for session in mains:
        carry = carries[session.key]
        text_rate = beta_for(fits, session.requests[0].family)["text"]
        for number, injection in enumerate(session.injections):
            turn = session.turns[injection.turn]
            if not turn_in_window(session, turn):
                continue
            if injection.kind in RECALL_KINDS:
                verdict = "unjudged" if injection.used is None else ("used" if injection.used else "unused")
                counts[f"recall_{verdict}"] += 1
                if injection.used is False:
                    a.claim(f"{session.key}:inject:{number}", carry.block(text_rate * injection.bytes, injection.index))
                elif injection.used:
                    misses.append((median_request[session.key], 1))
            elif injection.family == WORKING_STATE_FAMILY and turn.requests and not turn.calls:
                counts["working_state_in_tool_free_turns"] += 1
                a.claim(f"{session.key}:inject:{number}", carry.block(text_rate * injection.bytes, injection.index))
        for turn in session.turns:
            if not turn_in_window(session, turn):
                continue
            counts["turns"] += 1
            if turn.calls:
                misses.append((median_request[session.key], 1))
                continue
            counts["tool_free_turns"] += 1
            for number in turn.requests:
                request = session.requests[number]
                levels = {e: v for (m, e), v in think_p50.items() if m == request.model and v}
                if not request.thinking or request.effort not in levels:
                    continue
                lowest = min(levels.values())
                saved = request.thinking * max(0.0, 1 - lowest / levels[request.effort])
                if saved:
                    counts["tool_free_requests_above_lowest_effort"] += 1
                    a.claim(f"{session.key}:thinking:{number}", carry.priced(request.model, 0.0, 0.0, saved))
    think_table = {f"{m}/{e}": v for (m, e), v in sorted(think_p50.items())}
    ceiling = 0.0
    for session in sessions:
        for request in session.requests:
            levels = {e: v for (m, e), v in think_p50.items() if m == request.model and v}
            if request.thinking and request.effort in levels:
                ceiling += request.thinking * max(0.0, 1 - min(levels.values()) / levels[request.effort])
    counts["thinking_saved_if_every_request_ran_lowest_effort"] = round(ceiling)
    counts["thinking_with_effort_known"] = sum(v for values in thinking_by.values() for v in values)
    out["A_turn_plan"] = seat(a, misses, jev_cost(counts["turns"], per_axis * args.turn_plan_axes, routing),
                              {**counts, "thinking_p50_by_model_effort": think_table})
    claims["A"] = a

    # B — which files to read, injected before the turn explores.
    b, b_files = Claims(), Claims()
    counts, misses = Counter(), []
    for session in mains:
        carry = carries[session.key]
        for turn in session.turns:
            edits = [i for i in turn.calls if session.calls[i].kind == "edit" and not session.calls[i].is_error]
            if not edits or not turn_in_window(session, turn):
                continue
            counts["edit_turns"] += 1
            explore = [session.calls[i] for i in turn.calls if i < edits[0]]
            for number in sorted({c.request for c in explore if c.request >= 0}):
                kinds = {session.calls[i].kind for i in session.requests[number].calls}
                if kinds and kinds <= set(READ_ONLY_KINDS):
                    counts["read_only_requests_before_edit"] += 1
                    b.claim(f"{session.key}:request:{number}", carry.request(number))
                    if kinds == {"read"}:
                        counts["file_read_requests_before_edit"] += 1
                        b_files.claim(f"{session.key}:request:{number}", carry.request(number))
            reads = []
            for call in explore:
                if call.kind in READ_ONLY_KINDS and call.result_index is not None:
                    tokens = result_tokens(session, fits, call)
                    reads.append(tokens)
                    if call.used is False:
                        counts["unused_exploration_results"] += 1
                        value = carry.block(tokens, call.result_index, cleared=call.cleared)
                        b.claim(f"{session.key}:result:{call.number}", value)
                        if call.kind == "read":
                            b_files.claim(f"{session.key}:result:{call.number}", value)
            wasted = statistics.median(reads) * args.pre_inject_files if reads else 0.0
            misses.append((carry.block(wasted, turn.start, within_turn=True), 1))
    row = seat(b, misses, jev_cost(counts["edit_turns"], args.file_candidates * per_candidate, rerank), dict(counts))
    row["perfect_file_reads_only"] = b_files.total().as_dict()
    out["B_file_pick"] = row
    claims["B"] = b

    # C — which chunks of a long result to keep.
    c_claims = Claims()
    counts, misses, kept_shares, per_result_misses = Counter(), [], [], []
    for session in mains:
        carry = carries[session.key]
        for call in session.calls:
            if not call.large or call.result_index is None or not call_in_window(session, call):
                continue
            counts["large_results"] += 1
            if call.chunk_kept_share is None:
                continue
            counts["judged"] += 1
            counts["chunks"] += call.chunks
            kept_shares.append(call.chunk_kept_share)
            value = carry.block(result_tokens(session, fits, call), call.result_index, cleared=call.cleared)
            c_claims.claim(f"{session.key}:result:{call.number}", value, 1 - call.chunk_kept_share)
            # Every chunk it needed is a chance to drop one (DEFINITION 8's 1 - h^n).
            misses.append((median_request[session.key], max(1, call.chunks_needed)))
            per_result_misses.append((median_request[session.key], 1))
    row = seat(c_claims, misses, jev_cost(counts["chunks"], per_candidate, rerank),
               {**counts, "kept_share": dist(kept_shares)})
    # The optimistic reading: one chance per result, as if chunk errors moved together.
    row["at_hit_rate_one_chance_per_result"] = seat(c_claims, per_result_misses, row["jev"], {})["at_hit_rate"]
    out["C_result_chunks"] = row
    claims["C"] = c_claims

    # D — what to evict where the prefix is rewritten anyway: at compaction (the
    # brief's seat) and, as a wider variant, at every break.
    for name, pick in (("D_evict_at_compaction", "compaction"), ("D_evict_at_breaks", "break")):
        d = Claims()
        counts, misses = Counter(), []
        for session in mains:
            carry = carries[session.key]
            points = []
            for number, request in enumerate(session.requests):
                compaction = number > 0 and request.ctx_start != session.requests[number - 1].ctx_start
                if not request.in_window:
                    continue
                if (pick == "compaction" and compaction) or (pick == "break" and request.brk and not compaction):
                    points.append(number)
            counts["rewrite_points"] += len(points)
            needed_at: Counter = Counter()
            for point in points:
                request = session.requests[point]
                for call in session.calls:
                    if call.result_index is None or not call.judgeable:
                        continue
                    if not request.ctx_start <= call.result_index < request.index:
                        continue
                    counts["decisions"] += 1
                    if call.last_mention >= request.index:
                        needed_at[call.number] += 1
                        continue
                    counts["stale_decisions"] += 1
                    if call.cleared:
                        counts["stale_already_cleared"] += 1
                        continue
                    value = carry.block(result_tokens(session, fits, call), call.result_index, from_request=point)
                    d.claim(f"{session.key}:result:{call.number}", value)
            counts["blocks_needed_later"] += len(needed_at)
            for number, chances in needed_at.items():
                misses.append((median_request[session.key], chances))
        out[name] = seat(d, misses, jev_cost(counts["decisions"], per_candidate, rerank), dict(counts))
        claims[name[0] if pick == "compaction" else "D+"] = d

    # E — repeated calls: exact, and near (same file or search, other window).
    e = Claims()
    counts, misses = Counter(), []
    for session in mains:
        carry = carries[session.key]
        safe_by_request: dict[int, int] = Counter()
        for call in session.calls:
            if not call_in_window(session, call):
                continue
            if call.duplicate:
                counts[f"exact_{call.duplicate}"] += 1
            if not call.near or call.result_index is None:
                continue
            counts[f"near_{call.near}"] += 1
            counts[f"rule_blocks_near_{call.near}"] += call.rule_would_block
            if call.near == "changed":
                misses.append((median_request[session.key], 1))
                continue
            e.claim(f"{session.key}:result:{call.number}",
                    carry.block(result_tokens(session, fits, call), call.result_index, cleared=call.cleared))
            if call.request >= 0:
                safe_by_request[call.request] += 1
        for number, safe in safe_by_request.items():
            if safe == len(session.requests[number].calls):
                counts["requests_made_only_of_safe_repeats"] += 1
                e.claim(f"{session.key}:request:{number}", carry.request(number))
    decisions = counts["near_safe"] + counts["near_changed"]
    out["E_repeat_calls"] = seat(e, misses, jev_cost(decisions, per_candidate, rerank), dict(counts))
    claims["E"] = e

    # F — whether to spawn.
    f = Claims()
    counts, misses = Counter(), []
    for session in mains:
        for call in session.calls:
            if call.kind != "delegate" or not call_in_window(session, call):
                continue
            counts["spawns"] += 1
            agent = by_key.get(links.key_of.get(call.agent_id or "", ""))
            if agent is None:
                continue
            counts["linked"] += 1
            carry = carries[agent.key]
            cost = Cost()
            for number in range(len(agent.requests)):
                cost.add(carry.request(number))
            if len(agent.calls) <= SMALL_SUBAGENT_CALLS:
                counts["small"] += 1
                # Inline, the parent would still have spent about one request on it.
                inline = Cost()
                inline.add(cost)
                inline.add(median_request[session.key], -1)
                if inline.tokens > 0:
                    f.claim(f"{agent.key}:subagent", inline)
            else:
                misses.append((cost, 1))
    out["F_spawn"] = seat(f, misses, jev_cost(counts["spawns"], per_candidate, rerank), dict(counts))
    claims["F"] = f

    # G — whether a completion claim needs re-checking.
    g = Claims()
    counts, misses = Counter(), []
    for session in mains:
        carry = carries[session.key]
        for turn in session.turns:
            if not turn_in_window(session, turn):
                continue
            for gate in turn.gates:
                counts["gates"] += 1
                follow = [session.calls[i] for i in turn.calls
                          if session.calls[i].result_index is not None and session.calls[i].result_index > gate]
                if any(c.kind == "edit" or c.failed_check for c in follow):
                    counts["needed"] += 1
                    misses.append((median_turn[session.key], 1))
                    continue
                for number in turn.requests:
                    if session.requests[number].index > gate:
                        g.claim(f"{session.key}:request:{number}", carry.request(number))
    out["G_recheck_claim"] = seat(g, misses, jev_cost(counts["gates"], CLAIM_EVIDENCE_CANDIDATES * per_candidate,
                                                      rerank), dict(counts))
    claims["G"] = g
    return out, claims


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------


def render(report: dict) -> str:
    lines = [f"window {report['window']['since']} → {report['window']['until']}  excluded {report['excluded']}"]
    for section in ("sample", "prompt_input", "calibration"):
        lines.append(f"== {section}")
        lines.append(f"  {json.dumps(report[section], ensure_ascii=False)}")
    tokens = report["tokens"]
    lines.append(f"== input tokens by kind (total {tokens['input_total']:,})")
    for kind, row in tokens["by_kind"].items():
        lines.append(f"  {kind:<22} {row['tokens']:>13,}  {row['share']:>7.2%}  per turn {row['per_turn_mean']}")
    for key in ("tool_results_share", "per_turn_input", "per_request_input", "billing", "billing_by_family",
                "thinking_exact", "attribution_scale", "fixed_prefix_per_session", "usd_priced"):
        lines.append(f"  {key}: {json.dumps(tokens[key], ensure_ascii=False)}")
    for section in ("tools", "turns", "subagents"):
        lines.append(f"== {section}")
        for key, value in report[section].items():
            lines.append(f"  {key}: {json.dumps(value, ensure_ascii=False)}")
    lines.append("== effort")
    for key, value in report["effort"].items():
        lines.append(f"  {key}: {json.dumps(value, ensure_ascii=False)}")
    lines.append("== Jev ledgers")
    for key, value in report["jev"].items():
        lines.append(f"  {key}: {json.dumps(value, ensure_ascii=False)}")
    lines.append(f"== request wall time (ttfb + stream, ms) {json.dumps(report['timings'])}")
    lines.append("== seats")
    for key, value in report["seats"].items():
        lines.append(f"  {key}: {json.dumps(value, ensure_ascii=False)}")
    lines.append(f"== assumptions {json.dumps(report['assumptions'])}")
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--projects", type=Path, default=Path.home() / ".zo" / "projects")
    parser.add_argument("--prompt-cache", type=Path, default=Path.home() / ".zo" / "cache" / "prompt-cache")
    parser.add_argument("--until", help="ISO local time the window ends (default: now)")
    parser.add_argument("--days", type=float, default=DEFAULT_DAYS)
    parser.add_argument("--prompt-input", type=Path,
                        help="a saved `zo --model <anthropic model> --prompt-input` report; its provider count "
                             "splits the tool block off the fixed prefix")
    parser.add_argument("--hit-rates", default=",".join(str(h) for h in DEFAULT_HIT_RATES))
    parser.add_argument("--file-candidates", type=int, default=DEFAULT_FILE_CANDIDATES)
    parser.add_argument("--turn-plan-axes", type=int, default=DEFAULT_TURN_PLAN_AXES)
    parser.add_argument("--pre-inject-files", type=int, default=DEFAULT_PRE_INJECT_FILES)
    parser.add_argument("--recommend", default="G,B,C", help="seats whose union the report prints")
    parser.add_argument("--json", type=Path, help="write the whole report here")
    args = parser.parse_args(argv)
    args.hit_rates = [float(h) for h in args.hit_rates.split(",")]

    until_ms = iso_to_ms(args.until) if args.until else int(time.time() * 1000)
    window = (until_ms - int(args.days * 24 * 3600 * 1000), until_ms)

    tool_share, prompt_input = None, None
    if args.prompt_input:
        text = args.prompt_input.read_text(encoding="utf-8")
        total = re.search(r"Provider count\s+([\d,]+) tokens", text)
        tools = re.search(r"([\d,]+) of that is the tool block", text)
        if total and tools:
            prompt_input = {"provider_count": int(total.group(1).replace(",", "")),
                            "tool_block": int(tools.group(1).replace(",", ""))}
            tool_share = prompt_input["tool_block"] / prompt_input["provider_count"]

    prices = Prices(PRICE_TABLE)
    started = time.time()
    sessions, excluded, links = read_corpus(args.projects, args.prompt_cache, window)
    fits = calibrate(sessions)
    for session in sessions:
        attribute(session, fits, tool_share)
    report = {"window": {"since": ms_to_iso(window[0]), "until": ms_to_iso(window[1]), "days": args.days},
              "excluded": excluded, "prompt_input": prompt_input, "calibration": fits}
    report.update(baseline(sessions, fits, prices, links))
    report["jev"] = jev_ledgers(args.projects, window)
    report["timings"] = request_timings(args.projects, window)
    seat_rows, claims = seats(sessions, fits, prices, report["jev"], links, report["timings"], args)
    by_key = {session.key: session for session in sessions}
    recommended = [s.strip() for s in args.recommend.split(",") if s.strip() in claims]
    merged = union([claims[s] for s in recommended])
    seat_rows["union_recommended"] = {"seats": recommended, **merged.total().as_dict(),
                                      "removed": removed_requests(merged, by_key, report["timings"])}
    everything = union(list(claims.values()))
    seat_rows["union_all"] = {**everything.total().as_dict(),
                              "removed": removed_requests(everything, by_key, report["timings"])}
    report["seats"] = seat_rows
    report["assumptions"] = {"file_candidates": args.file_candidates, "turn_plan_axes": args.turn_plan_axes,
                             "pre_inject_files": args.pre_inject_files, "hit_rates": args.hit_rates,
                             "elapsed_s": round(time.time() - started, 1)}
    print(render(report))
    if args.json:
        args.json.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
