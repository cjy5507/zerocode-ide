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


def summarize(report):
    series = []
    for row in report["rows"]:
        ns = row["ns"]
        line = {
            "series": row["series"],
            "n": len(ns),
            "p50_ms": rank(ns, 0.5) / 1e6,
            "p95_ms": rank(ns, 0.95) / 1e6,
            "max_ms": max(ns) / 1e6,
            "work": row["work"],
            "load": [row["load_before"], row["load_after"]],
        }
        for key in ("scene", "detectors", "frame"):
            if key in row:
                line[key] = row[key]
        if "cold_ns" in row:
            line["cold_ms"] = row["cold_ns"] / 1e6
        if row["work"]:
            line["ns_per_sample_p50"] = rank(ns, 0.5) / row["work"]
            line["ns_per_sample_p95"] = rank(ns, 0.95) / row["work"]
        series.append(line)
    cells = [line for line in series if line["series"] == "cells"]
    worst = max(cells, key=lambda line: line["p95_ms"]) if cells else None
    return {
        "limits": report["limits"],
        "series": series,
        "cells_scenes": len(cells),
        "cells_worst_p95_ms": worst["p95_ms"] if worst else None,
        "cells_worst_scene": worst.get("scene") if worst else None,
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
