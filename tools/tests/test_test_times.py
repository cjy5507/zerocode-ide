#!/usr/bin/env python3
"""Contract for `tools/test-timing/test_times.py` — the per-test clock the
shell crate's test-time work (t-80605) is measured with.

A wrong reader here makes a wrong before/after: a test whose seconds are read
from the wrong field counts as 0 s, a test that never finished drops out of the
count, and `RUSTC_BOOTSTRAP` handed to cargo instead of the test executable
rebuilds the whole crate under a fingerprint no other check uses. The fake
executable below stands in for a libtest binary, so nothing here builds Rust.

Run: python3 tools/tests/test_test_times.py   (stdlib only)
"""

import importlib.util
import json
import os
import stat
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("test_times", REPO / "tools" / "test-timing" / "test_times.py")
times = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = times
_spec.loader.exec_module(times)

# A libtest stand-in: `--list --format terse` names three tests; json mode
# prints libtest's events and refuses without RUSTC_BOOTSTRAP, as stable
# libtest does; a serial run prints `test NAME ... ok` lines.
FAKE = textwrap.dedent("""\
    #!/usr/bin/env python3
    import json, os, sys
    NAMES = ["a::one", "a::two", "b::three"]
    argv = sys.argv[1:]
    if "--list" in argv:
        assert "RUSTC_BOOTSTRAP" not in os.environ
        print("\\n".join(name + ": test" for name in NAMES))
        sys.exit(0)
    if "--format" in argv and "json" in argv:
        if os.environ.get("RUSTC_BOOTSTRAP") != "1":
            print("error: the option `Z` is only accepted on the nightly compiler", file=sys.stderr)
            sys.exit(101)
        assert os.path.realpath(os.environ["CARGO_MANIFEST_DIR"]) == os.path.realpath(os.getcwd())
        print(json.dumps({"type": "suite", "event": "started", "test_count": 3}))
        for name, took, outcome in (("a::one", 0.5, "ok"), ("b::three", 2.25, "failed"), ("a::two", 0.01, "ok")):
            print(json.dumps({"type": "test", "event": "started", "name": name}))
            print("a stray print")
            print(json.dumps({"type": "test", "name": name, "event": outcome, "exec_time": took}))
        print(json.dumps({"type": "test", "event": "ignored", "name": "b::ignored"}))
        print(json.dumps({"type": "suite", "event": "failed", "passed": 2, "failed": 1, "ignored": 1,
                          "filtered_out": 0, "exec_time": 2.8}))
        sys.exit(101)
    assert "RUSTC_BOOTSTRAP" not in os.environ
    for name in [a for a in argv if "::" in a]:
        print("test " + name + " ... ok")
    """)


def ticking(*stamps):
    values = iter(stamps)
    return lambda: next(values)


class ReadsLibtestEvents(unittest.TestCase):
    def test_seconds_come_from_exec_time_and_stamps_from_arrival(self):
        lines = [
            '{"type":"suite","event":"started","test_count":2}',
            '{"type":"test","event":"started","name":"m::slow"}',
            '{"type":"test","event":"started","name":"m::fast"}',
            '{"type":"test","name":"m::fast","event":"ok","exec_time":0.002}',
            '{"type":"test","name":"m::slow","event":"failed","exec_time":3.5,"stdout":"boom"}',
            '{"type":"suite","event":"failed","passed":1,"failed":1,"ignored":0,"filtered_out":0,"exec_time":3.6}',
        ]
        folded = times.read_json_events(lines, ticking(10.0, 10.0, 10.1, 10.2, 13.6, 13.7))
        rows = {row["name"]: row for row in folded["tests"]}
        self.assertEqual(rows["m::slow"], {"name": "m::slow", "started": 0.0, "outcome": "failed",
                                           "finished": 3.6, "seconds": 3.5})
        self.assertEqual(rows["m::fast"]["seconds"], 0.002)
        self.assertEqual(folded["suite"]["failed"], 1)

    def test_a_line_that_is_not_an_event_is_skipped(self):
        folded = times.read_json_events(["not json", "[1]", '{"type":"test","event":"ignored","name":"x"}'],
                                        ticking(0.0, 0.1, 0.2))
        self.assertEqual(folded["tests"], [{"name": "x", "outcome": "ignored", "finished": 0.2}])

    def test_a_serial_line_is_timed_from_the_line_before_it(self):
        rows = times.read_serial_lines(["running 2 tests\n", "test a::x ... ok\n", "test a::y ... FAILED\n"],
                                       ticking(1.0, 1.5, 4.0), started=1.0)
        self.assertEqual(rows, [{"name": "a::x", "outcome": "ok", "seconds": 0.5},
                                {"name": "a::y", "outcome": "FAILED", "seconds": 2.5}])


class FindsTheExecutable(unittest.TestCase):
    def test_only_the_test_profile_artifact_of_the_named_target(self):
        lines = [
            "Compiling chatter from the build line",
            json.dumps({"reason": "compiler-artifact", "profile": {"test": False}, "executable": "/t/debug/shell",
                        "target": {"name": "shell"}, "manifest_path": "/r/crates/shell/Cargo.toml"}),
            json.dumps({"reason": "compiler-artifact", "profile": {"test": True}, "executable": "/t/debug/deps/pick-1",
                        "target": {"name": "pick"}, "manifest_path": "/r/crates/shell/Cargo.toml"}),
            json.dumps({"reason": "compiler-artifact", "profile": {"test": True}, "executable": "/t/debug/deps/shell-2",
                        "target": {"name": "shell"}, "manifest_path": "/r/crates/shell/Cargo.toml"}),
        ]
        executable, manifest = times.find_executable(lines, "shell")
        self.assertEqual(executable, Path("/t/debug/deps/shell-2"))
        self.assertEqual(manifest, Path("/r/crates/shell/Cargo.toml"))


def result(commit, seconds, run, tests_extra=()):
    tests = [{"name": name, "outcome": "ok", "seconds": value, "started": 0.0, "finished": value}
             for name, value in seconds.items()]
    return {"package": "p", "target": "--bin p", "commit": commit, "mode": "json", "threads": 8,
            "load_before": [1.0], "load_after": [1.2], "condition": {"CARGO_INCREMENTAL": "0"},
            "build_seconds": 100.0, "run_seconds": run, "tests": tests + list(tests_extra), "exit": 0}


class Tables(unittest.TestCase):
    def test_the_top_rows_and_their_share_of_the_sum(self):
        text = times.table(result("abc", {"m::a::slow": 6.0, "m::a::mid": 3.0, "n::fast": 1.0}, run=6.5), 2, 2)
        self.assertIn("| 1 | 6.00 | 60.0 | 60.0 |", text)
        self.assertIn("| 2 | 3.00 | 30.0 | 90.0 |", text)
        self.assertNotIn("n::fast` |", text.split("top 2")[0])
        self.assertIn("top 2: 9.0 s, 90.0 % of the sum", text)
        self.assertIn("| `m::a` | 2 | 9.0 | 90.0 |", text)
        self.assertIn("still running in the last 10 % of the run", text)

    def test_compare_names_the_seconds_each_test_saved(self):
        before = result("before", {"x": 10.0, "y": 1.0}, run=10.0)
        after = result("after", {"x": 2.0, "y": 1.0}, run=4.0)
        text = times.compare(before, after, 5)
        self.assertIn("| run s | 10.0 → 4.0 (-60.0 %) |", text)
        self.assertIn("| tests | 2 → 2 |", text)
        self.assertIn("| 8.00 | 10.00 | 2.00 | `x` |", text)
        self.assertNotIn("environments differ", text)

    def test_an_unfinished_test_still_counts(self):
        facts = times.summary(result("c", {"x": 1.0}, run=1.0, tests_extra=[{"name": "hung", "started": 0.5}]))
        self.assertEqual(facts["tests"], 2)
        self.assertEqual(facts["outcomes"], {"ok": 1, "unfinished": 1})


class RunsTheExecutable(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.cwd = Path(self.dir.name)
        self.exe = self.cwd / "fake-tests"
        self.exe.write_text(FAKE)
        self.exe.chmod(self.exe.stat().st_mode | stat.S_IXUSR)
        self.saved = os.environ.pop("RUSTC_BOOTSTRAP", None)

    def tearDown(self):
        if self.saved is not None:
            os.environ["RUSTC_BOOTSTRAP"] = self.saved
        self.dir.cleanup()

    def test_json_mode_gives_the_switch_to_the_executable_alone(self):
        ran = times.run_json(self.exe, self.cwd, None, [])
        self.assertNotIn("RUSTC_BOOTSTRAP", os.environ)
        self.assertEqual(ran["exit"], 101)
        seconds = {row["name"]: row.get("seconds") for row in ran["tests"]}
        self.assertEqual(seconds, {"a::one": 0.5, "b::three": 2.25, "a::two": 0.01, "b::ignored": None})

    def test_groups_mode_runs_every_listed_test_once(self):
        ran = times.run_groups(self.exe, self.cwd, 1, [])
        self.assertEqual(sorted(row["name"] for row in ran["tests"]), ["a::one", "a::two", "b::three"])
        self.assertEqual(ran["suite"], {"groups": 2, "groups_failed": 0})


class Arguments(unittest.TestCase):
    def test_filters_after_the_double_dash_go_to_libtest(self):
        self.assertEqual(times.split_filters(["measure", "--jobs", "4", "--", "orchestration::"]),
                         (["measure", "--jobs", "4"], ["orchestration::"]))


if __name__ == "__main__":
    unittest.main()
