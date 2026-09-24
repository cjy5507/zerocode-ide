#!/usr/bin/env python3
"""Summarises a probe measurement: per series, nearest-rank p50/p95 and the maximum in
milliseconds, the first observation apart, and nanoseconds per sample read.

    python3 tools/game-state-probe/summarize.py <measure.json> [--output <summary.json>]
"""
import argparse
import json
import math
from pathlib import Path


def rank(values, fraction):
    """The nearest-rank quantile: the smallest value at or above `fraction` of the samples."""
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def line(name, rows):
    """One series under one QoS: every block's warm samples pooled; each block's load kept."""
    ns = [value for row in rows for value in row["ns"]]
    work = max(row["work"] for row in rows)
    summary = {
        "series": name,
        "qos": rows[0].get("qos"),
        "n": len(ns),
        "blocks": sorted({row.get("block") for row in rows}, key=str),
        "p50_ms": rank(ns, 0.5) / 1e6,
        "p95_ms": rank(ns, 0.95) / 1e6,
        "max_ms": max(ns) / 1e6,
        "work": work,
        "load": [[row["load_before"], row["load_after"]] for row in rows],
    }
    for key in ("detectors", "frame"):
        if key in rows[0]:
            summary[key] = rows[0][key]
    colds = [row["cold_ns"] for row in rows if "cold_ns" in row]
    if colds:
        summary["cold_max_ms"] = max(colds) / 1e6
    if work:
        summary["ns_per_sample_p50"] = rank(ns, 0.5) / work
        summary["ns_per_sample_p95"] = rank(ns, 0.95) / work
    return summary


def summarize(report):
    groups = {}
    for row in report["rows"]:
        groups.setdefault((row["series"], row.get("qos")), []).append(row)
    series = [line(name, rows) for (name, _), rows in groups.items()]
    worst = {}
    for row in report["rows"]:
        if row["series"] == "cells":
            p95 = rank(row["ns"], 0.95) / 1e6
            if p95 > worst.get(row.get("qos"), (0, None))[0]:
                worst[row.get("qos")] = (p95, row.get("scene"))
    return {
        "limits": report["limits"],
        "series": series,
        "cells_worst_scene_p95_ms": {str(qos): value for qos, value in worst.items()},
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("report")
    parser.add_argument("--output")
    arguments = parser.parse_args()
    summary = summarize(json.loads(Path(arguments.report).read_text()))
    text = json.dumps(summary, indent=1, sort_keys=True)
    if arguments.output:
        Path(arguments.output).write_text(text + "\n")
    print(text)


if __name__ == "__main__":
    main()
