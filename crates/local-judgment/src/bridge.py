"""Owned, offline native decision process. The parent owns the HTTP listener."""

import importlib.metadata
import hashlib
import json
import os
from pathlib import Path
import pickle
import queue
import sys
import threading
import time

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


def offline_process(event, arguments):
    if event in ("socket.connect", "socket.getaddrinfo", "subprocess.Popen", "os.system", "os.posix_spawn"):
        raise ValueError("offline_process")


def refuse_pickle(*args, **kwargs):
    raise ValueError("pickle_disabled")


def refuse_dynamic_code(*args, **kwargs):
    raise ValueError("unreviewed_code_disabled")


def memory_guard(process, limit):
    while True:
        try:
            over_limit = process.memory_info().rss > limit
        except Exception:
            os._exit(1)
        if over_limit:
            os._exit(1)
        time.sleep(0.25)


class LayaAdapter:
    def __init__(self, initial):
        from laya import Agent

        self.agent = Agent(initial["modelDirectory"], device="cpu", compile=False)

    def predict(self, request):
        questions = list(request["questions"].items())
        answers = {}
        usage = {"input_tokens": 0, "output_tokens": 0}
        for start in range(0, len(questions), 4):
            native = self.agent.predict(request["state"], dict(questions[start:start + 4]))
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
        return {"answers": answers, "usage": usage}


def reviewed_system_one_runtime():
    sys.modules["d3_fast"] = None
    import d3_runtime

    d3_runtime.fast_module = lambda root: None
    d3_runtime.load_file_module = refuse_dynamic_code
    d3_runtime.resolve_dir = refuse_dynamic_code
    d3_runtime._download = refuse_dynamic_code
    d3_runtime.vision_weights_present = lambda root: False
    return d3_runtime


def verify_model_data(initial):
    root = Path(initial["modelDirectory"]).resolve(strict=True)
    for artifact in initial["modelFiles"]:
        path = root / artifact["path"]
        if not path.resolve(strict=True).is_relative_to(root) or path.is_symlink() or not path.is_file():
            raise ValueError("unsafe_path")
        if path.stat().st_size != artifact["bytes"]:
            raise ValueError("integrity_failed")
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        if digest.hexdigest() != artifact["sha256"]:
            raise ValueError("integrity_failed")
    return root


class TransformersSystemOneAdapter:
    def __init__(self, initial):
        runtime = reviewed_system_one_runtime()
        root = verify_model_data(initial)
        self.model = runtime.D3(root, device="cpu", batch_size=2, max_length=1024, model_name=initial["modelId"])

    def predict(self, request):
        return self.model.system_one(state=request["state"], questions=request["questions"])


def configure_loaders(initial):
    from transformers import AutoConfig, AutoTokenizer

    config_loader = AutoConfig.from_pretrained
    tokenizer_loader = AutoTokenizer.from_pretrained

    def offline_config(*args, **kwargs):
        kwargs.update(local_files_only=True, trust_remote_code=False)
        return config_loader(*args, **kwargs)

    def offline_tokenizer(*args, **kwargs):
        kwargs.update(local_files_only=True, trust_remote_code=False)
        tokenizer = tokenizer_loader(*args, **kwargs)
        if initial["engine"] == "transformers_system_one":
            tokenizer.chat_template = initial["chatTemplate"]
        return tokenizer

    AutoConfig.from_pretrained = offline_config
    AutoTokenizer.from_pretrained = offline_tokenizer


ADAPTERS = {"laya": LayaAdapter, "transformers_system_one": TransformersSystemOneAdapter}


threading.Thread(target=read_parent, daemon=True).start()
sys.addaudithook(offline_process)

try:
    initial = messages.get(timeout=10)
    if initial["engine"] not in ADAPTERS:
        raise ValueError("unsupported_engine")
    for package, version in initial["packages"].items():
        if importlib.metadata.version(package) != version:
            raise ValueError("runtime_version")
    if initial.get("rssLimitBytes"):
        import psutil

        threading.Thread(target=memory_guard, args=(psutil.Process(), initial["rssLimitBytes"]), daemon=True).start()

    import torch
    configure_loaders(initial)

    torch.set_num_threads(2)
    torch.set_num_interop_threads(1)

    torch.load = refuse_pickle
    torch.jit.load = refuse_pickle
    pickle.load = refuse_pickle
    pickle.loads = refuse_pickle
    adapter = None
    if initial["mode"] == "model":
        adapter = ADAPTERS[initial["engine"]](initial)
    elif initial["engine"] == "transformers_system_one":
        reviewed_system_one_runtime()
    reply({"ready": True})
except Exception:
    reply({"error": "runtime_failed"})
    os._exit(1)

while True:
    request = messages.get()
    try:
        if adapter is None:
            raise ValueError("model_missing")
        if "images" in request or "videos" in request:
            raise ValueError("input_not_supported")
        result = adapter.predict(request)
        result["model"] = initial["modelId"]
        reply(result)
    except Exception:
        reply({"error": "native_protocol_failed"})
