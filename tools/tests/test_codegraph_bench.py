#!/usr/bin/env python3
"""Contract for `tools/codegraph-bench/run.py` and `truth.py` — the table the
first prints is read straight into a commit message, so its readings must
hold (the peak resident size `/usr/bin/time` reports, BSD bytes or GNU KiB;
one column per label with every row the report names); and the second's
verdicts are the precision numbers a commit cites, so what counts as a real
dependency and a true reference is pinned on a hand-made LSIF.

Run: python3 tools/tests/test_codegraph_bench.py   (stdlib only)
"""

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]


def load(name: str, file: str):
    spec = importlib.util.spec_from_file_location(name, REPO / "tools" / "codegraph-bench" / file)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


bench = load("codegraph_bench_run", "run.py")
truth = load("codegraph_bench_truth", "truth.py")


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
    return {"label": label, "load_average": [4.0 * scale, 3.0, 2.0],
            "phases": {"build": [build, build], "load": [load] * 3, "query": [query], "edit": [edit]}}


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

    def test_runs_sharing_a_label_pool_into_one_median_column(self):
        runs = [fake_run("before", 1.0), fake_run("after", 0.5), fake_run("before", 3.0)]
        lines = bench.table(runs).splitlines()
        self.assertEqual(lines[0], "| 지표 | before | after |")
        # The pooled column holds the median of 960 and 2880 ms over six loads.
        self.assertIn("| 캐시 로드 (ms) | 1920.0 | 480.0 |", lines)
        self.assertIn("| 실행 수 (중앙값의 표본) | 2 | 1 |", lines)
        self.assertIn("| 기계 부하 (1분 평균, 중앙값) | 8.0 | 2.0 |", lines)

    def test_an_older_single_report_run_still_reads(self):
        run = fake_run("before", 1.0)
        run["phases"]["query"] = run["phases"]["query"][0]
        run["phases"]["edit"] = run["phases"]["edit"][0]
        self.assertIn("| find_references p50 (ms) | 18.00 |", bench.table([run]).splitlines())

    def test_a_missing_number_is_a_dash_not_a_zero(self):
        run = fake_run("before", 1.0)
        for load in run["phases"]["load"]:
            load["peak_resident_mb"] = None
        self.assertIn("| 로드 프로세스 최대 RSS (MB) | — |", bench.table([run]).splitlines())


def tiny_lsif(root: Path) -> list[str]:
    """Two documents: `use.rs` spells a name defined in `lib.rs` (ids 10/11 are
    the definition and the reference, both resolving to the definition) and
    a local defined and used in `use.rs` itself."""
    uri = lambda file: (root / file).as_uri()
    elements = [
        {"id": 1, "type": "vertex", "label": "document", "uri": uri("src/lib.rs")},
        {"id": 2, "type": "vertex", "label": "document", "uri": uri("src/use.rs")},
        {"id": 10, "type": "vertex", "label": "range", "start": {"line": 0, "character": 7}},
        {"id": 11, "type": "vertex", "label": "range", "start": {"line": 1, "character": 4}},
        {"id": 12, "type": "vertex", "label": "range", "start": {"line": 2, "character": 8}},
        {"id": 20, "type": "vertex", "label": "resultSet"},
        {"id": 21, "type": "vertex", "label": "resultSet"},
        {"id": 30, "type": "vertex", "label": "definitionResult"},
        {"id": 31, "type": "vertex", "label": "definitionResult"},
        {"id": 40, "type": "edge", "label": "contains", "outV": 1, "inVs": [10]},
        {"id": 41, "type": "edge", "label": "contains", "outV": 2, "inVs": [11, 12]},
        {"id": 42, "type": "edge", "label": "next", "outV": 10, "inV": 20},
        {"id": 43, "type": "edge", "label": "next", "outV": 11, "inV": 20},
        {"id": 44, "type": "edge", "label": "next", "outV": 12, "inV": 21},
        {"id": 45, "type": "edge", "label": "textDocument/definition", "outV": 20, "inV": 30},
        {"id": 46, "type": "edge", "label": "textDocument/definition", "outV": 21, "inV": 31},
        {"id": 47, "type": "edge", "label": "item", "outV": 30, "inVs": [10], "document": 1},
        {"id": 48, "type": "edge", "label": "item", "outV": 31, "inVs": [12], "document": 2},
    ]
    return [json.dumps(element) for element in elements]


class Truth(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / "src").mkdir()
        (self.root / "src" / "lib.rs").write_text("pub fn helper() {}\n")
        (self.root / "src" / "use.rs").write_text("fn run() {\n    helper();\n    let helper = 1;\n}\n")
        self.lsif = truth.Lsif.read(tiny_lsif(self.root), self.root)

    def tearDown(self):
        self.directory.cleanup()

    def test_a_file_depends_on_where_its_names_resolve(self):
        self.assertEqual(self.lsif.dependencies(), {"src/use.rs": {"src/lib.rs"}})

    def test_a_link_counts_when_the_dependency_is_real(self):
        links = {"files": [{
            "file": "src/use.rs",
            "uses": [{"file": "src/lib.rs", "references": 1, "test": False},
                     {"file": "src/nowhere.rs", "references": 1, "test": False}],
            "used_by": [],
        }]}
        report = truth.judge_links(links, self.lsif)
        # nowhere.rs is not an LSIF document: not judged, not counted.
        self.assertEqual(report["uses_all"], {"links": 1, "confirmed": 1, "precision": 1.0})
        self.assertEqual(report["uses_recall"], 1.0)

    def test_an_occurrence_is_true_only_where_it_resolves_to_the_definition(self):
        definition = {"name": "helper", "kind": "fn", "file": "src/lib.rs", "container": None,
                      "range": {"start": {"row": 0, "column": 7}, "end": {"row": 0, "column": 13},
                                "start_byte": 7, "end_byte": 13}}
        sample = {"samples": [{
            "definition": definition,
            "definers": 1,
            "occurrences": [
                {"file": "src/use.rs", "row": 1, "column": 4, "imports_spell": False},
                {"file": "src/use.rs", "row": 2, "column": 8, "imports_spell": False},
            ],
        }]}
        report = truth.judge_references(sample, self.lsif, self.root)
        exact = report["filters"]["exact_name"]
        self.assertEqual((exact["kept"], exact["kept_true"]), (2, 1))
        self.assertEqual(exact["pooled_recall"], 1.0)
        # Neither occurrence's file imports the name: only the one-definer
        # rule keeps them.
        self.assertEqual(report["filters"]["imports_spell"]["kept"], 0)
        self.assertEqual(report["filters"]["unique_or_imports_spell"]["kept"], 2)


if __name__ == "__main__":
    unittest.main()
