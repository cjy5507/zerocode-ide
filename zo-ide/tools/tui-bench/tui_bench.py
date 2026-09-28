#!/usr/bin/env python3
"""tui-bench — one scripted conversation through zo and the other agent CLIs, measured from outside.

Every CLI gets the same thing: a 120x40 pty, a hermetic home, and a local model
service that streams the same markdown reply at the same delta rate. The driver
answers the terminal's queries (cursor position, colours, device attributes)
the way a terminal would, types into the composer while a reply streams, and
reads the process tree's CPU time, wakeups and memory from the kernel
(`proc_pid_rusage`), not from a `%CPU` snapshot.

    python3 zo-ide/tools/tui-bench/tui_bench.py --zo target/release/zo --runs 5
    python3 zo-ide/tools/tui-bench/tui_bench.py --clis zo --zo ./zo-a --zo-label before \\
        --zo-extra ./zo-b:after --runs 5

States, in the order one run passes through them:

  idle          5 s at the prompt after the first paint settles
  waiting       the model holds its first byte for 3 s (spinner, timer)
  streaming     a long markdown reply (headings, lists, code, a table) at
                `--delta-ms` per `--delta-chars`; one keystroke every 250 ms,
                each a letter that appears nowhere else, timed from the write
                to the pty until the CLI paints it (keystroke-to-echo)
  tool          a shell command that prints 120 lines over 3 s
  warm-up       `--turns` more turns, each a tool call and an answer, so the
                transcript is long ("after N turns")
  streaming@N   the same long reply and typing again, on the long transcript
  idle@N        5 s at the prompt again

What is measured per state: CPU time of the CLI's own processes (user+sys; a
shell a tool ran and everything under it is the tool's, the same for every CLI,
and is counted apart), wakeups (idle + interrupt), bytes the CLI wrote to the
pty, write bursts (a frame, from outside), DEC 2026 frames, resident memory;
per run: cold start to first byte and to the ready composer, and the resident
memory after each warm-up turn. `--frames` adds zo's own draw times by phase
(`ZO_PROBE_FRAMES`); `--sample N` runs `sample` on each state for attribution
(`attribute.py` reads those files) and makes that run's timings meaningless.

zo's agent store is seeded with `--seed-agents` finished helpers of other
sessions (432 by default — the person's zerocode project store on 2026-09-28):
zo's roster watcher reads that store, and an empty one hides its cost. Codex
runs with `--no-daemon`, so its app-server lives inside the measured tree.

Requires macOS (libproc) and `pyte` (`python3 -m pip install --user pyte`).
Nothing here reads or writes the person's own homes: each CLI runs with HOME,
its config directory and TMPDIR inside a fresh temporary directory, with an
environment built from nothing (no TMUX, no ZEROCODE_*, no API keys but a
dummy one for the local service, which refuses any other credential and stops
the benchmark if one arrives). Only processes the benchmark started are ever
signalled: each CLI leads its own session, and its group is what is ended.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import pty
import random
import re
import select
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import fcntl
import threading
import time
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Callable

ROWS, COLS = 40, 120
DUMMY_KEY = "sk-ant-dummy-0000000000000000000000000000000000000000"
BENCH_PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

# ---------------------------------------------------------------------------
# The scripted replies — the same bytes for every CLI.
# ---------------------------------------------------------------------------

END_LONG = "zqx-end-long"
END_WAIT = "zqx-end-wait"
END_TOOL = "zqx-end-tool"


def end_warm(turn: int) -> str:
    return f"zqx-end-warm-{turn}"


def long_reply() -> str:
    """About 7 KB of markdown, ASCII only (the typed letters must be unique)."""
    parts = [
        "# Streaming benchmark reply\n\n",
        "This reply is the same for every CLI. It is long on purpose, and it has "
        "the shapes an agent's answer usually has: headings, prose, lists, code "
        "blocks and a table, so the renderer does its ordinary work.\n\n",
        "## Plan\n\n",
    ]
    for step in range(1, 9):
        parts.append(
            f"{step}. Read module {step} of the workspace, note which functions it "
            f"exports, and check that the tests for step {step} still pass.\n"
        )
    parts.append("\n## Notes\n\n")
    for note in range(1, 11):
        parts.append(
            f"- **Note {note}**: the `config_{note}` value is read once at start; "
            f"a change to it needs a restart, and the *reload* path is not wired yet.\n"
        )
    parts.append("\n## Code\n\n```rust\n")
    for line in range(1, 21):
        parts.append(f"fn step_{line}(input: &str) -> usize {{ input.len() + {line} }}\n")
    parts.append("```\n\n```python\n")
    for line in range(1, 16):
        parts.append(f"def step_{line}(values):\n    return sum(values) + {line}\n")
    parts.append("```\n\n## Table\n\n| step | file | lines |\n|---|---|---|\n")
    for row in range(1, 7):
        parts.append(f"| {row} | src/module_{row}.rs | {row * 37} |\n")
    parts.append("\n## Details\n\n")
    for paragraph in range(1, 9):
        parts.append(
            f"Paragraph {paragraph} explains the change in plain words. The reader "
            "should be able to follow it without the code open, so each sentence "
            "says one thing: what moved, why it moved, and what was measured "
            "before and after the move. Nothing here is decorative.\n\n"
        )
    parts.append(f"{END_LONG}\n")
    return "".join(parts)


def warm_reply(turn: int) -> str:
    lines = [f"## Warm-up turn {turn}\n\n"]
    for item in range(1, 13):
        lines.append(f"- item {item} of turn {turn}: the command printed its lines and exited 0.\n")
    lines.append("\n```text\n")
    for line in range(1, 9):
        lines.append(f"line {line} of the tool output, kept for turn {turn}\n")
    lines.append("```\n\n")
    lines.append(f"{end_warm(turn)}\n")
    return "".join(lines)


WAIT_REPLY = f"The answer took three seconds to begin.\n\n{END_WAIT}\n"
TOOL_REPLY = f"The command finished and printed 120 lines.\n\n{END_TOOL}\n"
# No command substitution: Gemini CLI refuses `$(…)` as an injection.
TOOL_COMMAND = (
    "for i in {1..120}; do echo \"tool output line $i: the quick brown fox "
    "jumps over the lazy dog\"; sleep 0.025; done"
)


def warm_command(turn: int) -> str:
    return f"printf 'warm line %s of turn {turn}\\n' 1 2 3 4 5 6 7 8"


# ---------------------------------------------------------------------------
# The local model service (runs in its own process: `tui_bench.py serve`).
# ---------------------------------------------------------------------------


def mono_ns() -> int:
    return time.monotonic_ns()


class Plan:
    """What a request asks for: the newest `bench:` word any user message carries,
    and whether the conversation's last message is a tool's result."""

    def __init__(self, kind: str, turn: int = 0) -> None:
        self.kind = kind  # long | wait | tool | tool-done | warm | warm-done | other
        self.turn = turn


def plan_from_texts(texts: list[str], answered: Callable[[str], bool]) -> Plan:
    """The newest `bench:` word decides the turn; `answered(key)` says whether
    the conversation already carries the result of the tool call this service
    issued for that turn (then the turn's answer is due, not another call)."""
    joined = "\n".join(texts)
    match = None
    for match in re.finditer(r"bench:(long|wait|tool|warm)(?:\s+(\d+))?", joined):
        pass
    if match is None:
        return Plan("other")
    kind, turn = match.group(1), int(match.group(2) or 0)
    if kind in ("tool", "warm") and answered(f"{kind}:{turn}"):
        return Plan(f"{kind}-done", turn)
    return Plan(kind, turn)


class Service:
    def __init__(self, delta_ms: float, delta_chars: int, log_path: Path) -> None:
        self.delta_ms = delta_ms
        self.delta_chars = delta_chars
        self.log_path = log_path
        self.lock = threading.Lock()
        self.tool_ids = 0
        self.issued: dict[str, str] = {}

    def log(self, **event) -> None:
        event["t"] = mono_ns()
        line = json.dumps(event) + "\n"
        with self.lock:
            with open(self.log_path, "a", encoding="utf-8") as handle:
                handle.write(line)

    def issue(self, prefix: str, plan: Plan) -> str:
        """A tool call id, remembered with the turn it was issued for."""
        with self.lock:
            self.tool_ids += 1
            ident = f"{prefix}_{self.tool_ids}"
            self.issued[ident] = f"{plan.kind}:{plan.turn}"
            return ident

    def answered_by(self, result_ids: set[str]) -> Callable[[str], bool]:
        def answered(key: str) -> bool:
            with self.lock:
                return any(self.issued.get(ident) == key for ident in result_ids)
        return answered

    def chunks(self, text: str, fast: bool) -> list[str]:
        size = 64 if fast else self.delta_chars
        return [text[index:index + size] for index in range(0, len(text), size)]

    def pace(self, fast: bool) -> float:
        return 0.0 if fast else self.delta_ms / 1000.0


def all_user_texts(body: dict) -> list[str]:
    texts = []
    for message in body.get("messages") or []:
        if message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, str):
            texts.append(content)
            continue
        for block in content or []:
            if block.get("type") == "text":
                texts.append(block.get("text", ""))
    return texts


def pick_tool(names: list[str], preferred: list[str]) -> str | None:
    lowered = {name.lower(): name for name in names}
    for want in preferred:
        if want.lower() in lowered:
            return lowered[want.lower()]
    return None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    service: Service

    def log_message(self, *_args) -> None:  # quiet
        return

    # -- plumbing ----------------------------------------------------------

    def body_json(self) -> dict:
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            return json.loads(raw or b"{}")
        except json.JSONDecodeError:
            return {}

    def send_json(self, status: int, payload: dict) -> None:
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def start_sse(self) -> None:
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

    def chunk(self, data: bytes) -> None:
        self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
        self.wfile.flush()

    def end_chunks(self) -> None:
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def sse(self, event: str | None, payload: dict) -> None:
        head = f"event: {event}\n" if event else ""
        self.chunk(f"{head}data: {json.dumps(payload)}\n\n".encode())

    # -- routes ------------------------------------------------------------

    def do_GET(self) -> None:  # noqa: N802
        if "/models" in self.path:
            self.send_json(200, {"data": [], "models": [], "object": "list"})
            return
        self.send_json(404, {"error": {"message": "not here"}})

    def do_HEAD(self) -> None:  # noqa: N802
        self.send_response(200)
        self.send_header("content-length", "0")
        self.end_headers()

    def foreign_credential(self) -> bool:
        """Anything but the dummy key in an auth header. The value is never kept."""
        for name in ("x-api-key", "x-goog-api-key", "authorization"):
            value = self.headers.get(name)
            if value and DUMMY_KEY not in value:
                return True
        return False

    def do_POST(self) -> None:  # noqa: N802
        path = self.path.split("?", 1)[0]
        body = self.body_json()
        if self.foreign_credential():
            # A CLI found a login of the person's own: refuse, and let the
            # driver stop the whole benchmark (it reads this event).
            self.service.log(ev="foreign-credential", path=path)
            self.send_json(401, {"error": {"message": "only the benchmark's dummy key is accepted"}})
            return
        try:
            if path.endswith("/v1/messages/count_tokens"):
                self.send_json(200, {"input_tokens": 1000})
            elif path.endswith("/v1/messages"):
                self.anthropic(body)
            elif path.endswith("/responses"):
                self.openai_responses(body)
            elif ":streamGenerateContent" in path or ":generateContent" in path:
                self.gemini(body, streaming=":streamGenerateContent" in path)
            elif ":countTokens" in path:
                self.send_json(200, {"totalTokens": 1000})
            else:
                self.send_json(404, {"error": {"message": f"no route {path}"}})
        except (BrokenPipeError, ConnectionResetError):
            self.service.log(ev="client-gone", path=path)

    # -- Anthropic /v1/messages -------------------------------------------

    def anthropic(self, body: dict) -> None:
        svc = self.service
        results = {block.get("tool_use_id", "") for message in body.get("messages") or []
                   if isinstance(message.get("content"), list)
                   for block in message["content"] if block.get("type") == "tool_result"}
        tool_names = [tool.get("name", "") for tool in body.get("tools") or []]
        # A request with no tools is a side request (a title, a topic): it is
        # answered, never taken for the turn the driver is timing.
        plan = (plan_from_texts(all_user_texts(body), svc.answered_by(results))
                if tool_names else Plan("other"))
        tail = [(message.get("role"), [block.get("type") for block in message.get("content")]
                 if isinstance(message.get("content"), list) else "str")
                for message in (body.get("messages") or [])[-3:]]
        svc.log(ev="req", api="anthropic", kind=plan.kind, turn=plan.turn,
                stream=bool(body.get("stream")), model=body.get("model"), tools=len(tool_names),
                tail=tail)
        if plan.kind == "wait":
            time.sleep(3.0)
        text, tool = self.script(plan)
        tool_name = pick_tool(tool_names, ["bash", "Bash"]) if tool else None
        if tool and tool_name is None:
            text, tool = f"no shell tool was offered\n\n{END_TOOL}\n", None
        if not body.get("stream"):
            content = [{"type": "text", "text": text}] if text else []
            if tool:
                content.append({"type": "tool_use", "id": svc.issue("toolu_bench", plan),
                                "name": tool_name, "input": {"command": tool}})
            self.send_json(200, {
                "id": "msg_bench", "type": "message", "role": "assistant",
                "model": body.get("model") or "bench", "content": content,
                "stop_reason": "tool_use" if tool else "end_turn", "stop_sequence": None,
                "usage": {"input_tokens": 1000, "output_tokens": max(1, len(text) // 4)},
            })
            return
        fast = plan.kind in ("warm", "warm-done", "other", "tool", "wait")
        self.start_sse()
        self.sse("message_start", {"type": "message_start", "message": {
            "id": "msg_bench", "type": "message", "role": "assistant",
            "model": body.get("model") or "bench", "content": [], "stop_reason": None,
            "stop_sequence": None, "usage": {"input_tokens": 1000, "output_tokens": 1}}})
        index = 0
        if text:
            self.sse("content_block_start", {"type": "content_block_start", "index": 0,
                                             "content_block": {"type": "text", "text": ""}})
            deltas = svc.chunks(text, fast)
            pace = svc.pace(fast)
            svc.log(ev="first-delta", kind=plan.kind, deltas=len(deltas))
            for number, piece in enumerate(deltas):
                if number and pace:
                    time.sleep(pace)
                self.sse("content_block_delta", {"type": "content_block_delta", "index": 0,
                                                 "delta": {"type": "text_delta", "text": piece}})
            svc.log(ev="last-delta", kind=plan.kind, deltas=len(deltas))
            self.sse("content_block_stop", {"type": "content_block_stop", "index": 0})
            index = 1
        if tool:
            ident = svc.issue("toolu_bench", plan)
            self.sse("content_block_start", {"type": "content_block_start", "index": index,
                                             "content_block": {"type": "tool_use", "id": ident,
                                                               "name": tool_name, "input": {}}})
            self.sse("content_block_delta", {"type": "content_block_delta", "index": index,
                                             "delta": {"type": "input_json_delta",
                                                       "partial_json": json.dumps({"command": tool})}})
            self.sse("content_block_stop", {"type": "content_block_stop", "index": index})
        self.sse("message_delta", {"type": "message_delta",
                                   "delta": {"stop_reason": "tool_use" if tool else "end_turn",
                                             "stop_sequence": None},
                                   "usage": {"input_tokens": 1000, "output_tokens": max(1, len(text) // 4),
                                             "cache_creation_input_tokens": 0,
                                             "cache_read_input_tokens": 0}})
        self.sse("message_stop", {"type": "message_stop"})
        self.end_chunks()
        svc.log(ev="done", kind=plan.kind, tool=bool(tool))

    def script(self, plan: Plan) -> tuple[str, str | None]:
        """(text, shell command or None) for a plan."""
        if plan.kind == "long":
            return long_reply(), None
        if plan.kind == "wait":
            return WAIT_REPLY, None
        if plan.kind == "tool":
            return "", TOOL_COMMAND
        if plan.kind == "tool-done":
            return TOOL_REPLY, None
        if plan.kind == "warm":
            return "", warm_command(plan.turn)
        if plan.kind == "warm-done":
            return warm_reply(plan.turn), None
        return "ok", None

    # -- OpenAI Responses (codex) ------------------------------------------

    def openai_responses(self, body: dict) -> None:
        svc = self.service
        items = body.get("input") or []
        texts = []
        for item in items:
            if item.get("role") == "user":
                for part in item.get("content") or []:
                    if part.get("type") in ("input_text", "text"):
                        texts.append(part.get("text", ""))
        outputs = ("function_call_output", "custom_tool_call_output", "local_shell_call_output")
        results = {item.get("call_id", "") for item in items if item.get("type") in outputs}
        tools = body.get("tools") or []
        names = [tool.get("name") or tool.get("type", "") for tool in tools]
        plan = plan_from_texts(texts, svc.answered_by(results)) if names else Plan("other")
        svc.log(ev="req", api="responses", kind=plan.kind, turn=plan.turn, tools=names[:12])
        if plan.kind == "wait":
            time.sleep(3.0)
        text, tool = self.script(plan)
        fast = plan.kind in ("warm", "warm-done", "other", "tool", "wait")
        self.start_sse()
        seq = 0

        def emit(payload: dict) -> None:
            nonlocal seq
            payload["sequence_number"] = seq
            seq += 1
            self.sse(payload["type"], payload)

        emit({"type": "response.created", "response": {"id": "resp_bench", "status": "in_progress",
                                                       "output": []}})
        output_index = 0
        if text:
            item = {"id": "msg_bench", "type": "message", "role": "assistant", "status": "in_progress",
                    "content": []}
            emit({"type": "response.output_item.added", "output_index": 0, "item": item})
            emit({"type": "response.content_part.added", "item_id": "msg_bench", "output_index": 0,
                  "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}})
            deltas = svc.chunks(text, fast)
            pace = svc.pace(fast)
            svc.log(ev="first-delta", kind=plan.kind, deltas=len(deltas))
            for number, piece in enumerate(deltas):
                if number and pace:
                    time.sleep(pace)
                emit({"type": "response.output_text.delta", "item_id": "msg_bench", "output_index": 0,
                      "content_index": 0, "delta": piece})
            svc.log(ev="last-delta", kind=plan.kind, deltas=len(deltas))
            emit({"type": "response.output_text.done", "item_id": "msg_bench", "output_index": 0,
                  "content_index": 0, "text": text})
            done_item = dict(item, status="completed",
                             content=[{"type": "output_text", "text": text, "annotations": []}])
            emit({"type": "response.output_item.done", "output_index": 0, "item": done_item})
            output_index = 1
        if tool:
            call_id = svc.issue("call_bench", plan)
            name = pick_tool(names, ["shell_command", "shell", "exec_command", "local_shell"])
            if name == "shell":
                arguments = {"command": ["bash", "-lc", tool]}
            elif name == "exec_command":
                arguments = {"cmd": tool}
            else:
                arguments = {"command": tool}
            call = {"id": f"fc_{call_id}", "type": "function_call", "status": "completed",
                    "name": name or "shell_command", "call_id": call_id,
                    "arguments": json.dumps(arguments)}
            emit({"type": "response.output_item.added", "output_index": output_index, "item": call})
            emit({"type": "response.output_item.done", "output_index": output_index, "item": call})
        emit({"type": "response.completed", "response": {
            "id": "resp_bench", "status": "completed", "output": [],
            "usage": {"input_tokens": 1000, "input_tokens_details": {"cached_tokens": 0},
                      "output_tokens": max(1, len(text) // 4),
                      "output_tokens_details": {"reasoning_tokens": 0},
                      "total_tokens": 1000 + max(1, len(text) // 4)}}})
        self.end_chunks()
        svc.log(ev="done", kind=plan.kind, tool=bool(tool))

    # -- Gemini generateContent --------------------------------------------

    def gemini(self, body: dict, streaming: bool) -> None:
        svc = self.service
        contents = body.get("contents") or []
        texts = [part["text"] for content in contents if content.get("role") == "user"
                 for part in content.get("parts") or [] if "text" in part]
        last = contents[-1] if contents else {}
        # Gemini's function responses carry no id this service chose, so the
        # shape decides: a conversation that ends on a function response is
        # waiting for the turn's answer.
        after_tool = any("functionResponse" in part for part in last.get("parts") or [])
        declarations = []
        for tool in body.get("tools") or []:
            for declaration in tool.get("functionDeclarations") or []:
                declarations.append(declaration.get("name", ""))
        plan = (plan_from_texts(texts, lambda _key: after_tool)
                if declarations else Plan("other"))
        answers = [json.dumps(part["functionResponse"])[:300] for part in last.get("parts") or []
                   if "functionResponse" in part]
        svc.log(ev="req", api="gemini", kind=plan.kind, turn=plan.turn, stream=streaming,
                tools=len(declarations), answers=answers)
        if plan.kind == "wait":
            time.sleep(3.0)
        text, tool = self.script(plan)
        usage = {"promptTokenCount": 1000, "candidatesTokenCount": max(1, len(text) // 4),
                 "totalTokenCount": 1000 + max(1, len(text) // 4)}
        if plan.kind == "other" and not streaming:
            # A side request (next-speaker check, a summary): an answer that
            # hands the turn back to the person.
            answer = json.dumps({"reasoning": "done", "next_speaker": "user"})
            self.send_json(200, {"candidates": [{"content": {"role": "model",
                                                             "parts": [{"text": answer}]},
                                                 "finishReason": "STOP", "index": 0}],
                                 "usageMetadata": usage})
            return
        name = pick_tool(declarations, ["run_shell_command"]) if tool else None
        if not streaming:
            parts = [{"text": text}] if text else []
            if tool and name:
                parts.append({"functionCall": {"name": name, "args": {"command": tool}}})
            self.send_json(200, {"candidates": [{"content": {"role": "model", "parts": parts},
                                                 "finishReason": "STOP", "index": 0}],
                                 "usageMetadata": usage})
            return
        fast = plan.kind in ("warm", "warm-done", "other", "tool", "wait")
        self.start_sse()
        if text:
            deltas = svc.chunks(text, fast)
            pace = svc.pace(fast)
            svc.log(ev="first-delta", kind=plan.kind, deltas=len(deltas))
            for number, piece in enumerate(deltas):
                if number and pace:
                    time.sleep(pace)
                self.sse(None, {"candidates": [{"content": {"role": "model", "parts": [{"text": piece}]},
                                                "index": 0}]})
            svc.log(ev="last-delta", kind=plan.kind, deltas=len(deltas))
        if tool and name:
            self.sse(None, {"candidates": [{"content": {"role": "model", "parts": [
                {"functionCall": {"name": name, "args": {"command": tool}}}]}, "index": 0}]})
        self.sse(None, {"candidates": [{"content": {"role": "model", "parts": [{"text": ""}]},
                                        "finishReason": "STOP", "index": 0}], "usageMetadata": usage})
        self.end_chunks()
        svc.log(ev="done", kind=plan.kind, tool=bool(tool and name))


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, request, client_address) -> None:
        # A CLI that exits closes its kept-alive connection under a handler
        # waiting for the next request: that is an ending, not an error.
        if isinstance(sys.exc_info()[1], (ConnectionResetError, BrokenPipeError)):
            return
        super().handle_error(request, client_address)


def serve(args: argparse.Namespace) -> int:
    Handler.service = Service(args.delta_ms, args.delta_chars, Path(args.log))
    server = Server(("127.0.0.1", 0), Handler)
    Path(args.port_file).write_text(str(server.server_address[1]))
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        server.serve_forever(poll_interval=0.5)
    except KeyboardInterrupt:
        pass
    return 0


# ---------------------------------------------------------------------------
# Reading a process tree from the kernel.
# ---------------------------------------------------------------------------


class RusageInfoV2(ctypes.Structure):
    _fields_ = [
        ("ri_uuid", ctypes.c_uint8 * 16),
        ("ri_user_time", ctypes.c_uint64),
        ("ri_system_time", ctypes.c_uint64),
        ("ri_pkg_idle_wkups", ctypes.c_uint64),
        ("ri_interrupt_wkups", ctypes.c_uint64),
        ("ri_pageins", ctypes.c_uint64),
        ("ri_wired_size", ctypes.c_uint64),
        ("ri_resident_size", ctypes.c_uint64),
        ("ri_phys_footprint", ctypes.c_uint64),
        ("ri_proc_start_abstime", ctypes.c_uint64),
        ("ri_proc_exit_abstime", ctypes.c_uint64),
        ("ri_child_user_time", ctypes.c_uint64),
        ("ri_child_system_time", ctypes.c_uint64),
        ("ri_child_pkg_idle_wkups", ctypes.c_uint64),
        ("ri_child_interrupt_wkups", ctypes.c_uint64),
        ("ri_child_pageins", ctypes.c_uint64),
        ("ri_child_elapsed_abstime", ctypes.c_uint64),
        ("ri_diskio_bytesread", ctypes.c_uint64),
        ("ri_diskio_byteswritten", ctypes.c_uint64),
    ]


class TimebaseInfo(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


_LIBPROC = ctypes.CDLL("/usr/lib/libproc.dylib")
_TIMEBASE = TimebaseInfo()
ctypes.CDLL("/usr/lib/libSystem.dylib").mach_timebase_info(ctypes.byref(_TIMEBASE))


def ticks_to_ms(ticks: int) -> float:
    return ticks * _TIMEBASE.numer / _TIMEBASE.denom / 1e6


def rusage(pid: int) -> RusageInfoV2 | None:
    info = RusageInfoV2()
    if _LIBPROC.proc_pid_rusage(pid, 2, ctypes.byref(info)) != 0:
        return None
    return info


def children(pid: int) -> list[int]:
    buffer = (ctypes.c_int * 512)()
    count = _LIBPROC.proc_listchildpids(pid, buffer, ctypes.sizeof(buffer))
    if count <= 0:
        return []
    return [value for value in buffer[:count] if value > 0]


def tree(pid: int) -> list[int]:
    return [child for child, _parent in tree_edges(pid)]


def tree_edges(root: int) -> list[tuple[int, int]]:
    """(pid, parent) for the root and everything below it, parents first."""
    seen: set[int] = set()
    edges: list[tuple[int, int]] = []
    stack = [(root, 0)]
    while stack:
        pid, parent = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        edges.append((pid, parent))
        stack.extend((child, pid) for child in children(pid))
    return edges


def executable(pid: int) -> str:
    buffer = ctypes.create_string_buffer(4096)
    if _LIBPROC.proc_pidpath(pid, buffer, ctypes.sizeof(buffer)) <= 0:
        return ""
    return os.path.basename(buffer.value.decode(errors="replace"))


# A tool's command runs in a shell; the shell and everything under it is the
# tool's work, the same for every CLI, and the rest is the CLI's own.
SHELLS = {"bash", "sh", "zsh", "dash", "ksh", "fish"}


@dataclass
class Sample:
    t: int
    cpu_ms: float           # the tree: live processes + their reaped children
    cli_cpu_ms: float       # the CLI's own processes (no shell, nothing a shell ran)
    self_cpu_ms: float      # the process the pty started
    wakeups: int
    rss: int                # resident bytes, the tree
    cli_rss: int            # resident bytes, the CLI's own processes
    footprint: int          # physical footprint, the tree
    self_rss: int
    processes: int


class TreeSampler:
    """CPU that only grows: a CLI process that exits keeps what it spent."""

    def __init__(self, root: int) -> None:
        self.root = root
        self.cli_seen: dict[int, float] = {}
        self.cli_gone = 0.0

    def sample(self) -> Sample | None:
        cpu = self_cpu = 0.0
        wakeups = rss = cli_rss = footprint = self_rss = 0
        tool: set[int] = set()
        cli_now: dict[int, float] = {}
        count = 0
        for pid, parent in tree_edges(self.root):
            if parent in tool or executable(pid) in SHELLS:
                tool.add(pid)
            info = rusage(pid)
            if info is None:
                continue
            count += 1
            own = ticks_to_ms(info.ri_user_time + info.ri_system_time)
            reaped = ticks_to_ms(info.ri_child_user_time + info.ri_child_system_time)
            cpu += own + reaped
            wakeups += info.ri_pkg_idle_wkups + info.ri_interrupt_wkups
            rss += info.ri_resident_size
            footprint += info.ri_phys_footprint
            if pid not in tool:
                cli_now[pid] = own
                cli_rss += info.ri_resident_size
            if pid == self.root:
                self_cpu = own
                self_rss = info.ri_resident_size
        if count == 0:
            return None
        for pid, spent in self.cli_seen.items():
            if pid not in cli_now:
                self.cli_gone += spent
        self.cli_seen = cli_now
        return Sample(mono_ns(), cpu, sum(cli_now.values()) + self.cli_gone, self_cpu, wakeups,
                      rss, cli_rss, footprint, self_rss, count)


# ---------------------------------------------------------------------------
# A terminal: a pty, a screen, and answers to what a CLI asks a terminal.
# ---------------------------------------------------------------------------

try:
    import pyte  # type: ignore
except ImportError:  # pragma: no cover - the message says what to do
    pyte = None


class AnsweringScreen(pyte.Screen if pyte else object):  # type: ignore[misc]
    def __init__(self, columns: int, lines: int) -> None:
        super().__init__(columns, lines)
        self.replies: list[str] = []

    def write_process_input(self, data: str) -> None:
        self.replies.append(data)

    def select_graphic_rendition(self, *attrs, private: bool = False) -> None:
        # `CSI ? … m` (a private SGR some CLIs send) has no colour to keep.
        if not private:
            super().select_graphic_rendition(*attrs)


QUERIES = [
    (re.compile(rb"\x1b\]1([01]);\?(?:\x07|\x1b\\)"), None),       # OSC 10/11 colour query
    (re.compile(rb"\x1b\[\?2026\$p"), b"\x1b[?2026;2$y"),          # DECRQM synchronized output
    (re.compile(rb"\x1b\[>0?c"), b"\x1b[>1;10;0c"),                  # DA2
    (re.compile(rb"\x1b\[18t"), f"\x1b[8;{ROWS};{COLS}t".encode()),  # text area size
    (re.compile(rb"\x1b\[14t"), f"\x1b[4;{ROWS * 18};{COLS * 9}t".encode()),
    (re.compile(rb"\x1b\[16t"), b"\x1b[6;18;9t"),
]
COLOURS = {b"0": b"rgb:d8d8/d8d8/d8d8", b"1": b"rgb:1c1c/1c1c/1c1c"}
SYNC_BEGIN = b"\x1b[?2026h"
# Keyboard-protocol modes (kitty push/pop, modifyOtherKeys): no picture of
# their own, and pyte would print their parameters as text.
KEYBOARD_MODES = re.compile(rb"\x1b\[[<>=][0-9;]*[a-zA-Z]")


@dataclass
class Chunk:
    t: int
    size: int


@dataclass
class Keystroke:
    char: str
    sent: int
    echoed: int | None = None


class Terminal:
    """One CLI in a pty. Everything happens on this thread, in `pump`."""

    def __init__(self, argv: list[str], env: dict[str, str], cwd: Path) -> None:
        self.started = mono_ns()
        pid, fd = pty.fork()
        if pid == 0:  # the child: size its terminal, then become the CLI
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
                os.chdir(cwd)
                os.execve(argv[0], argv, env)
            finally:
                os._exit(127)
        self.pid, self.fd = pid, fd
        self.sampler = TreeSampler(pid)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        flags = fcntl.fcntl(fd, fcntl.F_GETFL)
        fcntl.fcntl(fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)
        self.screen = AnsweringScreen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.chunks: list[Chunk] = []
        self.sync_frames: list[int] = []
        self.first_byte: int | None = None
        self.carry = b""
        self.samples: list[Sample] = []
        self.next_sample = 0
        self.keys: list[Keystroke] = []
        self.pending_echo: list[Keystroke] = []
        self.search_tail = b""
        self.exited: int | None = None
        self.output_total = 0
        self.watch: list[tuple[str, list[int]]] = []

    # -- plumbing ----------------------------------------------------------

    def write(self, data: bytes) -> None:
        view = memoryview(data)
        while view and self.exited is None:
            try:
                written = os.write(self.fd, view)
            except BlockingIOError:
                select.select([], [self.fd], [], 0.05)
                continue
            except OSError:
                # The CLI is gone and took its terminal with it.
                self.reap(block=False)
                if self.exited is None:
                    self.exited = -1
                return
            view = view[written:]

    def answer(self, data: bytes) -> None:
        scan = self.carry + data
        boundary = len(self.carry)
        for pattern, fixed in QUERIES:
            for match in pattern.finditer(scan):
                if match.end() <= boundary:
                    continue
                if fixed is not None:
                    self.write(fixed)
                    continue
                which = match.group(1)
                terminator = b"\x07" if match.group(0).endswith(b"\x07") else b"\x1b\\"
                self.write(b"\x1b]1" + which + b";" + COLOURS[which] + terminator)
        self.carry = scan[-64:]

    def feed(self, data: bytes, now: int) -> None:
        if self.first_byte is None:
            self.first_byte = now
        self.chunks.append(Chunk(now, len(data)))
        self.output_total += len(data)
        start = 0
        while True:
            index = data.find(SYNC_BEGIN, start)
            if index < 0:
                break
            self.sync_frames.append(now)
            start = index + 1
        self.answer(data)
        self.stream.feed(KEYBOARD_MODES.sub(b"", data))
        for reply in self.screen.replies:
            self.write(reply.encode())
        self.screen.replies.clear()
        haystack = self.search_tail + data
        if self.pending_echo:
            still = []
            for key in self.pending_echo:
                if key.char.encode() in haystack:
                    key.echoed = now
                else:
                    still.append(key)
            self.pending_echo = still
        waiting = [(needle, hits) for needle, hits in self.watch if not hits]
        if waiting:
            # On the screen, not in the bytes: a renderer that writes only the
            # cells that changed may never send a word in one piece.
            text = self.screen_text()
            for needle, hits in waiting:
                if needle in text:
                    hits.append(now)
        self.search_tail = haystack[-96:]

    def pump(self, timeout: float) -> None:
        """Read what is there (waiting at most `timeout`), sample, reap."""
        deadline = time.monotonic() + timeout
        while True:
            now_s = time.monotonic()
            if mono_ns() >= self.next_sample:
                self.take_sample()
            wait = max(0.0, min(deadline - now_s, (self.next_sample - mono_ns()) / 1e9))
            try:
                ready, _, _ = select.select([self.fd], [], [], wait)
            except InterruptedError:
                ready = []
            if ready:
                while True:
                    try:
                        data = os.read(self.fd, 65536)
                    except BlockingIOError:
                        break
                    except OSError:
                        data = b""
                    if not data:
                        self.reap(block=False)
                        break
                    self.feed(data, mono_ns())
            if self.exited is None:
                self.reap(block=False)
            if time.monotonic() >= deadline:
                return

    def take_sample(self) -> None:
        sample = self.sampler.sample() if self.exited is None else None
        if sample is not None:
            self.samples.append(sample)
        self.next_sample = mono_ns() + 100_000_000

    def reap(self, block: bool) -> None:
        if self.exited is not None:
            return
        try:
            pid, status = os.waitpid(self.pid, 0 if block else os.WNOHANG)
        except ChildProcessError:
            self.exited = -1
            return
        if pid == self.pid:
            self.exited = status

    # -- conversation ------------------------------------------------------

    def screen_text(self) -> str:
        return "\n".join(self.screen.display)

    def wait_screen(self, pattern: re.Pattern, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if pattern.search(self.screen_text()):
                return True
            if self.exited is not None:
                return False
            self.pump(0.05)
        return False

    def wait_quiet(self, quiet_s: float, timeout: float) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            last = self.chunks[-1].t if self.chunks else self.started
            if mono_ns() - last >= quiet_s * 1e9:
                return
            self.pump(0.05)

    def expect(self, needle: str) -> list[int]:
        hits: list[int] = []
        self.watch.append((needle, hits))
        return hits

    def type_text(self, text: str) -> None:
        self.write(text.encode())

    def submit(self, text: str) -> int:
        self.type_text(text)
        self.pump(0.2)
        sent = mono_ns()
        self.write(b"\r")
        return sent

    def keystroke(self, char: str) -> None:
        key = Keystroke(char, mono_ns())
        self.keys.append(key)
        self.pending_echo.append(key)
        self.write(char.encode())

    def close(self, quit_keys: list[bytes]) -> None:
        for keys in quit_keys:
            if self.exited is not None:
                break
            self.write(keys)
            self.pump(0.3)
        deadline = time.monotonic() + 5
        while self.exited is None and time.monotonic() < deadline:
            self.pump(0.1)
        for sig in (signal.SIGTERM, signal.SIGKILL):
            if self.exited is not None:
                break
            try:
                os.killpg(self.pid, sig)  # the CLI is its own session: only what we started
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + 3
            while self.exited is None and time.monotonic() < deadline:
                self.pump(0.1)
        try:
            os.close(self.fd)
        except OSError:
            pass

    # -- measurement -------------------------------------------------------

    def at(self, t: int) -> Sample | None:
        best = None
        for sample in self.samples:
            if sample.t <= t:
                best = sample
            else:
                break
        return best

    def window(self, start: int, end: int, deltas: int = 0) -> dict:
        first = self.at(start)
        last = self.at(end)
        seconds = max(1e-9, (end - start) / 1e9)
        chunks = [chunk for chunk in self.chunks if start <= chunk.t < end]
        written = sum(chunk.size for chunk in chunks)
        bursts, previous = 0, None
        for chunk in chunks:
            if previous is None or chunk.t - previous > 4_000_000:
                bursts += 1
            previous = chunk.t
        syncs = sum(1 for t in self.sync_frames if start <= t < end)
        inside = [sample for sample in self.samples if start <= sample.t <= end]
        result = {
            "seconds": round(seconds, 3),
            "bytes": written,
            "bytes_per_s": round(written / seconds, 1),
            "bursts_per_s": round(bursts / seconds, 2),
            "sync_frames_per_s": round(syncs / seconds, 2),
        }
        if first and last:
            result["cpu_ms"] = round(last.cpu_ms - first.cpu_ms, 1)
            result["cpu_pct"] = round((last.cpu_ms - first.cpu_ms) / (seconds * 10), 2)
            result["cli_cpu_ms"] = round(last.cli_cpu_ms - first.cli_cpu_ms, 1)
            result["cli_cpu_pct"] = round((last.cli_cpu_ms - first.cli_cpu_ms) / (seconds * 10), 2)
            result["self_cpu_ms"] = round(last.self_cpu_ms - first.self_cpu_ms, 1)
            result["wakeups_per_s"] = round((last.wakeups - first.wakeups) / seconds, 1)
        if inside:
            result["rss_mb"] = round(max(sample.rss for sample in inside) / 2**20, 1)
            result["cli_rss_mb"] = round(max(sample.cli_rss for sample in inside) / 2**20, 1)
            result["footprint_mb"] = round(max(sample.footprint for sample in inside) / 2**20, 1)
            result["self_rss_mb"] = round(max(sample.self_rss for sample in inside) / 2**20, 1)
            result["processes"] = max(sample.processes for sample in inside)
        if deltas:
            result["deltas"] = deltas
            result["bytes_per_delta"] = round(written / deltas, 1)
        return result


# ---------------------------------------------------------------------------
# The CLIs.
# ---------------------------------------------------------------------------

TYPED = "абвгдежзийклмнопрстуфхцчшщыэюя"  # letters nothing else on screen uses


@dataclass
class Cli:
    name: str
    binary: str
    argv: Callable[["RunDirs"], list[str]]
    env: Callable[["RunDirs", str], dict[str, str]]
    prepare: Callable[["RunDirs", str], None]
    ready: re.Pattern
    quit_keys: list[bytes] = field(default_factory=lambda: [b"\x03", b"\x03", b"\x04"])
    version: str = ""
    kind: str = ""


@dataclass
class RunDirs:
    root: Path
    home: Path
    project: Path
    state: Path


def base_env(dirs: RunDirs) -> dict[str, str]:
    return {
        "PATH": BENCH_PATH,
        "HOME": str(dirs.home),
        "USER": os.environ.get("USER", "bench"),
        "LOGNAME": os.environ.get("USER", "bench"),
        "SHELL": "/bin/bash",
        "TERM": "xterm-256color",
        "COLORTERM": "truecolor",
        "LANG": "en_US.UTF-8",
        "LC_ALL": "en_US.UTF-8",
        "TMPDIR": str(dirs.state) + "/",
    }


def zo_cli(binary: str, label: str, seed_agents: int) -> Cli:
    def argv(_dirs: RunDirs) -> list[str]:
        return [binary, "--permission-mode", "danger-full-access"]

    def env(dirs: RunDirs, url: str) -> dict[str, str]:
        values = base_env(dirs)
        values.update({
            "ZO_CONFIG_HOME": str(dirs.home),
            "ZO_SESSION_ROOT": str(dirs.state / "sessions"),
            "ZO_STATE_DIR": str(dirs.state),
            "ZO_AGENT_STORE": str(dirs.state / "agents"),
            "ANTHROPIC_BASE_URL": url,
            "ANTHROPIC_API_KEY": DUMMY_KEY,
            "ZO_DISABLE_KEYCHAIN": "1",
            "ZO_DISABLE_MODEL_DISCOVERY": "1",
            "ZO_DISABLE_EXTERNAL_CREDENTIALS": "1",
            "CLAUDE_CONFIG_DIR": str(dirs.home / "claude"),
            "CODEX_HOME": str(dirs.home / "codex"),
            "ZO_CODEX_HOME": str(dirs.home / "codex"),
        })
        return values

    def prepare(dirs: RunDirs, _url: str) -> None:
        store = dirs.state / "agents"
        store.mkdir(parents=True, exist_ok=True)
        seed_agent_store(store, seed_agents)

    return Cli(label, binary, argv, env, prepare, re.compile(r"›"), kind="zo")


def seed_agent_store(store: Path, count: int) -> None:
    """Finished helpers of other sessions, the shape a busy project's store has.

    A person's project store held 432 manifests on 2026-09-28; the roster
    watcher reads every one of them each second. Synthetic, so nothing of a
    real session is copied.
    """
    rng = random.Random(7)
    started = int(time.time()) - 86_400
    for index in range(count):
        agent_id = f"agent-{index:05d}-{rng.randrange(16**8):08x}"
        manifest = {
            "agentId": agent_id,
            "name": f"helper {index}",
            "label": f"helper {index}",
            "subagentType": "general-purpose",
            "model": "bench-model",
            "status": "completed",
            "parentSessionId": f"session-{rng.randrange(16**12):012x}",
            "startedAt": str(started + index),
            "recentTools": ["Read", "Grep", "Bash", "Edit"],
            "toolCalls": rng.randrange(1, 90),
            "lastActivityAt": started + index + 60,
            "outputTail": " ".join(f"word{rng.randrange(1000)}" for _ in range(160)),
        }
        (store / f"{agent_id}.json").write_text(json.dumps(manifest))
        (store / f"{agent_id}.session.jsonl").write_text('{"type":"session_meta"}\n')


def claude_cli(binary: str) -> Cli:
    def argv(_dirs: RunDirs) -> list[str]:
        return [binary, "--permission-mode", "default", "--allowedTools", "Bash"]

    def env(dirs: RunDirs, url: str) -> dict[str, str]:
        values = base_env(dirs)
        values.update({
            "CLAUDE_CONFIG_DIR": str(dirs.home / ".claude"),
            "ANTHROPIC_BASE_URL": url,
            "ANTHROPIC_API_KEY": DUMMY_KEY,
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_TELEMETRY": "1",
            "DISABLE_ERROR_REPORTING": "1",
            "CLAUDE_CODE_DISABLE_TERMINAL_TITLE": "1",
        })
        return values

    def prepare(dirs: RunDirs, _url: str) -> None:
        config = dirs.home / ".claude"
        config.mkdir(parents=True, exist_ok=True)
        state = {
            "numStartups": 3,
            "hasCompletedOnboarding": True,
            "lastOnboardingVersion": claude_version(binary),
            "lastReleaseNotesSeen": claude_version(binary),
            "theme": "dark",
            "autoUpdates": False,
            "customApiKeyResponses": {"approved": [DUMMY_KEY[-20:]], "rejected": []},
            "projects": {str(dirs.project): {"hasTrustDialogAccepted": True,
                                             "hasCompletedProjectOnboarding": True,
                                             "allowedTools": []}},
        }
        (config / ".claude.json").write_text(json.dumps(state))

    return Cli("claude", binary, argv, env, prepare, re.compile(r"❯|for shortcuts"),
               quit_keys=[b"\x03", b"\x03", b"\x04"])


def claude_version(binary: str) -> str:
    try:
        output = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=30,
                                env={"PATH": BENCH_PATH, "HOME": tempfile.gettempdir()}).stdout
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return output.split()[0] if output.split() else ""


def codex_cli(binary: str) -> Cli:
    def argv(_dirs: RunDirs) -> list[str]:
        # In-process: codex 0.157 otherwise hands the conversation to an
        # app-server daemon it detaches from the terminal, outside the tree
        # this benchmark measures (and whose socket path must fit SUN_LEN).
        return [binary, "--no-daemon"]

    def env(dirs: RunDirs, _url: str) -> dict[str, str]:
        values = base_env(dirs)
        values.update({"CODEX_HOME": str(dirs.home / ".codex"), "BENCH_API_KEY": DUMMY_KEY})
        return values

    def prepare(dirs: RunDirs, url: str) -> None:
        home = dirs.home / ".codex"
        home.mkdir(parents=True, exist_ok=True)
        (home / "config.toml").write_text(
            'model = "bench-model"\n'
            'model_provider = "bench"\n'
            'approval_policy = "never"\n'
            'sandbox_mode = "danger-full-access"\n'
            'check_for_update_on_startup = false\n'
            '[model_providers.bench]\n'
            'name = "bench"\n'
            f'base_url = "{url}/v1"\n'
            'wire_api = "responses"\n'
            'env_key = "BENCH_API_KEY"\n'
            'request_max_retries = 0\n'
            'stream_max_retries = 0\n'
            f'[projects."{dirs.project}"]\n'
            'trust_level = "trusted"\n'
            '[notice]\n'
            'hide_full_access_warning = true\n'
        )

    return Cli("codex", binary, argv, env, prepare, re.compile(r"model:\s+bench-model"),
               quit_keys=[b"\x03", b"\x03", b"\x04"])


def gemini_cli(binary: str) -> Cli:
    def argv(_dirs: RunDirs) -> list[str]:
        return [binary, "--yolo", "-m", "gemini-2.5-flash"]

    def env(dirs: RunDirs, url: str) -> dict[str, str]:
        values = base_env(dirs)
        values.update({"GEMINI_API_KEY": DUMMY_KEY, "GOOGLE_GEMINI_BASE_URL": url,
                       "GEMINI_CLI_NO_RELAUNCH": "true"})
        return values

    def prepare(dirs: RunDirs, _url: str) -> None:
        home = dirs.home / ".gemini"
        home.mkdir(parents=True, exist_ok=True)
        (home / "settings.json").write_text(json.dumps({
            "security": {"auth": {"selectedType": "gemini-api-key"},
                         "folderTrust": {"enabled": False}},
            "ui": {"theme": "Default"},
            "general": {"disableAutoUpdate": True, "disableUpdateNag": True},
            "privacy": {"usageStatisticsEnabled": False},
            "telemetry": {"enabled": False},
            "ide": {"hasSeenNudge": True, "enabled": False},
        }))

    return Cli("gemini", binary, argv, env, prepare, re.compile(r"Type your message"),
               quit_keys=[b"\x03", b"\x03", b"\x04"])


# ---------------------------------------------------------------------------
# One run.
# ---------------------------------------------------------------------------


class ServiceLog:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.offset = 0
        self.events: list[dict] = []

    def poll(self) -> None:
        if not self.path.exists():
            return
        with open(self.path, encoding="utf-8") as handle:
            handle.seek(self.offset)
            data = handle.read()
            self.offset = handle.tell()
        for line in data.splitlines():
            if line.strip():
                self.events.append(json.loads(line))

    def find(self, ev: str, kind: str | None = None, after: int = 0) -> dict | None:
        self.poll()
        for event in self.events:
            if event["t"] >= after and event["ev"] == ev and (kind is None or event.get("kind") == kind):
                return event
        return None


def make_dirs(base: Path, name: str, run: int) -> RunDirs:
    root = base / f"{name}-{run}"
    if root.exists():
        shutil.rmtree(root)
    home, project, state = root / "home", root / "project", root / "state"
    for path in (home, project, state):
        path.mkdir(parents=True)
    (project / "README.md").write_text("# bench project\n\nA folder for the benchmark to run in.\n")
    (project / "main.py").write_text("print('hello')\n")
    subprocess.run(["git", "init", "-q"], cwd=project, check=False,
                   env={"PATH": BENCH_PATH, "HOME": str(home)})
    return RunDirs(root, home, project, state)


class Sampler:
    """`sample <pid>` at the start of a state, for attribution runs only: the
    sampler stops the process at every sample, so its runs are not timed."""

    def __init__(self, seconds: int, out: Path) -> None:
        self.seconds = seconds
        self.out = out
        self.running: list[subprocess.Popen] = []

    def start(self, pid: int, name: str) -> None:
        if not self.seconds:
            return
        path = self.out / f"{name}.sample.txt"
        self.running.append(subprocess.Popen(
            ["/usr/bin/sample", str(pid), str(self.seconds), "-file", str(path)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))

    def finish(self) -> None:
        for process in self.running:
            process.wait()
        self.running.clear()


# The service event that opens each sampled state.
SAMPLE_AT = {"wait": ("req", "wait"), "long": ("first-delta", "long"), "tool": ("done", "tool")}


def turn(term: Terminal, log: ServiceLog, prompt: str, kind: str, marker: str,
         timeout: float, typing: bool = False,
         on_state: Callable[[], None] | None = None) -> dict:
    """Submit one prompt and follow it to the end marker. Windows are in ns."""
    hits = term.expect(marker)
    start = mono_ns()
    submitted = term.submit(prompt)
    typed: list[Keystroke] = []
    next_key = None
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline and not hits and term.exited is None:
        term.pump(0.02)
        if on_state and kind in SAMPLE_AT and log.find(SAMPLE_AT[kind][0], SAMPLE_AT[kind][1], after=start):
            on_state()
            on_state = None
        first = log.find("first-delta", kind, after=start)
        if typing and first and len(typed) < len(TYPED):
            now = mono_ns()
            if next_key is None:
                next_key = first["t"] + 1_000_000_000
            if now >= next_key and not log.find("last-delta", kind, after=start):
                term.keystroke(TYPED[len(typed)])
                typed.append(term.keys[-1])
                next_key = now + 250_000_000
    end_seen = hits[0] if hits else None
    term.wait_quiet(0.4, 8.0)
    # Let pending echoes land, then take the typed letters back out.
    if typed:
        term.pump(0.5)
        term.write(b"\x7f" * len(typed))
        term.wait_quiet(0.3, 3.0)
    log.poll()
    request = log.find("req", kind, after=start)
    first = log.find("first-delta", kind, after=start)
    last = log.find("last-delta", kind, after=start)
    return {
        "submitted": submitted,
        "request": request["t"] if request else None,
        "first_delta": first["t"] if first else None,
        "last_delta": last["t"] if last else None,
        "deltas": first.get("deltas", 0) if first else 0,
        "end_seen": end_seen,
        "typed": typed,
    }


# The turns one run takes, in order: `frame_times` reads zo's frame marks
# (`ZO_PROBE_FRAMES`) turn by turn and files them under the state each turn is.
TURN_STATES = ["waiting", "streaming", "tool"]


def frame_times(path: Path, result: dict) -> None:
    turns: list[list[dict]] = []
    if not path.exists():
        return
    for line in path.read_text().splitlines():
        mark = json.loads(line)
        if mark.get("mark") == "turn_start":
            turns.append([])
        elif mark.get("mark") == "frame" and turns:
            turns[-1].append(mark)
    states = TURN_STATES + ["warm"] * (len(turns) - len(TURN_STATES) - 1) + ["streaming_at_n"]
    for state, frames in zip(states, turns):
        holder = result.get(state)
        if not isinstance(holder, dict) or not frames:
            continue
        for key, name in (("paint_ms", "frame"), ("size_ms", "size"), ("commit_ms", "commit"),
                          ("frame_ms", "view")):
            values = sorted(frame[key] for frame in frames if key in frame)
            if values:
                holder[f"{name}_p50_ms"] = round(values[len(values) // 2], 3)
                holder[f"{name}_p99_ms"] = round(values[min(len(values) - 1, int(len(values) * 0.99))], 3)
        holder["draws"] = len(frames)


def echo_stats(keys: list[Keystroke]) -> dict:
    latencies = [(key.echoed - key.sent) / 1e6 for key in keys if key.echoed is not None]
    missing = sum(1 for key in keys if key.echoed is None)
    result = {"keys": len(keys), "echo_missing": missing,
              "echo_ms": [round(latency, 2) for latency in latencies]}
    if latencies:
        latencies.sort()
        result["echo_p50_ms"] = round(statistics.median(latencies), 1)
        result["echo_p99_ms"] = round(latencies[min(len(latencies) - 1, int(len(latencies) * 0.99))], 1)
        result["echo_max_ms"] = round(latencies[-1], 1)
    return result


def run_once(cli: Cli, run: int, args: argparse.Namespace, url: str, log: ServiceLog,
             base: Path) -> dict:
    dirs = make_dirs(base, cli.name, run)
    cli.prepare(dirs, url)
    result: dict = {"cli": cli.name, "run": run, "uptime": uptime()}
    env = cli.env(dirs, url)
    frames = dirs.root / "frames.jsonl"
    if args.frames and cli.kind == "zo":
        env["ZO_PROBE_FRAMES"] = str(frames)
    term = Terminal(cli.argv(dirs), env, dirs.project)
    sampler = Sampler(args.sample, Path(args.out_dir))

    def sample(state: str) -> Callable[[], None]:
        return lambda: sampler.start(term.pid, f"{cli.name}-{run}-{state}")

    try:
        ready = term.wait_screen(cli.ready, 90)
        now = mono_ns()
        result["start"] = {
            "first_byte_ms": round((term.first_byte - term.started) / 1e6, 1) if term.first_byte else None,
            "ready_ms": round((now - term.started) / 1e6, 1) if ready else None,
        }
        if not ready:
            result["error"] = "never showed its composer"
            result["screen"] = term.screen_text()
            return result
        if args.startup_only:
            return result
        term.wait_quiet(1.0, 10)
        t0 = mono_ns()
        sample("idle")()
        term.pump(5.0)
        result["idle"] = term.window(t0, mono_ns())

        waited = turn(term, log, "bench:wait", "wait", END_WAIT, 60, on_state=sample("waiting"))
        if waited["request"] and waited["first_delta"]:
            result["waiting"] = term.window(waited["request"] + 200_000_000, waited["first_delta"])

        def streaming(label: str) -> None:
            streamed = turn(term, log, "bench:long", "long", END_LONG, 120, typing=True,
                            on_state=sample(label))
            if streamed["first_delta"] and streamed["end_seen"]:
                window = term.window(streamed["first_delta"], streamed["end_seen"], streamed["deltas"])
                window.update(echo_stats(streamed["typed"]))
                window["tail_ms"] = round((streamed["end_seen"] - streamed["last_delta"]) / 1e6, 1)
                result[label] = window
            else:
                result.setdefault("errors", []).append(f"{label}: the reply never finished on screen")
                result[f"{label}_screen"] = term.screen_text()

        streaming("streaming")
        if term.exited is not None:
            result["error"] = "the CLI exited during the scenario"
            result["screen"] = term.screen_text()
            return result

        tooled = turn(term, log, "bench:tool", "tool", END_TOOL, 60, on_state=sample("tool"))
        log.poll()
        tool_done = log.find("done", "tool", after=tooled["submitted"])
        tool_back = log.find("req", "tool-done", after=tooled["submitted"])
        if tool_done and tool_back:
            result["tool"] = term.window(tool_done["t"], tool_back["t"])

        rss_by_turn = []
        for number in range(1, args.turns + 1):
            warm = turn(term, log, f"bench:warm {number}", "warm", end_warm(number), 60)
            if warm["end_seen"] is None:
                result.setdefault("errors", []).append(f"warm-up {number} never finished")
                break
            if term.samples:
                rss_by_turn.append(round(term.samples[-1].cli_rss / 2**20, 1))
        result["rss_by_turn_mb"] = rss_by_turn
        result["turns"] = args.turns
        streaming("streaming_at_n")
        term.wait_quiet(1.0, 10)
        t1 = mono_ns()
        sample("idle_at_n")()
        term.pump(5.0)
        result["idle_at_n"] = term.window(t1, mono_ns())
        if args.frames and cli.kind == "zo":
            frame_times(frames, result)
        peak = max((taken.cli_rss for taken in term.samples), default=0)
        result["peak_rss_mb"] = round(peak / 2**20, 1)
        result["output_total_kb"] = round(term.output_total / 1024, 1)
    finally:
        sampler.finish()
        # A start-up run ends at once: a CLI may ignore quit keys for its
        # first moments (zo's startup grace), and only the start is measured.
        term.close([] if args.startup_only else cli.quit_keys)
        if not args.keep:
            shutil.rmtree(dirs.root, ignore_errors=True)
    return result


def uptime() -> str:
    try:
        return subprocess.run(["uptime"], capture_output=True, text=True, timeout=5).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return ""


def load1() -> float:
    return os.getloadavg()[0]


def wait_for_calm(limit: float, patience: float) -> None:
    """Hold a run until the 1-minute load is under `limit` (0: never hold),
    for at most `patience` seconds; the run records the load it ran at."""
    deadline = time.monotonic() + patience
    while limit and load1() >= limit and time.monotonic() < deadline:
        time.sleep(15)


# ---------------------------------------------------------------------------
# The table.
# ---------------------------------------------------------------------------

ROWS_OF_TABLE = [
    ("start", "first_byte_ms", "cold start → first byte (ms)"),
    ("start", "ready_ms", "cold start → composer (ms)"),
    ("idle", "cli_rss_mb", "RSS idle after start (MB)"),
    ("idle", "cli_cpu_pct", "CPU idle (%)"),
    ("idle", "wakeups_per_s", "wakeups idle (/s)"),
    ("idle", "bytes_per_s", "pty bytes idle (/s)"),
    ("waiting", "cli_cpu_pct", "CPU waiting for model (%)"),
    ("waiting", "bytes_per_s", "pty bytes waiting (/s)"),
    ("waiting", "bursts_per_s", "frames waiting (/s)"),
    ("streaming", "cli_cpu_pct", "CPU streaming (%)"),
    ("streaming", "bytes_per_s", "pty bytes streaming (/s)"),
    ("streaming", "bytes_per_delta", "pty bytes per delta"),
    ("streaming", "bursts_per_s", "frames streaming (/s)"),
    ("streaming", "echo_p50_ms", "keystroke→echo p50 streaming (ms)"),
    ("streaming", "echo_p99_ms", "keystroke→echo p99 streaming (ms)"),
    ("streaming", "echo_max_ms", "keystroke→echo max streaming (ms)"),
    ("streaming", "tail_ms", "last delta → on screen (ms)"),
    ("streaming", "frame_p50_ms", "draw time p50 streaming (ms, zo --frames)"),
    ("streaming", "frame_p99_ms", "draw time p99 streaming (ms, zo --frames)"),
    ("streaming", "view_p99_ms", "  of which view built+written p99 (ms)"),
    ("waiting", "frame_p99_ms", "draw time p99 waiting (ms, zo --frames)"),
    ("tool", "cli_cpu_pct", "CPU tool output, CLI only (%)"),
    ("tool", "cpu_pct", "CPU tool output, with the tool (%)"),
    ("tool", "bytes_per_s", "pty bytes tool output (/s)"),
    ("streaming_at_n", "cli_cpu_pct", "CPU streaming after N turns (%)"),
    ("streaming_at_n", "bytes_per_delta", "pty bytes per delta after N turns"),
    ("streaming_at_n", "echo_p50_ms", "keystroke→echo p50 after N turns (ms)"),
    ("streaming_at_n", "echo_p99_ms", "keystroke→echo p99 after N turns (ms)"),
    ("streaming_at_n", "echo_max_ms", "keystroke→echo max after N turns (ms)"),
    ("idle_at_n", "cli_rss_mb", "RSS idle after N turns (MB)"),
    ("idle_at_n", "cli_cpu_pct", "CPU idle after N turns (%)"),
    ("idle_at_n", "wakeups_per_s", "wakeups idle after N turns (/s)"),
    (None, "peak_rss_mb", "RSS peak (MB)"),
]


def median_of(results: list[dict], state: str | None, key: str) -> float | None:
    values = []
    for result in results:
        holder = result if state is None else result.get(state)
        if isinstance(holder, dict) and isinstance(holder.get(key), (int, float)):
            values.append(holder[key])
    return round(statistics.median(values), 1) if values else None


def pooled(results: list[dict], state: str, percentile: float) -> float | None:
    """A percentile of every keystroke of every run: one run's 30 keystrokes
    make its own p99 the slowest one, and five runs pooled make 150."""
    values = sorted(value for result in results if isinstance(result.get(state), dict)
                    for value in result[state].get("echo_ms", []))
    if not values:
        return None
    return round(values[min(len(values) - 1, int(len(values) * percentile / 100))], 1)


POOLED_ROWS = [
    ("streaming", 50, "keystroke→echo p50 streaming, all runs pooled (ms)"),
    ("streaming", 99, "keystroke→echo p99 streaming, all runs pooled (ms)"),
    ("streaming_at_n", 99, "keystroke→echo p99 after N turns, all runs pooled (ms)"),
]


def table(by_cli: dict[str, list[dict]]) -> str:
    names = list(by_cli)
    lines = ["| metric (median of runs) | " + " | ".join(names) + " |",
             "|---|" + "---|" * len(names)]
    for state, key, label in ROWS_OF_TABLE:
        cells = []
        for name in names:
            value = median_of(by_cli[name], state, key)
            cells.append("—" if value is None else f"{value:g}")
        lines.append(f"| {label} | " + " | ".join(cells) + " |")
    for state, percentile, label in POOLED_ROWS:
        cells = []
        for name in names:
            value = pooled(by_cli[name], state, percentile)
            cells.append("—" if value is None else f"{value:g}")
        lines.append(f"| {label} | " + " | ".join(cells) + " |")
    runs = " · ".join(f"{name} {len(results)}" for name, results in by_cli.items())
    lines.append(f"\nruns: {runs}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


def start_service(args: argparse.Namespace, base: Path, out: Path) -> tuple[subprocess.Popen, str, ServiceLog]:
    port_file = base / "service.port"
    log_path = out / "service.jsonl"
    for path in (port_file, log_path):
        if path.exists():
            path.unlink()
    process = subprocess.Popen(
        [sys.executable, __file__, "serve", "--port-file", str(port_file), "--log", str(log_path),
         "--delta-ms", str(args.delta_ms), "--delta-chars", str(args.delta_chars)],
        stdin=subprocess.DEVNULL,
    )
    deadline = time.monotonic() + 10
    while not port_file.exists() or not port_file.read_text().strip():
        if time.monotonic() > deadline:
            process.kill()
            raise SystemExit("the local model service did not start")
        time.sleep(0.05)
    return process, f"http://127.0.0.1:{port_file.read_text().strip()}", ServiceLog(log_path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command")
    serve_parser = sub.add_parser("serve", help=argparse.SUPPRESS)
    serve_parser.add_argument("--port-file", required=True)
    serve_parser.add_argument("--log", required=True)
    serve_parser.add_argument("--delta-ms", type=float, default=25)
    serve_parser.add_argument("--delta-chars", type=int, default=10)
    table_parser = sub.add_parser("table", help="print the table of saved runs (runs.jsonl files)")
    table_parser.add_argument("runs", nargs="+")
    parser.add_argument("--clis", default="zo,claude,codex,gemini",
                        help="which CLIs, in order (zo,claude,codex,gemini)")
    parser.add_argument("--zo", default="target/release/zo", help="the zo binary to measure")
    parser.add_argument("--zo-label", default="zo", help="the column name of --zo")
    parser.add_argument("--zo-extra", action="append", default=[],
                        help="another zo build as BINARY:LABEL (a before/after pair)")
    parser.add_argument("--claude", default=str(Path.home() / ".local/bin/claude"))
    parser.add_argument("--codex", default="/opt/homebrew/bin/codex")
    parser.add_argument("--gemini", default="/opt/homebrew/bin/gemini")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--turns", type=int, default=24, help="warm-up turns before the second stream")
    parser.add_argument("--delta-ms", type=float, default=25, help="time between text deltas")
    parser.add_argument("--delta-chars", type=int, default=10, help="characters per text delta")
    parser.add_argument("--seed-agents", type=int, default=432,
                        help="finished helpers in zo's agent store (the roster watcher reads them all)")
    parser.add_argument("--out", default=None, help="directory for results (default: a temp dir)")
    parser.add_argument("--keep", action="store_true", help="keep each run's home and project")
    parser.add_argument("--note", default="",
                        help="the machine's conditions, kept with every run and printed under the table")
    parser.add_argument("--startup-only", action="store_true",
                        help="measure cold start alone: launch, wait for the composer, quit")
    parser.add_argument("--settle", type=float, default=3,
                        help="seconds to wait before each run, after the last one's teardown")
    parser.add_argument("--calm", type=float, default=0,
                        help="start each run only once the 1-minute load is under this (0: at once)")
    parser.add_argument("--calm-wait", type=float, default=1800,
                        help="the longest a run waits for --calm, in seconds")
    parser.add_argument("--frames", action="store_true",
                        help="zo only: record each draw's time by phase (ZO_PROBE_FRAMES); a draw-time "
                             "run, since the probe writes a line per frame")
    parser.add_argument("--sample", type=int, default=0,
                        help="seconds of `sample <pid>` at the start of each state (attribution runs; "
                             "the sampler pauses the process, so these runs are not timings)")
    args = parser.parse_args()
    if args.command == "serve":
        return serve(args)
    if args.command == "table":
        by_cli: dict[str, list[dict]] = {}
        for path in args.runs:
            for line in Path(path).read_text().splitlines():
                result = json.loads(line)
                by_cli.setdefault(result["cli"], []).append(result)
        print(table(by_cli))
        return 0
    if pyte is None:
        print("tui-bench needs pyte: python3 -m pip install --user pyte", file=sys.stderr)
        return 2

    out = Path(args.out) if args.out else Path(tempfile.mkdtemp(prefix="tui-bench-"))
    out.mkdir(parents=True, exist_ok=True)
    args.out_dir = str(out)
    # The real path: /var is /private/var on macOS, and a CLI that records a
    # folder's trust by its real path would ask again for the other spelling.
    base = Path(os.path.realpath(tempfile.mkdtemp(prefix="tui-bench-runs-")))
    clis: list[Cli] = []
    for name in [value.strip() for value in args.clis.split(",") if value.strip()]:
        if name == "zo":
            clis.append(zo_cli(str(Path(args.zo).resolve()), args.zo_label, args.seed_agents))
            for extra in args.zo_extra:
                binary, _, label = extra.partition(":")
                clis.append(zo_cli(str(Path(binary).resolve()), label or binary, args.seed_agents))
        elif name == "claude":
            clis.append(claude_cli(args.claude))
        elif name == "codex":
            clis.append(codex_cli(args.codex))
        elif name == "gemini":
            clis.append(gemini_cli(args.gemini))
        else:
            raise SystemExit(f"unknown CLI {name}")
    for cli in clis:
        try:
            cli.version = subprocess.run([cli.binary, "--version"], capture_output=True, text=True,
                                         timeout=30, env={"PATH": BENCH_PATH,
                                                          "HOME": str(base)}).stdout.strip().splitlines()[0]
        except (OSError, subprocess.TimeoutExpired, IndexError):
            cli.version = "?"
    process, url, log = start_service(args, base, out)
    by_cli: dict[str, list[dict]] = {cli.name: [] for cli in clis}
    raw = out / "runs.jsonl"
    try:
        for run in range(1, args.runs + 1):
            # Each round starts one CLI later: whoever runs right after the
            # last round's heaviest process (a 1 GB node exiting) starts
            # slower, measured as ~55 ms on the first composer of a round.
            shift = (run - 1) % len(clis)
            for cli in clis[shift:] + clis[:shift]:
                wait_for_calm(args.calm, args.calm_wait)
                time.sleep(args.settle)
                started = time.monotonic()
                result = run_once(cli, run, args, url, log, base)
                result["version"] = cli.version
                result["load1"] = load1()
                result["note"] = args.note
                result["wall_s"] = round(time.monotonic() - started, 1)
                by_cli[cli.name].append(result)
                with open(raw, "a", encoding="utf-8") as handle:
                    handle.write(json.dumps(result) + "\n")
                if log.find("foreign-credential"):
                    raise SystemExit(f"{cli.name} sent a credential that is not the benchmark's dummy key; "
                                     "stopped (the value was not recorded)")
                status = result.get("error") or ",".join(result.get("errors", [])) or "ok"
                print(f"[{cli.name} run {run}] {status} · load {result['load1']:.1f} · "
                      f"{result['wall_s']} s", flush=True)
    finally:
        process.terminate()
        process.wait(timeout=10)
        shutil.rmtree(base, ignore_errors=True)
    versions = " · ".join(f"{cli.name}: {cli.version}" for cli in clis)
    rendered = table(by_cli) + f"\nversions: {versions}\nuptime at end: {uptime()}\n"
    if args.note:
        rendered += f"conditions: {args.note}\n"
    (out / "table.md").write_text(rendered)
    print(rendered)
    print(f"results: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
