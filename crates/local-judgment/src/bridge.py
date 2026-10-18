"""Owned, offline native decision process. The parent owns the HTTP listener."""

import importlib.metadata
import hashlib
import json
import math
import os
from pathlib import Path
import pickle
import queue
import sys
import threading
import time

sys.dont_write_bytecode = True

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
            # engine.rs MEMORY_LIMIT_EXIT: the parent keeps this end as the guard's stop.
            os._exit(75)
        time.sleep(0.25)


class EncodingRecord:
    def __init__(self, tokenizer):
        self.tokenizer = tokenizer
        self.whole = []

    def __getattr__(self, name):
        return getattr(self.tokenizer, name)

    def __call__(self, text, **kwargs):
        encoded = self.tokenizer(text, **kwargs)
        uncapped = {key: value for key, value in kwargs.items() if key not in ("truncation", "max_length")}
        whole = encoded if uncapped == kwargs else self.tokenizer(text, **uncapped)
        self.whole.append(list(whole["input_ids"]))
        return encoded


def measured_head(build_head, tokenizer, question, head_max_len, *args, **kwargs):
    # A record per head: the runtime reuses a text's ids only for a tokenizer object it has
    # already seen in the call, so every text of this head reaches this record.
    record = EncodingRecord(tokenizer)
    head = build_head(record, question, head_max_len, *args, **kwargs)
    ids, markers = head[0], head[1]
    stops = list(markers) + [len(ids) - 1]
    spans = [ids[1:stops[0] - 1]] + [ids[start + 1:stop] for start, stop in zip(markers, stops[1:])]
    return head, spans == record.whole


class HeadMeasure:
    def __init__(self, common):
        self.build_head = common.build_head
        self.heads = None
        common.build_head = self.measured

    def measured(self, tokenizer, question, head_max_len, *args, **kwargs):
        head, whole = measured_head(self.build_head, tokenizer, question, head_max_len, *args, **kwargs)
        if self.heads is not None:
            self.heads.append((question, whole))
        return head

    def during(self, call):
        self.heads = []
        try:
            return call(), self.heads
        finally:
            self.heads = None


def declared_limits(initial, tokens):
    model, runtime = initial.get("modelId"), initial.get("runtime")
    if not all(isinstance(name, str) and name for name in (model, runtime)):
        return None
    if not all(type(count) is int and count > 0 for count in tokens.values()):
        return None
    return {"model": model, "runtime": runtime, "tokens": tokens}


class LatencyMeasure:
    def __init__(self, agent, measure, fits, head_budget, batch_size):
        self.agent = agent
        self.measure = measure
        self.fits = fits
        self.head_budget = head_budget
        self.batch_size = batch_size
        self.maximum = agent.cfg['max_len']
        self.fixed = 0.0
        self.per_token = 0.0
        self.samples = []
        probe = {'type': 'noul', 'instructions': 'Calibration.',
                 'criteria': {'false': 'False', 'true': 'True'}}
        if not fits(probe) or type(self.maximum) is not int or self.maximum <= 0:
            raise ValueError('deadline_unmeasured')
        short = {'probe': probe}
        lower, upper = 0, self.maximum
        while lower < upper:
            middle = (lower + upper + 1) // 2
            items = self.items('calibration ' * middle, short)
            if any(item['state_stats']['state_tokens_dropped'] for item in items):
                upper = middle - 1
            else:
                lower = middle
        states = (('', 'short'), ('calibration ' * lower, 'long'))
        self.agent.predict('', short)
        for state, label in states:
            for count in (1, batch_size):
                questions = {'probe:%d' % index: probe for index in range(count)}
                lengths = [len(item['ids']) for item in self.items(state, questions)]
                began = time.perf_counter()
                native = self.agent.predict(state, questions)
                elapsed = time.perf_counter() - began
                if elapsed <= 0 or native['usage'].get('truncated'):
                    raise ValueError('deadline_unmeasured')
                self.samples.append({'state': label, 'questions': count,
                                     'lengths': lengths, 'seconds': elapsed})
                if label == 'short':
                    self.fixed = max(self.fixed, elapsed / count)
                else:
                    self.per_token = max(self.per_token, elapsed / self.work(lengths))
        record = EncodingRecord(agent.tok)
        head = measure.build_head(record, agent._to_internal(probe), head_budget)
        self.wrapping = len(head[0]) - sum(len(ids) for ids in record.whole) - len(head[1])
        self.state_suffix = len(self.items('', short)[0]['ids']) - len(head[0])

    def items(self, state, questions):
        return self.agent._encode_state(
            state, list(questions),
            {name: self.agent._to_internal(question) for name, question in questions.items()},
        )

    def work(self, lengths):
        return sum(len(lengths[start:start + self.batch_size]) * max(lengths[start:start + self.batch_size])
                   for start in range(0, len(lengths), self.batch_size))

    def estimate(self, lengths):
        return self.fixed * len(lengths) + self.per_token * self.work(lengths)

    def observe(self, lengths, elapsed):
        variable = max(0.0, elapsed - self.fixed * len(lengths))
        work = self.work(lengths)
        if work:
            self.per_token = max(self.per_token, variable / work)

    def future_count(self, question, pages):
        if len(pages) < 2:
            return 0
        internal = self.agent._to_internal(question)
        record = EncodingRecord(self.agent.tok)
        self.measure.build_head(record, internal, self.head_budget)
        ordered = sorted(zip(internal['crit'].items(), record.whole[1:]),
                         key=lambda item: len(item[1]), reverse=True)
        capacity = 1
        for size in range(2, len(ordered) + 1):
            criteria = dict(pair for pair, tokens in ordered[:size])
            if not self.fits(dict(question, criteria=criteria)):
                break
            capacity = size
        remaining = len(pages)
        if capacity < 2:
            return remaining * (remaining - 1) // 2
        future = 0
        while remaining > 1:
            remaining = (remaining + capacity - 1) // capacity
            future += remaining
        return future

    def plan(self, state, questions, pages):
        flattened = {}
        future = 0
        for name, question in questions.items():
            groups = pages[name]
            for index, group in enumerate(groups):
                flattened[name + ':' + str(index)] = group
            future += self.future_count(question, groups)
        items = self.items(state, flattened)
        lengths = [len(item['ids']) for item in items]
        state_tokens = items[0]['state_stats']['state_tokens']
        longest = min(self.maximum, self.head_budget + self.wrapping + state_tokens + self.state_suffix)
        return self.estimate(lengths) + future * (self.fixed + self.per_token * longest)


class LayaAdapter:
    batch_size = 4

    def __init__(self, initial):
        from laya import Agent, common

        self.agent = Agent(initial["modelDirectory"], device="cpu", compile=False)
        self.measure = HeadMeasure(common)
        loaded = getattr(self.agent, "cfg", None)
        loaded = loaded if isinstance(loaded, dict) else {}
        self.limits = declared_limits(
            initial, {"max_len": loaded.get("max_len"), "head_max_len": loaded.get("head_max_len")}
        )
        budget = loaded.get("head_max_len")
        self.head_budget = budget if type(budget) is int and budget > 0 else None
        self.latency = None
        if initial.get("measure_deadlines") and hasattr(self.agent, "_encode_state"):
            try:
                self.latency = LatencyMeasure(self.agent, self.measure, self.fits, self.head_budget, self.batch_size)
            except (AttributeError, KeyError, TypeError, ValueError):
                self.latency = None

    def fits(self, question):
        if self.head_budget is None:
            return False
        internal = self.agent._to_internal(question)
        _, whole = measured_head(
            self.measure.build_head, self.agent.tok, internal, self.head_budget,
            option_order=internal.get("option_order"),
        )
        return whole

    def pages(self, question):
        if self.head_budget is None or self.fits(question):
            return [question]
        if question.get("type") != "choice" or "option_order" in question:
            return [question]
        internal = self.agent._to_internal(question)
        criteria = internal["crit"]
        record = EncodingRecord(self.agent.tok)
        self.measure.build_head(record, internal, self.head_budget)
        options = [tuple(encoded) for encoded in record.whole[1:]]
        if len(set(options)) != len(options):
            return [question]
        pages, current = [], {}
        for label, description in criteria.items():
            extended = dict(current, **{label: description})
            if self.fits(dict(question, criteria=extended)):
                current = extended
                continue
            if not current:
                return [question]
            pages.append(dict(question, criteria=current))
            current = {label: description}
            if not self.fits(dict(question, criteria=current)):
                return [question]
        if current:
            pages.append(dict(question, criteria=current))
        return pages if len(pages) < len(criteria) else [question]

    def merge_page(self, question, answer, clusters):
        labels = self.agent._to_internal(question)["crit"]
        probabilities = answer.get("probabilities")
        if answer.get("type") != "choice" or not isinstance(probabilities, dict) or probabilities.keys() != labels.keys():
            raise ValueError("page_answer")
        if any(type(chance) not in (int, float) or not math.isfinite(chance) or not 0 <= chance <= 1
               for chance in probabilities.values()):
            raise ValueError("page_answer")
        if abs(sum(probabilities.values()) - 1) > len(labels) * 0.000051 + 0.000001:
            raise ValueError("page_answer")
        winner = answer.get("choice")
        chance = probabilities.get(winner, 0)
        if chance <= 0 or chance < max(probabilities.values()):
            raise ValueError("page_answer")
        weights = {}
        for label, probability in probabilities.items():
            for original, weight in clusters[label]["weights"].items():
                weights[original] = weight * (probability / chance)
        confidence = answer.get("confidence")
        answer_confidence = answer.get("answer_confidence", confidence)
        if any(type(value) not in (int, float) or not math.isfinite(value) or not 0 <= value <= 1
               for value in (confidence, answer_confidence)):
            raise ValueError("page_answer")
        return winner, {
            "weights": weights,
            "confidence": min(clusters[winner]["confidence"], confidence, chance),
            "answer_confidence": min(clusters[winner]["answer_confidence"], answer_confidence, chance),
        }

    def budget_refusal(self, usage, budget_ms, reason, estimated=None):
        usage["deadline_budget_exceeded"] = True
        usage["deadline_budget_ms"] = budget_ms
        usage["deadline_budget_reason"] = reason
        if estimated is not None:
            usage["estimated_ms"] = max(0.0, estimated * 1000)
        return {"answers": {}, "usage": usage}

    def predict(self, request, budget_ms=None):
        began = time.perf_counter()
        if budget_ms is not None and (type(budget_ms) is not int or budget_ms < 0):
            raise ValueError("deadline_budget")
        deadline = None if budget_ms is None else began + budget_ms / 1000
        answers = {}
        usage = {"input_tokens": 0, "output_tokens": 0, "model_calls": 0, "head_pages": {}}
        named = []
        pending = {}
        for name, question in request["questions"].items():
            criteria = self.agent._to_internal(question).get("crit") if question.get("type") == "choice" else {}
            criteria = criteria if isinstance(criteria, dict) else {}
            pending[name] = {
                "question": question,
                "clusters": {label: {"weights": {label: 1.0}, "confidence": 1.0, "answer_confidence": 1.0}
                             for label in criteria},
                "partitioned": False,
            }
            usage["head_pages"][name] = {"pages": 0, "rounds": 0, "partitioned": False}
        estimated = None
        if deadline is not None:
            initial_pages = {name: self.pages(question) for name, question in request["questions"].items()}
            for name, groups in initial_pages.items():
                usage["head_pages"][name]["partitioned"] = len(groups) > 1
                pending[name]["initial_pages"] = groups
            if self.head_budget is not None:
                cut = [name for name, groups in initial_pages.items() if not all(self.fits(question) for question in groups)]
                if cut:
                    usage["truncated_questions"] = cut
                    return {"answers": {}, "usage": usage}
            if self.latency is None:
                return self.budget_refusal(usage, budget_ms, "unmeasured")
            estimated = self.latency.plan(request["state"], request["questions"], initial_pages)
            usage["estimated_ms"] = estimated * 1000
            usage["deadline_budget_ms"] = budget_ms
            if time.perf_counter() + estimated >= deadline:
                return self.budget_refusal(usage, budget_ms, "estimated", estimated)
        while pending:
            tasks = []
            used = set(request["questions"])
            for name, progress in pending.items():
                pages = progress.pop("initial_pages", None) or self.pages(progress["question"])
                progress["partitioned"] = progress["partitioned"] or len(pages) > 1
                progress["results"] = []
                details = usage["head_pages"][name]
                details["pages"] += len(pages)
                details["rounds"] += 1
                details["partitioned"] = progress["partitioned"]
                for index, page in enumerate(pages):
                    identifier = name if len(pages) == 1 else "%s:%d" % (name, index)
                    if len(pages) > 1:
                        while identifier in used:
                            identifier += ":"
                    used.add(identifier)
                    tasks.append((identifier, name, page))
            failed = set()
            for start in range(0, len(tasks), self.batch_size):
                group = tasks[start:start + self.batch_size]
                batch = {identifier: question for identifier, _, question in group}
                lengths = [len(item["ids"]) for item in self.latency.items(request["state"], batch)] if self.latency is not None else None
                if deadline is not None:
                    estimated = self.latency.estimate(lengths)
                    if time.perf_counter() + estimated >= deadline:
                        return self.budget_refusal(usage, budget_ms, "estimated", estimated)
                started = time.perf_counter()
                native, heads = self.measure.during(lambda: self.agent.predict(request["state"], batch))
                elapsed = time.perf_counter() - started
                usage["model_calls"] += 1
                if self.latency is not None:
                    self.latency.observe(lengths, elapsed)
                measured = native["usage"]
                for key in ("input_tokens", "output_tokens", "state_tokens_dropped"):
                    if key in measured:
                        usage[key] = usage.get(key, 0) + measured[key]
                if measured.get("truncated"):
                    usage["truncated"] = True
                truncated = measured.get("truncated_questions") or ()
                if any(identifier not in batch for identifier in truncated):
                    raise ValueError("head_unmeasured")
                for identifier, name, question in group:
                    internal = self.agent._to_internal(question)
                    kept = [whole for seen, whole in heads if seen == internal]
                    if not kept:
                        raise ValueError("head_unmeasured")
                    if not all(kept) or identifier in truncated:
                        named.append(name)
                        failed.add(name)
                    pending[name]["results"].append((question, native["answers"][identifier]))
                if measured.get("options"):
                    usage.setdefault("options", {}).update(measured["options"])
                if deadline is not None and time.perf_counter() >= deadline:
                    return self.budget_refusal(usage, budget_ms, "elapsed", estimated)
            following = {}
            for name, progress in pending.items():
                results = progress["results"]
                if name in failed or not progress["partitioned"]:
                    answers[name] = results[0][1]
                    continue
                winners = {}
                for question, answer in results:
                    winner, cluster = self.merge_page(question, answer, progress["clusters"])
                    winners[winner] = cluster
                if len(results) == 1:
                    winner, cluster = next(iter(winners.items()))
                    total = sum(cluster["weights"].values())
                    probabilities = {label: cluster["weights"][label] / total
                                     for label in self.agent._to_internal(request["questions"][name])["crit"]}
                    final = dict(results[0][1], probabilities=probabilities)
                    final["confidence"] = min(cluster["confidence"], probabilities[winner])
                    final["answer_confidence"] = min(cluster["answer_confidence"], probabilities[winner])
                    answers[name] = final
                else:
                    criteria = self.agent._to_internal(progress["question"])["crit"]
                    progress["question"] = dict(progress["question"], criteria={label: criteria[label] for label in winners})
                    progress["clusters"] = winners
                    following[name] = progress
            pending = following
        if named:
            usage["truncated_questions"] = list(dict.fromkeys(named))
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
        self.limits = declared_limits(initial, {"max_len": getattr(self.model, "max_length", None)})

    def predict(self, request, budget_ms=None):
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
    ready = {"ready": True}
    if getattr(adapter, "limits", None):
        ready["limits"] = adapter.limits
    reply(ready)
except Exception:
    reply({"error": "runtime_failed"})
    os._exit(1)

while True:
    envelope = messages.get()
    try:
        request, budget_ms = envelope, None
        if isinstance(envelope, dict) and set(envelope) == {"request", "budget_ms"}:
            request, budget_ms = envelope["request"], envelope["budget_ms"]
        if adapter is None:
            raise ValueError("model_missing")
        if "images" in request or "videos" in request:
            raise ValueError("input_not_supported")
        started = time.perf_counter()
        result = adapter.predict(request, budget_ms=budget_ms)
        result["usage"]["processing_ms"] = (time.perf_counter() - started) * 1000
        result["model"] = initial["modelId"]
        reply(result)
    except Exception:
        reply({"error": "native_protocol_failed"})
