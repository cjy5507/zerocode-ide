import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

import tally

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[1]


def write(folder, name, body):
    with open(os.path.join(folder, name), "w", encoding="utf-8") as handle:
        handle.write(body)


def envelope(code, message="no"):
    """A refusal as the window writes it with --json: stderr, one line."""
    return json.dumps({"ok": False, "error": {"code": code, "message": message}})


def rows_file(folder, steps):
    write(folder, "steps.jsonl", "\n".join(json.dumps(row) for row in steps) + "\n")


class TallyTest(unittest.TestCase):
    def test_a_refusal_is_counted_by_its_code_not_its_text(self):
        # The audit's bug: the error is the whole stderr — an envelope, or a
        # plain sentence with no code — never a bare `<code>: …`.
        with tempfile.TemporaryDirectory() as run:
            rows_file(run, [
                {"n": 1, "verb": "mouse-click", "argv": ["mouse-click"], "ok": False, "error": envelope("confirmation_timeout")},
                {"n": 2, "verb": "key", "argv": ["key"], "ok": False, "error": envelope("stopped")},
                {"n": 3, "verb": "key", "argv": ["key"], "ok": False, "error": "zerocode-computer: the operator is stopped (hotkey); the person stopped it"},
            ])
            row = tally.measure(run)
            self.assertEqual((row["confirms"], row["stopped"]), (1, 1))
            self.assertEqual(row["codes"], {"confirmation_timeout": 1, "stopped": 1})

    def test_a_code_cut_at_the_windows_length_still_counts(self):
        cut = envelope("confirmation_required", "x" * 600)[:400]
        self.assertIsNone(tally.first_json_line(cut), "cut short, it is no longer JSON")
        self.assertEqual(tally.refusal_code({"error": cut}), "confirmation_required")
        self.assertEqual(tally.refusal_code({"code": "stopped", "error": cut}), "stopped", "the row's own code first")
        self.assertIsNone(tally.refusal_code({"error": "zerocode-computer: no answer"}))

    def test_a_confirming_press_counts_on_every_pressing_verb(self):
        with tempfile.TemporaryDirectory() as run:
            rows_file(run, [
                {"n": 1, "verb": "press-key", "argv": ["press-key", "--app", "X", "--key", "return", "--confirming", "delete"], "ok": True},
                {"n": 2, "verb": "hotkey", "argv": ["hotkey", "--app", "X", "--key", "cmd+return", "--confirming", "payment"], "ok": True},
                # A declared press the window then asked about is one step, once.
                {"n": 3, "verb": "mouse-click", "argv": ["mouse-click", "--confirming", "payment"], "ok": False, "error": envelope("confirmation_refused")},
            ])
            self.assertEqual(tally.measure(run)["confirms"], 3)

    def test_a_run_is_measured_from_its_files_alone(self):
        with tempfile.TemporaryDirectory() as root:
            run = os.path.join(root, "shop-cart", "evidence")
            os.makedirs(run)
            rows_file(run, [
                {"n": 1, "at_epoch_ms": 1000, "tool": "computer", "verb": "launch", "argv": ["launch"], "ok": True, "acts": True},
                {"n": 2, "at_epoch_ms": 2000, "tool": "computer", "verb": "mouse-click", "argv": ["mouse-click", "--confirming", "payment"], "ok": False, "error": envelope("confirmation_refused")},
                {"n": 3, "at_epoch_ms": 5000, "tool": "computer", "verb": "handoff", "argv": ["handoff"], "ok": True},
                {"n": 4, "at_epoch_ms": 9000, "tool": "computer", "verb": "verdict", "argv": ["verdict", "--pass"], "ok": True},
            ])
            write(run, "qa-verdict.json", json.dumps({"pass": True}))
            write(run, "state.json", json.dumps({"stuck": True}))
            row = tally.measure(run)
            self.assertEqual((row["success"], row["steps"], row["failed_steps"], row["acts"]), (True, 4, 1, 1))
            self.assertEqual((row["handoffs"], row["confirms"], row["stopped"], row["wall_ms"]), (1, 1, 1, 8000))
            self.assertEqual(tally.scenario_of(run), "evidence")
            write(os.path.join(root, "shop-cart"), "run.json", json.dumps({"automation_name": "bench:shop-cart"}))
            self.assertEqual(tally.scenario_of(run), "shop-cart")

    def test_a_bench_run_names_its_scenario_and_its_oracle_is_the_success(self):
        with tempfile.TemporaryDirectory() as root:
            def bench_run(name, **fields):
                run = os.path.join(root, name)
                os.makedirs(run)
                rows_file(run, [
                    {"n": 1, "at_epoch_ms": 1500, "verb": "screenshot", "argv": ["screenshot"], "ok": True},
                    {"n": 2, "at_epoch_ms": 2600, "verb": "type", "argv": ["type"], "ok": True, "acts": True},
                ])
                write(run, "qa-verdict.json", json.dumps({"pass": True}))
                body = {"scenario": "calculator-multiply", "lane": "live", "started_at_ms": 1000, "hands_proved": True,
                        "config": {"id": "default", "wire_model": "m1", "effort": None},
                        "oracle": {"pass": False}, "claimed": "PASS", "setup": {"ok": True},
                        "exit": {"code": 0, "timed_out": False}, "tool_calls": {"Computer": 4, "ToolSearch": 1},
                        "tokens_total": 5000, "run_wall_ms": 30000}
                body.update(fields)
                write(run, "bench-run.json", json.dumps(body))
                return tally.measure(run)
            row = bench_run("live-calculator-multiply-1")
            self.assertEqual(row["scenario"], "calculator-multiply", "the run's own name, not the folder's")
            self.assertEqual(row["success"], False, "the oracle decides, not qa-verdict.json")
            self.assertEqual((row["claim_mismatch"], row["tool_calls"], row["first_act_ms"]), (1, 5, 1600))
            self.assertEqual((row["config"], row["wire_model"]), ("default", "m1"), "a row is its config's, whatever model answered")
            self.assertIsNone(bench_run("unset", setup={"ok": False, "why": "window never came"})["success"])
            persons = bench_run("stopped", stopped_by="hotkey", oracle={"pass": True})
            self.assertEqual((persons["success"], persons["aborted"], persons["interventions"]), (None, True, 1))
            # A stop that is not the model's doing is not judged; the model's own is, and fails.
            for reason in ("window", "runner:SIGHUP", "chord_deaf", "request", "budget:session"):
                self.assertIsNone(bench_run(f"stop-{reason}", stopped_by=reason, oracle={"pass": True})["success"], reason)
            model = bench_run("model-stop", stopped_by="model:stop", oracle={"pass": True}, forbidden_verbs=["stop"])
            self.assertEqual((model["success"], model["aborted"], model["interventions"]), (False, False, 0),
                             "a model's own stop is its fault, not the person's hand")
            self.assertIs(bench_run("model-resume", stopped_by="model:resume", oracle={"pass": True})["success"], False)
            self.assertFalse(bench_run("off", oracle={"pass": True}, off_road=["Bash"])["success"])
            self.assertFalse(bench_run("late", oracle={"pass": True}, exit={"code": -15, "timed_out": True, "killed": True})["success"])
            self.assertEqual([tally.scenario_of(folder) for folder in tally.walk(root, bench_only=True)].count("calculator-multiply"), 12)

    def test_a_run_that_says_nothing_about_the_model_is_invalid(self):
        good = {"lane": "live", "hands_proved": True, "setup": {"ok": True}, "exit": {"code": 0}, "tokens_total": 10}
        self.assertIsNone(tally.invalid_reason(good))
        for change, why in (({"hands_proved": None}, "no hands proof"),
                            ({"exit": {"code": 1, "killed": False}}, "zo exited 1"),
                            ({"tokens_total": None}, "no model answer")):
            self.assertEqual(tally.invalid_reason(dict(good, **change)), why)
        self.assertIsNone(tally.invalid_reason(dict(good, exit={"code": -15, "killed": True, "timed_out": True})),
                          "zo the runner killed at the budget is the model's slowness")
        self.assertIsNone(tally.invalid_reason(dict(good, lane="hands", hands_proved=None)), "a hands run proves itself")
        self.assertIsNone(tally.invalid_reason(dict(good, setup={"ok": False}, tokens_total=None)), "a setup failure is its own count")
        with tempfile.TemporaryDirectory() as run:
            rows_file(run, [])
            write(run, "bench-run.json", json.dumps(dict(good, scenario="s", oracle={"pass": True}, config={"id": "c"}, tokens_total=None)))
            row = tally.measure(run)
            self.assertEqual((row["success"], row["invalid"], row["invalid_reason"]), (None, 1, "no model answer"))

    def test_the_summary_groups_by_scenario_lane_and_config_with_a_wilson_interval(self):
        base = {"scenario": "a", "lane": "live", "config": "c1", "wire_model": "m1", "tool_calls": 10, "first_act_ms": 2000,
                "run_wall_ms": 30000, "tokens_total": 4000, "interventions": 0, "steps": 12}
        rows = [dict(base, success=True) for _ in range(4)] + [dict(base, success=False, tool_calls=30)]
        rows += [dict(base, lane="hands", config="hands", wire_model=None, success=True),
                 dict(base, config="c2", wire_model=None, success=None, invalid=1)]
        summary = tally.summarize(rows)
        live = summary["a|live|c1"]
        self.assertEqual((live["runs"], live["judged"], live["passed"], live["success_rate"]), (5, 5, 4, 0.8))
        self.assertEqual(live["wilson"], [0.3755, 0.9638])
        self.assertEqual((live["median_tool_calls"], live["median_first_act_ms"], live["median_interventions"]), (10, 2000, 0))
        self.assertEqual(live["wire_models"], ["m1"])
        self.assertIn("a|hands|hands", summary)
        self.assertIsNone(summary["a|live|c2"]["success_rate"], "no judged run: invalid, never 0")
        table = tally.render(summary)
        self.assertIn("| a | live | c1 | m1 | 5 | 4/5 [0.38, 0.96] |", table)
        self.assertIn("| a | live | c2 | - | 1 | — |", table)
        self.assertTrue(table.splitlines()[-1].endswith("| 1 |"), "the invalid run is counted, not hidden")
        self.assertIn("computer bench", tally.markdown(summary))

    def test_a_baseline_comparison_says_up_only_when_the_intervals_part(self):
        def group(passed, n=5, tool_calls=10):
            rows = [{"scenario": "a", "lane": "live", "config": "-", "success": index < passed, "tool_calls": tool_calls} for index in range(n)]
            return tally.summarize(rows)
        self.assertEqual(tally.compare(group(5), group(0))["a|live|-"]["rate"], "up")
        self.assertEqual(tally.compare(group(5), group(2))["a|live|-"]["rate"], "within noise")
        self.assertEqual(tally.compare(group(0), group(5))["a|live|-"]["rate"], "down")
        self.assertEqual(tally.compare(group(5, tool_calls=12.5), group(5))["a|live|-"]["moved"], {"tool_calls": 25.0})
        self.assertEqual(tally.compare(group(5, tool_calls=11), group(5))["a|live|-"]["moved"], {})
        self.assertEqual(tally.compare(group(4, n=4, tool_calls=20), group(4, n=4))["a|live|-"]["moved"], {}, "too few runs to call a median")
        self.assertEqual(tally.compare(group(5), {})["a|live|-"]["rate"], "new")
        answered = lambda model: tally.summarize([{"scenario": "a", "lane": "live", "config": "c", "wire_model": model, "success": True}] * 5)
        self.assertEqual(tally.compare(answered("m2"), answered("m1"))["a|live|c"]["rate"], "model changed",
                         "a table another model answered is not compared")
        self.assertEqual(tally.compare(answered("m1"), answered("m1"))["a|live|c"]["rate"], "within noise")

    def test_an_ab_compares_two_configs_of_the_same_runs_per_scenario_and_pooled(self):
        rows = []
        for scenario in ("a", "b", "c"):
            for index in range(5):
                rows.append({"scenario": scenario, "lane": "live", "config": "default", "wire_model": "m1",
                             "success": index == 0, "tool_calls": 20})
                rows.append({"scenario": scenario, "lane": "live", "config": "marks", "wire_model": "m1",
                             "success": True, "tool_calls": 12})
        rows.append({"scenario": "a", "lane": "hands", "config": "hands", "success": True, "tool_calls": 3})
        verdicts = tally.versus(rows, "default", "marks")
        self.assertEqual(sorted(verdicts), ["*|live|marks", "a|live|marks", "b|live|marks", "c|live|marks"],
                         "only the two configs, and their pool")
        self.assertEqual(verdicts["a|live|marks"]["rate"], "within noise", "5/5 against 1/5: the intervals still meet")
        self.assertEqual(verdicts["*|live|marks"]["rate"], "up", "15/15 against 3/15 parts them")
        self.assertEqual(verdicts["*|live|marks"]["moved"], {"tool_calls": -40.0})
        self.assertEqual(tally.versus(rows, "marks", "default")["*|live|default"]["rate"], "down")
        with tempfile.TemporaryDirectory() as root:
            out = subprocess.run([sys.executable, str(HERE / "tally.py"), root, "--versus", "a:b", "--baseline", "x"],
                                 capture_output=True, text=True)
            self.assertEqual(out.returncode, 2, "one comparison at a time")

    def test_the_codes_and_files_are_the_products_own_words(self):
        protocol = (REPO / "crates/zerocode-core/src/computer_use_protocol/mod.rs").read_text()
        for code in tally.CONFIRM_CODES | tally.STOP_CODES:
            self.assertIn(f'= "{code}";', protocol, code)
        self.assertIn(f'STEPS_FILE: &str = "{tally.STEPS}";', (REPO / "crates/zerocode-shell/src/run_evidence.rs").read_text())
        self.assertIn(f'VERDICT_FILE: &str = "{tally.VERDICT}";', (REPO / "crates/zerocode-shell/src/computer_use/evidence.rs").read_text())
        self.assertIn(f'STATE_FILE: &str = "{tally.STATE}";', (REPO / "crates/zerocode-shell/src/computer_use/state.rs").read_text())
        evidence = (REPO / "crates/zerocode-shell/src/run_evidence.rs").read_text()
        self.assertIn(f'WALK_FILE_PREFIX: &str = "{tally.WALK_PREFIX}";', evidence)
        self.assertIn(f'WALK_FILE_EXTENSION: &str = "{tally.WALK_SUFFIX.lstrip(".")}";', evidence)
        core = (REPO / "crates/zerocode-core/src/computer_use.rs").read_text()
        self.assertIn("pub const PERSONS_STOP_REASONS: &[&str] = &[STOP_REASON_HOTKEY, STOP_REASON_WINDOW];", core)
        for reason, name in (("hotkey", "HOTKEY"), ("window", "WINDOW")):
            self.assertIn(f'pub const STOP_REASON_{name}: &str = "{reason}";', core)
        self.assertEqual(tally.PERSONS_STOP_REASONS, {"hotkey", "window"})
        self.assertIn(f'pub const PERSON_ASKED: &str = "{tally.PERSON_ASKED_CODE}";', protocol)

    def test_a_missing_or_broken_log_is_an_empty_run_not_a_crash(self):
        with tempfile.TemporaryDirectory() as root:
            write(root, "steps.jsonl", "not json\n\n{\"n\": 1, \"ok\": true, \"verb\": \"wait\"}\n")
            row = tally.measure(root)
            self.assertEqual((row["steps"], row["success"], row["wall_ms"]), (1, None, 0))
            self.assertEqual(tally.measure(os.path.join(root, "nowhere"))["steps"], 0)

    def test_stage_table_reads_walk_json_and_scales_to_the_human_floor(self):
        # A walk leaves walk-NNN.json beside steps.jsonl: per step the phases
        # the run closure timed (act/settle/verify/pointer) and the walk's own
        # ms; per run resolve/report. The stage table pools them, gives each
        # stage its share and p50/p95, and scales the median step against the
        # human floor (200 APM = 300 ms a step) — flygym's "x realtime" column.
        with tempfile.TemporaryDirectory() as run:
            rows_file(run, [
                {"n": 1, "at_epoch_ms": 1000, "tool": "computer", "verb": "click", "argv": ["click"], "ok": True, "acts": True},
                {"n": 2, "at_epoch_ms": 1600, "tool": "browser", "verb": "type", "argv": ["type"], "ok": True, "acts": True},
            ])
            write(run, "walk-001.json", json.dumps({
                "kind": "recipe-run", "at_epoch_ms": 1000, "elapsedMs": 930, "budgetMs": 60000,
                "phases": {"resolve": 20, "report": 8},
                "ran": [
                    {"step": 1, "tool": "computer", "verb": "click", "ok": True, "ms": 300, "evidence_n": 1,
                     "phases": {"act": 250, "settle": 30, "verify": 15, "pointer": 5}},
                    {"step": 2, "tool": "browser", "verb": "type", "ok": True, "ms": 600, "evidence_n": 2,
                     "phases": {"act": 500, "settle": 60, "verify": 30, "pointer": 10}},
                ],
            }))
            write(run, "walk-002.json", "not json")  # a broken walk is skipped, never a crash
            stages = tally.stages(run)
            self.assertEqual(stages["act"], [250, 500])
            self.assertEqual(stages["pointer"], [5, 10])
            self.assertEqual(stages["step"], [300, 600])
            self.assertEqual((stages["resolve"], stages["report"]), ([20], [8]))
            row = tally.measure(run)
            self.assertEqual(row["stages"]["settle"], [30, 60])
            values = tally.table()
            self.assertEqual(values["human_step_ms"], 300)
            summary = tally.summarize_stages([row], values)
            self.assertEqual(summary["act"]["n"], 2)
            self.assertEqual(summary["act"]["p50"], 375)
            self.assertEqual(summary["act"]["p95"], 500)
            # act 750 of 900 timed ms = 83.3%; the shares of the four step stages sum to 100.
            self.assertEqual(summary["act"]["share_pct"], 83.3)
            self.assertEqual(round(sum(summary[s]["share_pct"] for s in tally.STEP_STAGES), 1), 100.0)
            # median step 450 ms against the 300 ms human floor = 0.67x human speed.
            self.assertEqual(summary["step"]["p50"], 450)
            self.assertEqual(summary["step"]["x_human"], 0.67)
            table = tally.render_stages(summary)
            self.assertIn("| act |", table)
            self.assertIn("0.67", table)
            self.assertIn("83.3", table)
        # A folder without walks has an empty table, not an error.
        with tempfile.TemporaryDirectory() as bare:
            rows_file(bare, [{"n": 1, "verb": "key", "argv": ["key"], "ok": True}])
            self.assertEqual(tally.stages(bare)["step"], [])
            self.assertEqual(tally.summarize_stages([tally.measure(bare)], tally.table())["step"]["n"], 0)


if __name__ == "__main__":
    unittest.main()
