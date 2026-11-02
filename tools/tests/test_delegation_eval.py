#!/usr/bin/env python3
"""The direct-against-delegation harness (`tools/delegation-eval/eval.py`, t-73910 W4-3).

It spends nothing: a plan starts no agent. Every planned run stays in the denominator,
and a price or a verification it cannot read is unknown and never 0.

The second half judges the scenarios themselves, because a bad one wastes a paid measurement.
Every seed really fails until it is fixed, and every piece of its fix is needed. A coupled
scenario needs every file; an independent one splits into parts that pass alone. The answers
live only in `tools/tests/delegation_eval_fixes/`, and nothing reads a clock or a network.

Run: python3 tools/tests/test_delegation_eval.py   (stdlib only)
"""

import ast
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tokenize
import unittest
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EVAL = REPO / "tools/delegation-eval/eval.py"
TOOL = REPO / "tools/delegation-eval"
FIXES = REPO / "tools/tests/delegation_eval_fixes"
SCENARIOS = json.loads((TOOL / "scenarios.json").read_text())["scenarios"]
SAMPLE = "s01-small-fix"
ASKED_FOR = {"small-fix": 3, "coupled": 2, "independent": 2, "verification": 1, "recovery": 2}
QUIET = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
HASH_SEEDS = ("0", "1", "2")
AT_ONCE = 4
LEAK_WORDS = re.compile(r"\b(fixme|todo|xxx|hack|bug|buggy|fixed version|solution|answer|workaround)\b", re.I)
COMMENTED_CODE = re.compile(r"^\s*#\s*(return|if|for|while|def|class|import|from|raise|\w+\s*=[^=])\b")
NOT_DETERMINISTIC = {"asyncio", "datetime", "http", "multiprocessing", "random", "secrets", "select", "signal",
                     "socket", "ssl", "subprocess", "threading", "time", "urllib", "uuid"}
SHORTEST_LEAK = 24  # a line this long or more, added by a fix, should not already stand in the seed


def harness():
    spec = importlib.util.spec_from_file_location("delegation_eval_harness", EVAL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


HARNESS = harness()


def fix_of(scenario):
    return FIXES / f"{scenario['id']}.patch"


def by_id(scenario_id):
    return next(s for s in SCENARIOS if s["id"] == scenario_id)


def sections_of(patch):
    """A patch as [(header, [hunk, ...]), ...], one item per file, each piece its own text."""
    files = re.split(r"(?m)^(?=diff --git )", patch)
    sections = []
    for one in filter(None, files):
        pieces = re.split(r"(?m)^(?=@@ )", one)
        sections.append((pieces[0], pieces[1:]))
    return sections


def path_of(header):
    return re.search(r"(?m)^\+\+\+ b/(.+)$", header)[1]


def apply_sections(checkout, sections):
    """Put these pieces of a fix, written with no context lines, into the seed in CHECKOUT.

    `git apply` finds a context-free hunk by its new line number, which moves once a piece before
    it is left out. Here each piece stands on its old line numbers, and a seed that has drifted
    from its fix fails on the check of what the piece removes.
    """
    for header, hunks in sections:
        if not hunks:
            continue
        path = Path(checkout, path_of(header))
        lines = path.read_text().splitlines(keepends=True) if path.exists() else []
        for hunk in reversed(hunks):
            first, count = re.match(r"@@ -(\d+)(?:,(\d+))? ", hunk).group(1, 2)
            first, count = int(first), 1 if count is None else int(count)
            body = hunk.splitlines(keepends=True)[1:]
            at = first if count == 0 else first - 1
            assert lines[at:at + count] == [line[1:] for line in body if line.startswith("-")], f"{path.name}:{first}"
            lines[at:at + count] = [line[1:] for line in body if line.startswith("+")]
        path.write_text("".join(lines))


def apply_fix(checkout, scenario):
    apply_sections(checkout, sections_of(fix_of(scenario).read_text()))


def prose_of(path, text):
    """What a reader sees that a program does not run: all of a document, a program's comments and strings."""
    if path.suffix != ".py":
        return text
    tokens = tokenize.generate_tokens(io.StringIO(text).readline)
    return "\n".join(token.string for token in tokens if token.type in (tokenize.COMMENT, tokenize.STRING))


def imports_of(source):
    imported = set()
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.Import):
            imported |= {alias.name.split(".")[0] for alias in node.names}
        elif isinstance(node, ast.ImportFrom):
            imported.add((node.module or "").split(".")[0])
    return imported


class Checkout:
    """One scenario's seed as a git checkout, to be fixed in part and put back."""

    def __init__(self, scenario, root):
        self.scenario = scenario
        self.path = Path(root) / scenario["id"]
        shutil.copytree(TOOL / scenario["seed"], self.path)
        for args in (("init", "-q", "-b", "main"), ("add", "-A"), ("commit", "-q", "-m", "seed")):
            HARNESS.git(self.path, *args)

    def reset(self):
        HARNESS.git(self.path, "checkout", "-q", ".")
        HARNESS.git(self.path, "clean", "-fdq")

    def sections(self):
        return sections_of(fix_of(self.scenario).read_text())

    def run(self, argv=None, env=QUIET):
        argv = argv or HARNESS.accept_argv(self.scenario, TOOL)
        return subprocess.run(argv, cwd=self.path, capture_output=True, text=True, env=env, timeout=120)

    def fix(self):
        apply_fix(self.path, self.scenario)


class EvalTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.out = self.root / "out"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.orc_calls = self.root / "orc-calls"
        orc = self.bin / "zerocode-orc"
        orc.write_text(f"#!/bin/sh\necho \"$@\" >> '{self.orc_calls}'\nexit 0\n")
        orc.chmod(0o755)
        self.env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}"}

    def tearDown(self):
        self.tmp.cleanup()

    def eval(self, *args):
        said = subprocess.run([sys.executable, str(EVAL), *args], capture_output=True, text=True, env=self.env, timeout=60)
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        return said.stdout

    def plan(self, reps=1, seed=1, out=None, only=SAMPLE):
        return self.eval("plan", "--reps", str(reps), "--seed", str(seed), "--agent", "claude",
                         "--model", "model-x", "--effort", "high", "--out", str(out or self.out),
                         "--only", only)

    def runs(self, out=None):
        return json.loads(((out or self.out) / "plan.json").read_text())["runs"]

    def summary(self):
        return {(r["scenario"], r["condition"]): r for r in json.loads(self.eval("summarize", "--out", str(self.out), "--json"))["rows"]}

    def test_a_plan_holds_every_run_once_in_its_seeds_order_with_one_model_and_effort(self):
        printed = self.plan(reps=3, seed=7)
        runs = self.runs()
        self.assertEqual(len(runs), 6)
        self.assertEqual(sorted((r["condition"], r["rep"]) for r in runs),
                         sorted((c, n) for c in ("direct", "delegate") for n in (1, 2, 3)))
        self.plan(reps=3, seed=7, out=self.root / "again")
        self.assertEqual([r["run"] for r in self.runs(self.root / "again")], [r["run"] for r in runs])
        self.assertEqual(printed.count("--model model-x --effort high"), 6, "both conditions launch alike")
        self.assertIn("Do not summon", (self.out / runs[[r["condition"] for r in runs].index("direct")]["run"] / "brief.md").read_text())

    def test_nothing_it_does_starts_an_agent(self):
        self.plan()
        self.eval("prepare", "--out", str(self.out))
        self.eval("accept", "--out", str(self.out))
        self.eval("summarize", "--out", str(self.out))
        self.assertFalse(self.orc_calls.exists(), "the harness called zerocode-orc")

    def test_the_seed_fails_its_acceptance_until_it_is_fixed_and_a_changed_test_is_outside(self):
        self.plan()
        self.eval("prepare", "--out", str(self.out))
        first, second = self.runs()
        self.eval("accept", "--out", str(self.out))
        self.assertFalse(json.loads((self.out / first["run"] / "accept.json").read_text())["passed"])
        apply_fix(first["checkout"], by_id(SAMPLE))
        apply_fix(second["checkout"], by_id(SAMPLE))
        Path(second["checkout"], "test_tally.py").write_text("")
        self.eval("accept", "--out", str(self.out))
        self.assertTrue(json.loads((self.out / first["run"] / "accept.json").read_text())["passed"])
        outside = json.loads((self.out / second["run"] / "accept.json").read_text())
        self.assertFalse(outside["passed"])
        self.assertEqual(outside["outside"], ["test_tally.py"])

    def test_a_run_never_accepted_stays_in_the_denominator(self):
        self.plan()
        self.eval("prepare", "--out", str(self.out))
        direct = next(r for r in self.runs() if r["condition"] == "direct")
        apply_fix(direct["checkout"], by_id(SAMPLE))
        self.eval("accept", "--out", str(self.out), "--run", direct["run"])
        rows = self.summary()
        self.assertEqual((rows[("s01-small-fix", "direct")]["passed"], rows[("s01-small-fix", "direct")]["runs"]), (1, 1))
        self.assertEqual((rows[("s01-small-fix", "delegate")]["passed"], rows[("s01-small-fix", "delegate")]["runs"]), (0, 1))

    def dispatch(self, run, task, started, ended, verified, attempts=1):
        body = {"startedMs": started, "endedMs": ended, "time": {"task": {
            "firstStartedMs": started, "attempts": attempts, "afterReport": {"verifiedMs": verified}}}}
        (self.out / run / f"dispatch-{task}.json").write_text(json.dumps(body))

    def cost(self, run, task, usd, reason=None, complete=True):
        body = {"generation": {"historyComplete": complete, "sessionsKnown": 1, "sessionsLinked": 1,
                               "inputTokens": 10, "outputTokens": 5, "cacheReadTokens": 0, "cacheWriteTokens": 0,
                               "usd": usd, "usdReason": reason}}
        (self.out / run / f"cost-{task}.json").write_text(json.dumps(body))

    def test_a_price_it_cannot_read_is_unknown_never_zero(self):
        self.plan()
        delegate = next(r["run"] for r in self.runs() if r["condition"] == "delegate")
        self.dispatch(delegate, "t-1", 1000, 5000, 6000)
        self.dispatch(delegate, "t-2", 2000, 4000, 4500)
        self.cost(delegate, "t-1", 0.25)
        self.cost(delegate, "t-2", None, reason="unpriced_model")
        row = self.summary()[("s01-small-fix", "delegate")]
        self.assertIsNone(row["usdMedian"])
        self.assertEqual(row["tokensMedian"], 30)
        self.assertIn("dollars: unpriced_model", row["why"])
        self.assertIsNone(row["interventions"])
        printed = self.eval("summarize", "--out", str(self.out))
        line = next(l for l in printed.splitlines() if l.startswith("| s01-small-fix | delegate |"))
        self.assertEqual(line.split(" | ")[6], "unknown", "the dollars printed as a number")

    def test_the_wall_clock_runs_to_the_latest_verification_and_counts_overlap(self):
        self.plan()
        delegate = next(r["run"] for r in self.runs() if r["condition"] == "delegate")
        self.dispatch(delegate, "t-1", 1000, 5000, 9000, attempts=2)
        self.dispatch(delegate, "t-2", 2000, 4000, 4500)
        row = self.summary()[("s01-small-fix", "delegate")]
        self.assertEqual(row["wallMedianMs"], 8000)
        self.assertEqual(row["rework"], 1)
        self.assertEqual(row["mostAtOnce"], 2)
        direct = next(r["run"] for r in self.runs() if r["condition"] == "direct")
        self.dispatch(direct, "t-3", 1000, 3000, None)
        self.assertIsNone(self.summary()[("s01-small-fix", "direct")]["wallMedianMs"], "no verification, no wall clock")

    def test_only_chooses_scenarios_and_refuses_one_that_does_not_exist(self):
        self.plan(only="s02-version-ranges")
        self.assertEqual({r["scenario"] for r in self.runs()}, {"s02-version-ranges"})
        said = subprocess.run([sys.executable, str(EVAL), "plan", "--reps", "1", "--seed", "1", "--agent", "a", "--model", "m",
                               "--effort", "e", "--out", str(self.root / "none"), "--only", "s99-nothing"],
                              capture_output=True, text=True, env=self.env)
        self.assertNotEqual(said.returncode, 0)
        self.assertIn("s99-nothing", said.stderr)

    def test_without_only_a_plan_holds_every_scenario(self):
        self.eval("plan", "--reps", "3", "--seed", "1", "--agent", "a", "--model", "m", "--effort", "e", "--out", str(self.out))
        self.assertEqual(len(self.runs()), len(SCENARIOS) * 2 * 3)
        self.assertEqual({r["scenario"] for r in self.runs()}, {s["id"] for s in SCENARIOS})

    def test_an_acceptance_that_never_ends_is_not_a_pass_and_does_not_hold_up_the_rest(self):
        scenarios = self.root / "scenarios.json"
        scenarios.write_text(json.dumps({"scenarios": [{**by_id(SAMPLE), "seed": str(TOOL / by_id(SAMPLE)["seed"]),
                                                        "accept": [sys.executable, "-c", "while True: pass"]}]}))
        self.eval("plan", "--scenarios", str(scenarios), "--reps", "1", "--seed", "1", "--agent", "a", "--model", "m",
                  "--effort", "e", "--out", str(self.out))
        self.eval("prepare", "--out", str(self.out))
        self.eval("accept", "--out", str(self.out), "--timeout", "0.2")
        for run in self.runs():
            said = json.loads((self.out / run["run"] / "accept.json").read_text())
            self.assertFalse(said["passed"])
            self.assertIn("timed out", said["rc"])

    def test_a_file_moved_onto_an_allowed_name_still_counts_as_outside(self):
        self.plan(only="s08-pricing-audit")
        self.eval("prepare", "--out", str(self.out))
        run = self.runs()[0]
        HARNESS.git(run["checkout"], "mv", "test_pricing.py", "findings.json")
        HARNESS.git(run["checkout"], "commit", "-q", "-m", "moved")
        self.eval("accept", "--out", str(self.out), "--run", run["run"])
        said = json.loads((self.out / run["run"] / "accept.json").read_text())
        self.assertEqual(said["outside"], ["test_pricing.py"])


class ScenarioSetTest(unittest.TestCase):
    """What the set as a whole has to be, read from scenarios.json alone."""

    def test_the_set_has_the_shape_asked_for(self):
        self.assertEqual(Counter(s["kind"] for s in SCENARIOS), Counter(ASKED_FOR))
        ids = [s["id"] for s in SCENARIOS]
        self.assertEqual(len(ids), len(set(ids)))

    def test_every_seed_and_every_fix_belongs_to_a_scenario(self):
        ids = {s["id"] for s in SCENARIOS}
        self.assertEqual({p.name for p in (TOOL / "seed").iterdir() if p.is_dir()}, ids)
        self.assertEqual({p.stem for p in FIXES.glob("*.patch")}, ids)

    def test_every_brief_is_one_english_paragraph_that_does_not_tell_the_answer_or_how_to_divide_it(self):
        for scenario in SCENARIOS:
            with self.subTest(scenario["id"]):
                brief = scenario["brief"]
                self.assertTrue(brief.isascii())
                self.assertNotIn("\n", brief)
                self.assertNotRegex(brief, r"(?i)\b(summon|delegat|worker|split (it|the|this)|in parallel|independent of)\w*\b")

    def test_every_scenario_says_why_it_is_its_kind(self):
        for scenario in SCENARIOS:
            self.assertTrue(scenario["why"].strip(), scenario["id"])

    def test_both_conditions_ask_for_the_same_work(self):
        for scenario in SCENARIOS:
            with self.subTest(scenario["id"]):
                direct = HARNESS.brief_of(scenario, "direct", "CHECKOUT")
                delegate = HARNESS.brief_of(scenario, "delegate", "CHECKOUT")
                self.assertEqual(direct.split("\n", 1)[1], delegate.split("\n", 1)[1])
                for name in scenario["files"]:
                    self.assertIn(name, direct)

    def test_a_brief_prints_the_acceptance_command_unless_it_is_kept_out_of_sight(self):
        for scenario in SCENARIOS:
            with self.subTest(scenario["id"]):
                brief = HARNESS.brief_of(scenario, "direct", "CHECKOUT")
                self.assertEqual(" ".join(scenario["accept"]) in brief, not HARNESS.is_hidden(scenario))
                self.assertNotIn("conformance/", brief)


class ScenarioTest(unittest.TestCase):
    """What each scenario has to be, every one judged the same way."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        folder = Path(self.tmp.name) / "bin"
        folder.mkdir()
        self.orc_calls = Path(self.tmp.name) / "orc-calls"
        orc = folder / "zerocode-orc"
        orc.write_text(f"#!/bin/sh\necho \"$@\" >> '{self.orc_calls}'\nexit 0\n")
        orc.chmod(0o755)
        self.env = {**os.environ, "PATH": f"{folder}:{os.environ['PATH']}"}

    def each_scenario(self, body, kind=None):
        """BODY(number, scenario) for every scenario of KIND, a few at once, each reported on its own."""
        def outcome(numbered):
            try:
                body(*numbered)
            except Exception as error:
                return numbered[1]["id"], error
            return numbered[1]["id"], None

        chosen = [(n, s) for n, s in enumerate(SCENARIOS) if kind in (None, s["kind"])]
        with ThreadPoolExecutor(AT_ONCE) as pool:
            for scenario_id, error in pool.map(outcome, chosen):
                with self.subTest(scenario_id):
                    if error is not None:
                        raise error

    def each(self, body, kind=None):
        """BODY on a fresh checkout of every scenario of KIND."""
        self.each_scenario(lambda number, scenario: body(Checkout(scenario, Path(self.tmp.name) / str(number))), kind)

    def sources(self, checkout):
        return [p for p in checkout.path.rglob("*") if p.is_file() and ".git" not in p.parts and "__pycache__" not in p.parts]

    def test_the_seed_fails_on_what_it_does_not_do_and_not_on_being_broken(self):
        def body(checkout):
            said = checkout.run()
            self.assertNotEqual(said.returncode, 0, "the seed already passes")
            self.assertNotIn("Failed to import", said.stderr)
            self.assertNotIn("SyntaxError", said.stderr)
            counts = re.search(r"FAILED \((.+)\)", said.stderr)
            self.assertIsNotNone(counts, said.stderr[-400:])
            self.assertGreaterEqual(sum(int(n) for n in re.findall(r"=(\d+)", counts[1])), 3, "too few things wrong")
        self.each(body)

    def test_the_seed_fails_and_the_fix_passes_under_every_hash_seed_and_it_leaves_nothing_behind(self):
        def body(checkout):
            for hash_seed in HASH_SEEDS:
                self.assertNotEqual(checkout.run(env={**QUIET, "PYTHONHASHSEED": hash_seed}).returncode, 0)
            checkout.fix()
            for hash_seed in HASH_SEEDS:
                said = checkout.run(env={**QUIET, "PYTHONHASHSEED": hash_seed})
                self.assertEqual(said.returncode, 0, said.stderr[-800:])
            status = HARNESS.git(checkout.path, "status", "--porcelain")
            self.assertEqual({line[3:] for line in status.splitlines()}, set(checkout.scenario["files"]),
                             "the fix and the allowed files differ")
        self.each(body)

    def test_every_piece_of_the_fix_is_needed(self):
        def body(checkout):
            sections = checkout.sections()
            for at, (header, hunks) in enumerate(sections):
                for drop in range(len(hunks)):
                    kept = [*sections[:at], (header, hunks[:drop] + hunks[drop + 1:]), *sections[at + 1:]]
                    apply_sections(checkout.path, kept)
                    self.assertNotEqual(checkout.run().returncode, 0, f"{path_of(header)} piece {drop} changes nothing tested")
                    checkout.reset()
        self.each(body)

    def test_a_coupled_scenario_needs_every_file(self):
        def body(checkout):
            sections = checkout.sections()
            self.assertGreaterEqual(len(sections), 4)
            for drop in range(len(sections)):
                apply_sections(checkout.path, sections[:drop] + sections[drop + 1:])
                self.assertNotEqual(checkout.run().returncode, 0, f"{path_of(sections[drop][0])} was not needed")
                checkout.reset()
        self.each(body, "coupled")

    def test_a_coupled_scenarios_tests_do_not_dictate_its_inner_layout(self):
        def body(checkout):
            code = [p.read_text() for p in checkout.path.glob("*.py") if not p.name.startswith("test_")]
            tests = "".join(p.read_text() for p in checkout.path.glob("test_*.py"))
            known = set(re.findall(r"(?m)^\s*def (\w+)", "".join(code)))
            added = set(re.findall(r"(?m)^\+\s*def (\w+)", fix_of(checkout.scenario).read_text())) - known
            self.assertTrue(added, "the fix adds no inner call")
            for name in added:
                self.assertNotRegex(tests, rf"\b{name}\(", "the tests call an inner detail of one fix")
        self.each(body, "coupled")

    def test_an_independent_scenario_splits_into_parts_that_pass_alone(self):
        def body(checkout):
            parts = checkout.scenario["parts"]
            self.assertGreaterEqual(len(parts), 3)
            self.assertEqual(sorted(name for part in parts for name in part["files"]), sorted(checkout.scenario["files"]))
            sections = {path_of(header): (header, hunks) for header, hunks in checkout.sections()}
            for part in parts:
                apply_sections(checkout.path, [sections[name] for name in part["files"]])
                for other in parts:
                    said = checkout.run(other["accept"])
                    if other is part:
                        self.assertEqual(said.returncode, 0, f"{part['files']} does not stand alone: {said.stderr[-600:]}")
                    else:
                        self.assertNotEqual(said.returncode, 0, f"{other['files']} passed with only {part['files']} fixed")
                checkout.reset()
        self.each(body, "independent")

    def test_a_verification_scenarios_seed_looks_finished_and_the_judge_is_out_of_sight(self):
        def body(checkout):
            self.assertEqual(checkout.run([sys.executable, "-m", "unittest", "-q"]).returncode, 0, "the author's tests should pass")
            self.assertNotEqual(checkout.run().returncode, 0)
            self.assertTrue(HARNESS.is_hidden(checkout.scenario))
            names = {p.name for p in self.sources(checkout)}
            self.assertFalse({Path(word).name for word in checkout.scenario["accept"]} & names, "the judge is in the seed")
        self.each(body, "verification")

    def test_a_verification_scenario_refuses_a_missed_rule_and_a_wrong_one(self):
        def body(checkout):
            checkout.fix()
            findings = checkout.path / "findings.json"
            right = json.loads(findings.read_text())["violated"]
            spec = (checkout.path / "SPEC.md").read_text()
            clean = sorted(set(re.findall(r"\*\*(R\d+)\b", spec)) - set(right))
            self.assertGreaterEqual(len(clean), 3, "too few rules left clean to punish a guess")
            for wrong in (right[1:], [*right, clean[0]], []):
                findings.write_text(json.dumps({"violated": wrong}))
                self.assertNotEqual(checkout.run().returncode, 0, wrong)
            findings.write_text(json.dumps({"violated": right}))
            self.assertEqual(checkout.run().returncode, 0)
        self.each(body, "verification")

    def test_nothing_in_a_seed_gives_the_answer_away(self):
        def body(checkout):
            prose = []
            for path in self.sources(checkout):
                text = path.read_text()
                self.assertNotRegex(text, LEAK_WORDS, path.name)
                for line in text.splitlines():
                    self.assertNotRegex(line, COMMENTED_CODE, path.name)
                prose.append(prose_of(path, text))
            written = "\n".join(prose)
            for header, hunks in checkout.sections():
                lines = "".join(hunks).splitlines()
                for line in lines:
                    added = line[1:].strip()
                    if line.startswith("+") and len(added) >= SHORTEST_LEAK:
                        self.assertNotIn(added, written, f"a comment or text in the seed holds this line of {path_of(header)}")
        self.each(body)

    def test_nothing_reads_a_clock_or_a_network_and_only_the_standard_library_is_used(self):
        def body(checkout):
            checkout.fix()
            local = {p.stem for p in checkout.path.rglob("*.py")} | {p.name for p in checkout.path.iterdir() if p.is_dir()}
            judges = [Path(word) for word in HARNESS.accept_argv(checkout.scenario, TOOL) if word.endswith(".py")]
            for path in [*checkout.path.rglob("*.py"), *judges]:
                imported = imports_of(path.read_text())
                self.assertFalse(imported & NOT_DETERMINISTIC, path.name)
                self.assertEqual({m for m in imported - local if m not in sys.stdlib_module_names}, set(), path.name)
        self.each(body)

    def test_changing_anything_else_fails_acceptance_even_with_the_fix(self):
        def verdict(out, run):
            return json.loads((out / run["run"] / "accept.json").read_text())

        def body(number, scenario):
            out = Path(self.tmp.name) / f"out-{number}"
            self.eval_cli("plan", "--reps", "1", "--seed", "1", "--agent", "a", "--model", "m", "--effort", "e",
                          "--out", str(out), "--only", scenario["id"])
            self.eval_cli("prepare", "--out", str(out))
            first, second = json.loads((out / "plan.json").read_text())["runs"]
            self.eval_cli("accept", "--out", str(out))
            for run in (first, second):
                self.assertFalse(verdict(out, run)["passed"], "a seed passed")
                self.assertEqual(verdict(out, run)["outside"], [], "acceptance itself left a file behind")
                apply_fix(run["checkout"], scenario)
            self.eval_cli("accept", "--out", str(out))
            for run in (first, second):
                self.assertTrue(verdict(out, run)["passed"], verdict(out, run))
            root = Path(first["checkout"])
            edited = next(p for p in sorted(root.rglob("*")) if p.is_file() and ".git" not in p.parts and "__pycache__" not in p.parts
                          and p.relative_to(root).as_posix() not in scenario["files"])
            edited.write_text(edited.read_text() + "\n# touched\n")
            Path(second["checkout"], "stray.txt").write_text("not asked for\n")
            self.eval_cli("accept", "--out", str(out))
            for run, name in ((first, edited.relative_to(root).as_posix()), (second, "stray.txt")):
                self.assertFalse(verdict(out, run)["passed"])
                self.assertEqual(verdict(out, run)["outside"], [name])
            self.assertFalse(self.orc_calls.exists(), "the harness called zerocode-orc")

        self.each_scenario(body)

    def eval_cli(self, *args):
        said = subprocess.run([sys.executable, str(EVAL), *args], capture_output=True, text=True, env=self.env, timeout=300)
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=1)
