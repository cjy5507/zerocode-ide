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
        result = agent.predict(request["state"], request["questions"])
        result["model"] = initial["modelId"]
        reply(result)
    except Exception:
        reply({"error": "native_protocol_failed"})
