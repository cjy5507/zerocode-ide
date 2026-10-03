#!/usr/bin/env python3
"""Measure opt-in Rust workflow replays using an already-built tools test binary.

Build from zo-ide with `cargo test -p tools --lib --no-run --message-format=json`.
Pass its JSON output with --build-log. Results belong outside version control.
No model API is contacted: both fixtures use isolated settings and loopback replies.
"""

import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import sys
import time


DECISION_RULE = (
    "Fixed-response replay measures implementation impact only. Keep existing "
    "defaults and Auto evidence requirements regardless of apparent savings. "
    "Report incorrect loads, lost facts, recovery failures and fallbacks without "
    "excluding them. No model-quality, cache-hit, latency or billing claim is "
    "supported without independent real-provider measurements."
)


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def executable(build_log):
    found = []
    for line in build_log.read_text().splitlines():
        row = json.loads(line)
        if (row.get("reason") == "compiler-artifact"
                and row.get("target", {}).get("name") == "tools"
                and row.get("profile", {}).get("test") and row.get("executable")):
            found.append(row["executable"])
    if len(set(found)) != 1:
        raise ValueError("build log must identify exactly one tools test executable")
    path = Path(found[0])
    if not path.is_file():
        raise ValueError("build-log executable no longer exists; rebuild the fixture")
    return path


def cells():
    for size, needed, phase in itertools.product(
            ("below", "near", "above"), (0, 25, 100), ("cold", "warm")):
        yield {"kind": "skills", "size": size, "neededPercent": needed, "phase": phase}
    for scenario, phase in itertools.product(
            ("normal", "focus", "no_store", "headroom", "image", "payback"), ("cold", "warm")):
        yield {"kind": "retention", "scenario": scenario, "phase": phase}


def run_cell(binary, cell, output):
    env = os.environ.copy()
    for key in ("ZO_TRACE_ROOT", "ZO_DISABLE_RAW_VAULT", "ZO_COMPACTION_MODEL", "ZO_TEAM_INBOX_STORE"):
        env.pop(key, None)
    env.update(JEV_IMPACT_MODE=cell["mode"], JEV_IMPACT_PHASE=cell["phase"], ZO_DISABLE_KEYCHAIN="1")
    env["ZO_COMPACT_CACHED_PREFIX"] = "1"
    if cell["kind"] == "skills":
        env.update(JEV_IMPACT_SIZE=cell["size"], JEV_IMPACT_NEEDED_PERCENT=str(cell["neededPercent"]))
        test = "misc_tools::smart_router::skills_impact::measure_skill_workflow_impact"
    else:
        env["JEV_IMPACT_SCENARIO"] = cell["scenario"]
        test = "misc_tools::smart_router::retention_impact::measure_retention_workflow_impact"
    command = [str(binary), "--exact", test, "--ignored", "--nocapture", "--test-threads=1"]
    # A fresh process per sample makes the peak RSS attribution explicit.
    if sys.platform == "darwin":
        command = ["/usr/bin/time", "-l", *command]
    else:
        command = ["/usr/bin/time", "-f", "JEV_RSS_KIB=%M", *command]
    began = time.monotonic_ns()
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=90)
    elapsed = (time.monotonic_ns() - began) // 1000
    output.with_suffix(".stdout").write_text(result.stdout)
    output.with_suffix(".stderr").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(f"fixture failed ({result.returncode}); inspect {output.name}.stderr")
    matches = re.findall(r"JEV_IMPACT_JSON=(\{[^\n]+\})", result.stdout)
    if len(matches) != 1:
        raise ValueError(f"expected exactly one measurement in {output.name}")
    rss = (re.search(r"(\d+)\s+maximum resident set size", result.stderr)
           if sys.platform == "darwin" else re.search(r"JEV_RSS_KIB=(\d+)", result.stderr))
    if rss is None:
        raise ValueError("time did not report peak RSS")
    measurement = json.loads(matches[0])
    if cell["kind"] == "retention":
        rounds = measurement["rounds"]
        measurement["totals"] = {
            key: sum(row[key] for row in rounds)
            for key in ("boundaryHostMicros", "workflowHostMicros", "recoveryHostMicros", "recallCalls", "recallBytes", "summaryRequests")}
        measurement["totals"].update(
            retainedRounds=sum(row["retained"] for row in rounds),
            recoveredToolFacts=sum(row["toolFactRecovered"] for row in rounds),
            resumeFailures=sum(not row["coldResumeEqual"] for row in rounds),
            pairFailures=sum(not row["toolPairsValid"] for row in rounds),
            liveFacts={key: sum(row["factsLive"][key] for row in rounds)
                       for key in rounds[0]["factsLive"]},
            fallbackReasons={reason: sum((row["eligibility"] or {}).get("reason") == reason for row in rounds)
                             for reason in ("focus", "streak", "no_persistence", "headroom", "images", "payback", "no_applied_drop", "retained")})
    return {"cell": cell, "measurement": measurement,
            "processMicros": elapsed, "peakRssBytes": int(rss[1]) * (1 if sys.platform == "darwin" else 1024)}


def numbers(value, prefix=""):
    if isinstance(value, dict):
        for key, item in value.items():
            yield from numbers(item, f"{prefix}.{key}" if prefix else key)
    elif isinstance(value, (float, int)) and not isinstance(value, bool):
        yield prefix, value


def aggregate(samples):
    groups = {}
    for sample in samples:
        cell = {k: v for k, v in sample["cell"].items() if k != "repetition"}
        key = json.dumps(cell, sort_keys=True)
        group = groups.setdefault(key, {"cell": cell, "samples": 0, "metrics": {}})
        group["samples"] += 1
        for name, value in numbers({k: v for k, v in sample.items() if k != "cell"}):
            group["metrics"].setdefault(name, []).append(value)
    for group in groups.values():
        group["metrics"] = {key: {"median": statistics.median(values),
                                  "min": min(values), "max": max(values)}
                            for key, values in group["metrics"].items()}
    return list(groups.values())


def compare(groups):
    """Paired cell comparisons; never convert byte estimates into a money claim."""
    index = {json.dumps(row["cell"], sort_keys=True): row for row in groups}
    comparisons = []
    for row in groups:
        cell = row["cell"]
        if cell["mode"] != "on":
            continue
        paired = {mode: index[json.dumps(dict(cell, mode=mode), sort_keys=True)]
                  for mode in ("off", "shadow", "on")}
        metrics = ("measurement.sessionPromptBytes", "measurement.jevRequestBytes", "peakRssBytes",
                   "measurement.foregroundMicros", "measurement.scriptedFrontierRequests") if cell["kind"] == "skills" else (
                       "measurement.jevRequestBytes", "measurement.summaryRequests", "peakRssBytes",
                       "measurement.totals.workflowHostMicros", "measurement.totals.recallCalls",
                       "measurement.summaryRequestBytes", "measurement.probeRequestBytes")
        comparison = {"cell": {k: v for k, v in cell.items() if k != "mode"},
                      "medians": {metric: {mode: paired[mode]["metrics"][metric]["median"]
                                           for mode in paired} for metric in metrics}}
        if cell["kind"] == "skills":
            # This treats both request classes as equal bytes only for a visible
            # volume comparison. Different tokenizers/prices make it no bill.
            comparison["combinedRequestByteProxy"] = {
                mode: sum(paired[mode]["metrics"][metric]["median"] for metric in (
                    "measurement.sessionPromptBytes", "measurement.jevRequestBytes")) for mode in paired}
            comparison["onBelowOffForRequestByteProxy"] = (
                comparison["combinedRequestByteProxy"]["on"] < comparison["combinedRequestByteProxy"]["off"])
        comparisons.append(comparison)
    return comparisons


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-log", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--pilot", action="store_true", help="one cold sample per cell, no statistical claim")
    args = parser.parse_args()
    binary = executable(args.build_log)
    args.output.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[2]
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    diff = subprocess.check_output(["git", "diff", "HEAD", "--"], cwd=root)
    metadata = {"head": head, "workingDiffSha256": hashlib.sha256(diff).hexdigest(),
                "executableSha256": file_hash(binary),
                "profile": "native optimized test profile; not release",
                "repetitions": 1 if args.pilot else 5, "decisionRule": DECISION_RULE,
                "tokens": "estimated from bytes/4; provider-reported usage remains null",
                "time": "host wall time with fixed local replies; model wait is unmeasured",
                "rss": "whole fresh process peak, including fixture setup and warmup when warm",
                "coldWarm": "fresh process versus a complete warmup session in that process",
                "skillsPolicy": "twelve distinct turn inputs; a scripted oracle chooses the tool road and need. Prompt/history bytes are a normalized proxy, not a captured provider request",
                "retentionPolicy": "actual ConversationRuntime requests lowered through convert_messages, with fixed faithful model replies",
                "isolation": "temporary config, workspace, journal and loopback server per process"}
    # Persist the decision rule before the first sample.
    (args.output / "plan.json").write_text(json.dumps(metadata, indent=2) + "\n")
    samples = []
    with (args.output / "samples.jsonl").open("w") as stream:
        for repetition in range(metadata["repetitions"]):
            modes = ("off", "shadow", "on") if repetition % 2 == 0 else ("on", "shadow", "off")
            for cell in cells():
                if args.pilot and cell["phase"] == "warm":
                    continue
                for mode in modes:
                    current = dict(cell, mode=mode, repetition=repetition)
                    name = f"sample-{len(samples):04d}"
                    sample = run_cell(binary, current, args.output / name)
                    samples.append(sample)
                    stream.write(json.dumps(sample, sort_keys=True) + "\n")
                    stream.flush()
                    print(f"{name} {json.dumps(current, sort_keys=True)}", flush=True)
    groups = aggregate(samples)
    report = {"metadata": metadata, "cells": groups, "comparisons": compare(groups)}
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
