"""Owned, offline native decision process. The parent owns the HTTP listener."""

import importlib.metadata
import json
import os
import queue
import sys
import threading

MAX_FRAME = 2 * 1024 * 1024
wire = sys.stdout.buffer
sys.stdout = sys.stderr
messages = queue.Queue(maxsize=2)


def read_parent():
    while True:
        line = sys.stdin.buffer.readline(MAX_FRAME + 1)
        if not line:
            os._exit(0)
        if len(line) > MAX_FRAME or not line.endswith(b"\n"):
            os._exit(1)
        try:
            messages.put(json.loads(line), block=False)
        except (ValueError, queue.Full):
            os._exit(1)


def reply(value):
    encoded = json.dumps(value, separators=(",", ":"), allow_nan=False).encode("utf-8")
    if len(encoded) >= MAX_FRAME:
        raise ValueError("size_limit")
    wire.write(encoded + b"\n")
    wire.flush()


threading.Thread(target=read_parent, daemon=True).start()

try:
    initial = messages.get(timeout=10)
    if initial["engine"] != "laya":
        raise ValueError("unsupported_engine")
    for package, version in initial["packages"].items():
        if importlib.metadata.version(package) != version:
            raise ValueError("runtime_version")

    import torch
    from laya import Agent

    torch.set_num_threads(2)
    torch.set_num_interop_threads(1)

    def refuse_pickle(*args, **kwargs):
        raise ValueError("pickle_disabled")

    torch.load = refuse_pickle
    agent = None
    if initial["mode"] == "model":
        agent = Agent(initial["modelDirectory"], device="cpu", compile=False)
    reply({"ready": True})
except Exception:
    reply({"error": "runtime_failed"})
    os._exit(1)

while True:
    request = messages.get()
    try:
        if agent is None:
            raise ValueError("model_missing")
        questions = list(request["questions"].items())
        answers = {}
        usage = {"input_tokens": 0, "output_tokens": 0}
        for start in range(0, len(questions), 4):
            native = agent.predict(request["state"], dict(questions[start:start + 4]))
            answers.update(native["answers"])
            measured = native["usage"]
            for key in ("input_tokens", "output_tokens", "state_tokens_dropped"):
                if key in measured:
                    usage[key] = usage.get(key, 0) + measured[key]
            if measured.get("truncated"):
                usage["truncated"] = True
            if measured.get("truncated_questions"):
                usage.setdefault("truncated_questions", []).extend(measured["truncated_questions"])
            if measured.get("options"):
                usage.setdefault("options", {}).update(measured["options"])
        result = {"model": initial["modelId"], "answers": answers, "usage": usage}
        reply(result)
    except Exception:
        reply({"error": "native_protocol_failed"})
