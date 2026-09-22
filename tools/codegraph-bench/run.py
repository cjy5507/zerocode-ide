#!/usr/bin/env python3
"""What the codegraph index costs on this repository (t-5970).

Snapshots a checkout's HEAD (`git archive`, so the measured tree is exactly
the committed one and never the checkout itself), builds the `index_cost`
bench once, runs each phase as its own process under `/usr/bin/time` for the
peak resident size, and prints one table. The bench phases go through the
codegraph crate's public API only, so the same run measures the index before
and after a redesign behind that API.

    tools/codegraph-bench/run.py --scratch <dir> --rev <sha> --label before --out before.json
    tools/codegraph-bench/run.py --scratch <dir> --rev <sha> --label after --out after.json \
        --compare before.json

The scratch directory is emptied and refilled on every run; the cache lives
inside it, never under the real zo state tree. Every number the run uses is a
constant below.

A busy machine moves these numbers by twofold between two runs minutes
apart, so a before and an after are best measured interleaved: `--binary`
runs a bench built elsewhere (the before commit's, say), and runs sharing a
label are pooled into one column — every number is then a median over all
of them, and the table carries the load they ran under.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
ZO_IDE = REPO / "zo-ide"
BENCH_PACKAGE = "codegraph"
BENCH_TARGET = "index_cost"

# The five most-referenced names in this repository's index on 2026-09-22
# (Some 19.5k, String 15.0k, expect 14.4k, new 13.5k, path 12.0k) — the
# heaviest answers `find_references` gives here, so its percentiles are the
# worst case a tool call meets rather than a typical one.
REFERENCE_NAMES = ("Some", "String", "expect", "new", "path")
# The five most-defined names on the same day (tests 539, new 317,
# constructor 234, stream 155, fmt 151).
SYMBOL_NAMES = ("tests", "new", "constructor", "stream", "fmt")
# The file one save rewrites and one outline reads: 1.7k lines of Rust, an
# ordinary large module rather than a toy.
EDIT_FILE = "crates/zerocode-core/src/second_brain_graph.rs"
BUILD_RUNS = 3
LOAD_RUNS = 3
QUERY_REPETITIONS = 20
EDIT_SAMPLES = 5

BYTES_PER_MIB = 1024 * 1024
KIB_PER_MIB = 1024
MILLIS_PER_SECOND = 1000

# `/usr/bin/time`'s peak resident line: BSD `-l` prints bytes, GNU `-v` KiB.
TIME_FLAGS = {"darwin": ["-l"], "linux": ["-v"]}
PEAK_PATTERNS = (
    (re.compile(r"^\s*(\d+)\s+maximum resident set size\s*$", re.M), BYTES_PER_MIB),
    (re.compile(r"^\s*Maximum resident set size \(kbytes\):\s*(\d+)\s*$", re.M), KIB_PER_MIB),
)


def peak_resident_mb(time_stderr: str) -> float | None:
    """The peak resident size `/usr/bin/time` reported, in MiB."""
    for pattern, per_mib in PEAK_PATTERNS:
        match = pattern.search(time_stderr)
        if match:
            return int(match.group(1)) / per_mib
    return None


def snapshot(repo: Path, revision: str, destination: Path) -> str:
    """Extract `revision` of `repo` into an empty `destination`; answer its sha."""
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    archive = subprocess.run(
        ["git", "-C", str(repo), "archive", "--format=tar", revision],
        check=True,
        capture_output=True,
    ).stdout
    subprocess.run(["tar", "-x", "-C", str(destination)], input=archive, check=True)
    return git_sha(repo, revision)


def git_sha(repo: Path, revision: str = "HEAD") -> str:
    return subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "--short", revision],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def bench_binary() -> str:
    """Build the bench once (release profile, as zo ships) and name it."""
    completed = subprocess.run(
        ["cargo", "bench", "-p", BENCH_PACKAGE, "--bench", BENCH_TARGET, "--no-run",
         "--message-format=json"],
        cwd=ZO_IDE,
        check=True,
        capture_output=True,
        text=True,
    )
    for line in completed.stdout.splitlines():
        message = json.loads(line)
        if (
            message.get("reason") == "compiler-artifact"
            and message.get("target", {}).get("name") == BENCH_TARGET
            and message.get("executable")
        ):
            return message["executable"]
    raise SystemExit(f"cargo built no `{BENCH_TARGET}` executable")


def run_phase(binary: str, phase: str, arguments: list[str]) -> dict:
    """One phase in its own process; its report plus the peak resident size."""
    flags = TIME_FLAGS.get(sys.platform)
    command = [binary, phase, *arguments]
    if flags and Path("/usr/bin/time").exists():
        command = ["/usr/bin/time", *flags, *command]
    completed = subprocess.run(command, capture_output=True, text=True)
    if completed.returncode != 0:
        raise SystemExit(f"phase {phase} exited {completed.returncode}:\n{completed.stderr}")
    report = json.loads(completed.stdout.strip().splitlines()[-1])
    report["peak_resident_mb"] = peak_resident_mb(completed.stderr)
    return report


def measure(binary: str, workspace: Path, cache: Path) -> dict:
    common = ["--workspace", str(workspace), "--cache-dir", str(cache)]
    names = ["--references", ",".join(REFERENCE_NAMES)]
    builds = []
    for _ in range(BUILD_RUNS):
        if cache.exists():
            shutil.rmtree(cache)
        cache.mkdir(parents=True)
        builds.append(run_phase(binary, "build", common))
    loads = [run_phase(binary, "load", common + names) for _ in range(LOAD_RUNS)]
    query = run_phase(
        binary,
        "query",
        common
        + names
        + ["--symbols", ",".join(SYMBOL_NAMES), "--outline", EDIT_FILE,
           "--repetitions", str(QUERY_REPETITIONS)],
    )
    edit = run_phase(
        binary, "edit", common + ["--file", EDIT_FILE, "--samples", str(EDIT_SAMPLES)]
    )
    return {"build": builds, "load": loads, "query": [query], "edit": [edit]}


def median(reports: list[dict], key: str) -> float | None:
    values = [report[key] for report in reports if report.get(key) is not None]
    return statistics.median(values) if values else None


def median_of(reports: list[dict], key: str, statistic: str) -> float | None:
    """The median, over reports, of one percentile a report carries."""
    return median([report[key] for report in reports if report.get(key)], statistic)


def pooled(runs: list[dict]) -> list[dict]:
    """Runs sharing a label as one run, in first-seen order: every phase's
    reports concatenated, and every load average kept."""
    by_label: dict[str, dict] = {}
    for run in runs:
        phases = {
            name: reports if isinstance(reports, list) else [reports]
            for name, reports in run["phases"].items()
        }
        pool = by_label.setdefault(
            run["label"], {"label": run["label"], "load_averages": [], "phases": {}}
        )
        pool["load_averages"].append(run["load_average"][0])
        for name, reports in phases.items():
            pool["phases"].setdefault(name, []).extend(reports)
    return list(by_label.values())


def summary(run: dict) -> dict:
    """The table's numbers, each a median over every report of its phase."""
    phases = run["phases"]
    builds, loads, queries, edits = (
        phases["build"], phases["load"], phases["query"], phases["edit"]
    )
    build_ms = median(builds, "build_ms")
    definitions = [query["definition_queries"] for query in queries if "definition_queries" in query]
    return {
        "runs": len(queries),
        "load_average": statistics.median(run["load_averages"]),
        "indexed_files": builds[-1]["indexed_files"],
        "skipped_files": builds[-1]["skipped_files"],
        "build_s": None if build_ms is None else build_ms / MILLIS_PER_SECOND,
        "build_peak_mb": median(builds, "peak_resident_mb"),
        "cache_mb": median(builds, "cache_mb"),
        "load_ms": median(loads, "load_ms"),
        "load_resident_mb": median(loads, "resident_mb"),
        "load_peak_mb": median(loads, "peak_resident_mb"),
        "first_query_ms": median(loads, "first_query_ms"),
        "open_existing_ms": median(loads, "open_existing_ms"),
        "refresh_after_save_ms": median_of(edits, "refresh_after_save_ms", "p50"),
        "refresh_after_create_ms": median_of(edits, "refresh_after_create_ms", "p50"),
        "refresh_after_delete_ms": median_of(edits, "refresh_after_delete_ms", "p50"),
        "find_references_p50_ms": median_of(queries, "find_references_ms", "p50"),
        "find_references_p95_ms": median_of(queries, "find_references_ms", "p95"),
        "find_symbol_p50_ms": median_of(queries, "find_symbol_ms", "p50"),
        "find_symbol_p95_ms": median_of(queries, "find_symbol_ms", "p95"),
        "file_outline_p50_ms": median_of(queries, "file_outline_ms", "p50"),
        "file_links_p50_ms": median_of(queries, "file_links_ms", "p50"),
        "references_to_p50_ms": median_of(definitions, "references_to_ms", "p50"),
        "impact_p50_ms": median_of(definitions, "impact_ms", "p50"),
        "unchanged_refresh_p50_ms": median_of(queries, "unchanged_refresh_ms", "p50"),
        "query_resident_mb": median(queries, "resident_mb"),
    }


# One row per number, in the order the report reads them.
ROWS = (
    ("runs", "실행 수 (중앙값의 표본)", "{:.0f}"),
    ("load_average", "기계 부하 (1분 평균, 중앙값)", "{:.1f}"),
    ("indexed_files", "인덱스한 파일 (건너뜀 제외)", "{:.0f}"),
    ("skipped_files", "건너뛴 파일", "{:.0f}"),
    ("build_s", "첫 인덱스 (s)", "{:.2f}"),
    ("build_peak_mb", "첫 인덱스 최대 RSS (MB)", "{:.0f}"),
    ("cache_mb", "캐시 크기 (MB)", "{:.1f}"),
    ("load_ms", "캐시 로드 (ms)", "{:.1f}"),
    ("load_resident_mb", "로드 뒤 상주 RSS (MB)", "{:.0f}"),
    ("load_peak_mb", "로드 프로세스 최대 RSS (MB)", "{:.0f}"),
    ("first_query_ms", "로드 뒤 첫 find_references (ms)", "{:.1f}"),
    ("open_existing_ms", "읽기가 여는 인덱스 open_existing (ms)", "{:.1f}"),
    ("refresh_after_save_ms", "파일 하나 저장 뒤 갱신 p50 (ms)", "{:.1f}"),
    ("refresh_after_create_ms", "파일 하나 추가 뒤 갱신 p50 (ms)", "{:.1f}"),
    ("refresh_after_delete_ms", "파일 하나 삭제 뒤 갱신 p50 (ms)", "{:.1f}"),
    ("find_references_p50_ms", "find_references p50 (ms)", "{:.2f}"),
    ("find_references_p95_ms", "find_references p95 (ms)", "{:.2f}"),
    ("find_symbol_p50_ms", "find_symbol p50 (ms)", "{:.2f}"),
    ("find_symbol_p95_ms", "find_symbol p95 (ms)", "{:.2f}"),
    ("file_outline_p50_ms", "file_outline p50 (ms)", "{:.2f}"),
    ("file_links_p50_ms", "file_links p50 (ms, 이웃 한 줄의 질문)", "{:.2f}"),
    ("references_to_p50_ms", "정의로 좁힌 find_references p50 (ms)", "{:.2f}"),
    ("impact_p50_ms", "impact p50 (ms)", "{:.2f}"),
    ("unchanged_refresh_p50_ms", "변화 없는 신선도 확인 p50 (ms)", "{:.2f}"),
    ("query_resident_mb", "질의 뒤 상주 RSS (MB)", "{:.0f}"),
)


def cell(value: float | None, template: str) -> str:
    return "—" if value is None else template.format(value)


def table(runs: list[dict]) -> str:
    """A Markdown table, one column per label, in the order first given."""
    runs = pooled(runs)
    summaries = [summary(run) for run in runs]
    lines = [
        "| 지표 | " + " | ".join(run["label"] for run in runs) + " |",
        "|---|" + "---:|" * len(runs),
    ]
    for key, label, template in ROWS:
        lines.append(
            f"| {label} | " + " | ".join(cell(s[key], template) for s in summaries) + " |"
        )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--scratch", type=Path,
                        help="emptied and refilled: the snapshot and the cache live here")
    parser.add_argument("--repo", type=Path, default=REPO, help="checkout whose tree is measured")
    parser.add_argument("--rev", default="HEAD",
                        help="the commit measured — pin it so a before and an after read one tree")
    parser.add_argument("--label", help="the table column this run fills")
    parser.add_argument("--out", type=Path, help="write this run's reports as JSON")
    parser.add_argument("--compare", type=Path, action="append", default=[],
                        help="an earlier --out file, shown before this run (pooled by label)")
    parser.add_argument("--binary",
                        help="a prebuilt index_cost bench to run instead of building this checkout's")
    parser.add_argument("--no-run", action="store_true",
                        help="measure nothing; print the table of the --compare files")
    arguments = parser.parse_args(argv)
    earlier = [json.loads(path.read_text()) for path in arguments.compare]
    if arguments.no_run:
        print(table(earlier))
        return 0
    if arguments.scratch is None or arguments.label is None:
        parser.error("--scratch and --label are required unless --no-run")

    scratch = arguments.scratch.resolve()
    workspace, cache = scratch / "workspace", scratch / "cache"
    measured_sha = snapshot(arguments.repo, arguments.rev, workspace)
    binary = arguments.binary or bench_binary()
    started = time.time()
    run = {
        "label": arguments.label,
        "measured_sha": measured_sha,
        "harness_sha": git_sha(REPO),
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(started)),
        "load_average": list(os.getloadavg()),
        "cpus": os.cpu_count(),
        "phases": measure(binary, workspace, cache),
    }
    if arguments.out:
        arguments.out.write_text(json.dumps(run, indent=2, ensure_ascii=False) + "\n")
    print(table([*earlier, run]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
