#!/usr/bin/env python3
"""Contract for `tools/codegraph-bench/run.py` — the table it prints is read
straight into a commit message, so its two readings must hold: the peak
resident size `/usr/bin/time` reports (BSD bytes, GNU KiB), and one column per
run with every row the report names.

Run: python3 tools/tests/test_codegraph_bench.py   (stdlib only)
"""

import importlib.util
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "codegraph_bench_run", REPO / "tools" / "codegraph-bench" / "run.py"
)
bench = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = bench
_spec.loader.exec_module(bench)


def percentiles(p50: float, p95: float) -> dict:
    return {"p50": p50, "p95": p95, "min": p50, "max": p95, "samples": 20}


def fake_run(label: str, scale: float) -> dict:
    """A run shaped like the bench's JSON lines, every number times `scale`."""
    build = {"build_ms": 1400 * scale, "indexed_files": 1347, "skipped_files": 2,
             "cache_mb": 350 * scale, "peak_resident_mb": 1100 * scale}
    load = {"load_ms": 960 * scale, "resident_mb": 790 * scale,
            "peak_resident_mb": 800 * scale, "first_query_ms": 30 * scale}
    query = {"find_references_ms": percentiles(18 * scale, 20 * scale),
             "find_symbol_ms": percentiles(12 * scale, 13 * scale),
             "file_outline_ms": percentiles(12 * scale, 13 * scale),
             "unchanged_refresh_ms": percentiles(12 * scale, 12 * scale),
             "resident_mb": 800 * scale}
    edit = {"refresh_after_save_ms": percentiles(500 * scale, 520 * scale),
            "refresh_after_create_ms": percentiles(1500 * scale, 1600 * scale),
            "refresh_after_delete_ms": percentiles(1500 * scale, 1600 * scale)}
    return {"label": label,
            "phases": {"build": [build, build], "load": [load] * 3, "query": query, "edit": edit}}


class PeakResident(unittest.TestCase):
    def test_bsd_time_reports_bytes(self):
        stderr = "        1.02 real         0.90 user\n  838860800  maximum resident set size\n"
        self.assertEqual(bench.peak_resident_mb(stderr), 800.0)

    def test_gnu_time_reports_kibibytes(self):
        stderr = "\tMaximum resident set size (kbytes): 819200\n"
        self.assertEqual(bench.peak_resident_mb(stderr), 800.0)

    def test_no_line_is_no_number(self):
        self.assertIsNone(bench.peak_resident_mb("real 1.0\n"))


class Table(unittest.TestCase):
    def test_one_column_per_run_and_every_row(self):
        table = bench.table([fake_run("before", 1.0), fake_run("after", 0.5)])
        lines = table.splitlines()
        self.assertEqual(lines[0], "| 지표 | before | after |")
        self.assertEqual(len(lines), 2 + len(bench.ROWS))
        self.assertIn("| 캐시 로드 (ms) | 960.0 | 480.0 |", lines)
        self.assertIn("| 첫 인덱스 (s) | 1.40 | 0.70 |", lines)

    def test_a_missing_number_is_a_dash_not_a_zero(self):
        run = fake_run("before", 1.0)
        for load in run["phases"]["load"]:
            load["peak_resident_mb"] = None
        self.assertIn("| 로드 프로세스 최대 RSS (MB) | — |", bench.table([run]).splitlines())


if __name__ == "__main__":
    unittest.main()
