#!/usr/bin/env python3
"""Producer/verifier boundary tests with an explicitly fake ledger and local child commands.

Run with: python3 tools/release/tests/test_gate_receipt.py
Native authorization, immutability and reuse decisions are tested in Rust, not simulated here.
"""
import argparse
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "gate_receipt.py"
SPEC = importlib.util.spec_from_file_location("gate_receipt", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
PASS = b"test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
TAP = b"TAP version 13\nok 1 - fixture\n1..1\n# tests 1\n# suites 0\n# pass 1\n# fail 0\n# cancelled 0\n# skipped 0\n# todo 0\n# duration_ms 1.25\n"
SPEC_OUTPUT = "✔ fixture (1.25ms)\nℹ tests 1\nℹ suites 0\nℹ pass 1\nℹ fail 0\nℹ cancelled 0\nℹ skipped 0\nℹ todo 0\nℹ duration_ms 1.25\n".encode()
UNITTEST = b"..\n----------------------------------------------------------------------\nRan 2 tests in 0.001s\n\nOK\n"


class FakeLedger:
    """An observation spy, never an authentication implementation or offline fallback."""

    def __init__(self, refused=None):
        self.calls = []
        self.refused = refused
        self.metadata = None

    def call(self, verb, args, value, retry, extra=()):
        self.calls.append((verb, copy.deepcopy(value), retry))
        if verb == self.refused or self.refused == "gate-finish" and verb == "gate-record" and not extra:
            raise gate.UnknownInput("fake native refusal")
        if verb == "gate-register":
            return {"registration": "m-1", "lease": "gx-1", "registeredMs": 0}
        if verb == "gate-record" and extra:
            return {"execution": "m-start", "startedMs": 0}
        if verb == "gate-record":
            self.metadata = copy.deepcopy(value)
            return {"original": "m-2", "metadataSha256": gate.digest(gate.encoded(value))}
        if verb == "gate-reuse":
            return {"decision": "miss", "reason": "fake ledger never grants trust", "original": "m-2",
                    "decisionRecord": "m-3", "executed": False, "newExecutions": 0}
        raise AssertionError(verb)


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-q")
        self.git("config", "user.name", "dev")
        self.git("config", "user.email", "dev@example.com")
        (self.repo / "source.rs").write_text("fn main() {}\n")
        (self.repo / "Cargo.lock").write_text("version = 4\n")
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")
        self.deps = self.root / "deps"
        self.deps.mkdir()
        (self.deps / "dependency").write_bytes(b"locked dependency bytes")
        self.tools = self.root / "tools"
        self.tools.mkdir()
        (self.tools / "runtime").write_bytes(b"declared runtime fixture")
        self.args = argparse.Namespace(
            repo=str(self.repo), gate="gate-root", task="t-1", attempt="dp-1",
            scope="deterministic", closure="declared", profile="test-debug", clean_env=True,
            dependency_root=[str(self.deps)], toolchain_root=[str(self.tools)],
            lock_file=[], script_file=[], timeout=10, output=str(self.root / "out"),
            command=[sys.executable, "-c", f"import sys; sys.stdout.buffer.write({PASS!r})"],
            metadata=str(self.root / "out/metadata.json"), log=str(self.root / "out/gate.log"),
        )

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], check=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout

    def fingerprint(self, environment=None):
        return gate.fingerprint(self.args, gate.execution_environment(self.args) if environment is None else environment, self.args.command)

    def test_producer_runs_a_real_child_only_after_native_registration(self):
        marker = self.root / "executed"
        self.args.command = [sys.executable, "-c",
            f"from pathlib import Path; import sys; Path({str(marker)!r}).write_text('ran'); sys.stdout.buffer.write({PASS!r})"]
        ledger = FakeLedger()
        original_call = ledger.call

        def observed_call(verb, args, value, retry, extra=()):
            if verb == "gate-register":
                self.assertFalse(marker.exists())
            if verb == "gate-record" and not extra:
                self.assertEqual(marker.read_text(), "ran")
                self.assertEqual(gate.digest(Path(args.metadata).read_bytes()), gate.digest(gate.encoded(value)))
                self.assertEqual(gate.digest(Path(args.log).read_bytes()), value["logSha256"])
            return original_call(verb, args, value, retry, extra=extra)

        ledger.call = observed_call
        result = gate.produce(self.args, ledger)
        self.assertEqual(result["state"], "recorded")
        self.assertEqual(result["tests"], 2)
        self.assertTrue(ledger.metadata["inputsUnchanged"])
        self.assertEqual([call[0] for call in ledger.calls], ["gate-register", "gate-record", "gate-record"])
        self.assertNotEqual(ledger.calls[0][2], ledger.calls[1][2])

    def test_a_native_refusal_before_execution_does_not_run_the_command(self):
        marker = self.root / "must-not-exist"
        self.args.command = [sys.executable, "-c", f"open({str(marker)!r}, 'w').close()"]
        with self.assertRaises(gate.UnknownInput):
            gate.produce(self.args, FakeLedger(refused="gate-register"))
        self.assertFalse(marker.exists())

    def test_a_failed_native_completion_is_unattested_after_a_real_execution(self):
        with self.assertRaises(gate.ExecutionUnattested):
            gate.produce(self.args, FakeLedger(refused="gate-finish"))
        self.assertTrue(Path(self.args.metadata).is_file())

    def test_verification_recomputes_bytes_and_never_calls_the_execution_function(self):
        ledger = FakeLedger()
        gate.produce(self.args, ledger)
        metadata = Path(self.args.metadata).read_bytes()
        with mock.patch.object(gate, "execute", side_effect=AssertionError("verify executed a gate")):
            result = gate.verify(self.args, ledger)
        self.assertEqual(result["decision"], "miss")
        self.assertEqual(result["newExecutions"], 0)
        verb, observations, _ = ledger.calls[-1]
        self.assertEqual(verb, "gate-reuse")
        self.assertEqual(observations["metadataSha256"], gate.digest(metadata))
        self.assertEqual(observations["logSha256"], gate.digest(Path(self.args.log).read_bytes()))

    def test_tampered_metadata_and_log_are_observations_not_local_cache_hits(self):
        ledger = FakeLedger()
        gate.produce(self.args, ledger)
        original = Path(self.args.metadata).read_bytes()
        Path(self.args.metadata).write_bytes(original + b"\n")
        Path(self.args.log).write_bytes(PASS + b"changed\n")
        self.assertEqual(gate.verify(self.args, ledger)["decision"], "miss")
        observations = ledger.calls[-1][1]
        self.assertNotEqual(observations["metadataSha256"], gate.digest(original))
        self.assertEqual(observations["logSha256"], gate.digest(PASS + b"changed\n"))

    def test_missing_native_ledger_has_no_json_file_fallback(self):
        gate.produce(self.args, FakeLedger())
        with mock.patch.object(gate, "Ledger", side_effect=gate.UnknownInput("not connected")):
            with self.assertRaises(gate.UnknownInput):
                gate.verify(self.args)

    def test_failed_timeout_and_zero_test_executions_are_recorded_truthfully(self):
        for label, command, outcome, count in [
            ("failure", [sys.executable, "-c", "raise SystemExit(7)"], "failed", 0),
            ("zero", [sys.executable, "-c", "print('no tests')"], "passed", 0),
            ("timeout", [sys.executable, "-c", "import time; time.sleep(10)"], "timed_out", 0),
        ]:
            with self.subTest(label=label):
                self.args.output = str(self.root / label)
                self.args.command = command
                self.args.timeout = 0.1 if label == "timeout" else 10
                ledger = FakeLedger()
                result = gate.produce(self.args, ledger)
                self.assertEqual(result["outcome"], outcome)
                self.assertEqual(result["tests"], count)

    def test_source_changes_during_execution_are_not_hidden_by_rc_zero(self):
        self.args.command = [sys.executable, "-c",
            f"from pathlib import Path; import sys; Path('source.rs').write_text('changed'); sys.stdout.buffer.write({PASS!r})"]
        ledger = FakeLedger()
        gate.produce(self.args, ledger)
        self.assertFalse(ledger.metadata["inputsUnchanged"])

    def test_each_real_fingerprint_dimension_reacts_to_its_input(self):
        before = self.fingerprint()
        self.assertTrue(all(before[key] for key in ("source", "deps", "lock", "script", "recipe", "toolchain", "platform", "profile", "env")))
        (self.deps / "dependency").write_bytes(b"changed dependency")
        self.assertNotEqual(self.fingerprint()["deps"], before["deps"])
        (self.tools / "runtime").write_bytes(b"changed runtime")
        self.assertNotEqual(self.fingerprint()["toolchain"], before["toolchain"])
        (self.repo / "Cargo.lock").write_text("version = 3\n")
        self.assertNotEqual(self.fingerprint()["lock"], before["lock"])
        self.assertIsNone(self.fingerprint()["source"])
        self.assertNotEqual(self.fingerprint({"PATH": os.environ.get("PATH", ""), "GATE_OPTION": "different"})["env"], before["env"])
        self.args.profile = "test-release"
        self.assertNotEqual(self.fingerprint()["profile"], before["profile"])
        self.args.command += ["changed-command-argument"]
        self.assertNotEqual(self.fingerprint()["recipe"], before["recipe"])
        with mock.patch.object(gate.platform, "version", return_value="changed platform"):
            self.assertNotEqual(self.fingerprint()["platform"], before["platform"])
        extra_script = self.root / "gate-script.py"
        extra_script.write_text("pass\n")
        self.args.script_file = [str(extra_script)]
        first_script = self.fingerprint()["script"]
        extra_script.write_text("raise SystemExit(1)\n")
        self.assertNotEqual(self.fingerprint()["script"], first_script)

    def test_same_tree_new_head_is_observed_as_a_different_input(self):
        before = self.fingerprint()
        self.git("commit", "--allow-empty", "-qm", "same tree new commit")
        after = self.fingerprint()
        self.assertEqual(after["tree"], before["tree"])
        self.assertEqual(after["source"], before["source"])
        self.assertNotEqual(after["head"], before["head"])

    def test_ambient_git_environment_cannot_redirect_the_source_observation(self):
        before = self.fingerprint()
        with mock.patch.dict(os.environ, {"GIT_DIR": str(self.root / "wrong-git"), "GIT_WORK_TREE": str(self.root / "wrong-tree")}):
            after = self.fingerprint()
        self.assertEqual(after["head"], before["head"])
        self.assertEqual(after["source"], before["source"])

    def test_unknown_input_closure_and_absent_dependencies_stay_unknown(self):
        self.args.closure = "unknown"
        unknown = self.fingerprint()
        self.assertIsNone(unknown["deps"])
        self.assertIsNone(unknown["script"])
        self.args.closure = "declared"
        self.args.dependency_root = []
        self.args.toolchain_root = []
        unknown = self.fingerprint()
        self.assertIsNone(unknown["deps"])
        self.assertIsNone(unknown["toolchain"])

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_a_link_or_missing_input_does_not_produce_a_known_digest(self):
        (self.deps / "linked").symlink_to(self.repo / "Cargo.lock")
        self.assertIsNone(self.fingerprint()["deps"])
        with self.assertRaises(gate.UnknownInput):
            gate.file_digest(self.deps / "linked")

    def test_private_environment_values_are_masked_and_their_identity_stays_unknown(self):
        self.args.clean_env = False
        secret = "synthetic-test-secret-value"
        with mock.patch.dict(os.environ, {"GATE_TEST_SECRET": secret}):
            self.args.command = [sys.executable, "-c", f"import os; print(os.environ['GATE_TEST_SECRET']); print({PASS.decode()!r})"]
            ledger = FakeLedger()
            gate.produce(self.args, ledger)
        self.assertNotIn(secret.encode(), Path(self.args.metadata).read_bytes())
        self.assertNotIn(secret.encode(), Path(self.args.log).read_bytes())
        self.assertNotIn("GATE_TEST_SECRET", json.dumps(ledger.metadata))
        self.assertIsNone(ledger.metadata["inputs"]["env"])

    def test_output_in_the_source_checkout_is_refused(self):
        self.args.output = str(self.repo / "output")
        with self.assertRaises(gate.UnknownInput):
            gate.produce(self.args, FakeLedger())
        self.assertFalse(Path(self.args.output).exists())

    def test_test_counts_require_complete_passing_nonmeasured_summaries(self):
        self.assertEqual(gate.test_count(PASS), 2)
        self.assertEqual(gate.test_count(PASS + PASS), 4)
        for log in (b"rc=0", PASS[:-15], PASS.replace(b"2 passed", b"0 passed"),
                    PASS.replace(b"ok.", b"FAILED."), PASS.replace(b"0 failed", b"1 failed"),
                    PASS.replace(b"0 measured", b"1 measured"), PASS + b"SKIPPED suite\n",
                    PASS + b"test result: ok. truncated\n", PASS + b"error: test failed\n",
                    b"==> verify recipe one\n" + PASS,
                    b"==> verify recipe one\n" + PASS + b"<== verify recipe one rc=1 2s\n"):
            with self.subTest(log=log):
                self.assertEqual(gate.test_count(log), 0)

    def test_masking_cannot_turn_a_failed_summary_into_passing_evidence(self):
        self.args.clean_env = False
        output = PASS + PASS.replace(b"ok.", b"FAILED.")
        self.args.command = [sys.executable, "-c", f"import sys; sys.stdout.buffer.write({output!r})"]
        with mock.patch.dict(os.environ, {"GATE_TEST_SECRET": "FAILED"}):
            ledger = FakeLedger()
            gate.produce(self.args, ledger)
        self.assertEqual(ledger.metadata["tests"], 0)

    def test_child_never_inherits_fixture_ledger_or_account_authority(self):
        private = {
            "HOME": str(self.root / "account-home"),
            "ZEROCODE_AGENT_TEAM_TOKEN": "fixture-pane-capability",
            "ZEROCODE_AGENT_TEAM_TOKEN_FILE": str(self.root / "fixture-auth"),
            "ZEROCODE_ORCHESTRATION_RUN": "fixture-run",
            "CLAUDE_CODE_MESSAGING_TOKEN": "fixture-message-capability",
            "HERDR_TEST_HOOK": "fixture-hook-capability",
        }
        self.args.child_home = str(self.root / "child-home")
        with mock.patch.dict(os.environ, private):
            for clean in (False, True):
                with self.subTest(clean=clean):
                    self.args.clean_env = clean
                    child = gate.execution_environment(self.args)
                    self.assertFalse(set(private).difference({"HOME"}).intersection(child))
                    self.assertEqual(child.get("HOME"), self.args.child_home)
                    self.assertNotEqual(child.get("HOME"), private["HOME"])

    def test_candidate_owned_producer_is_refused_before_a_child_runs(self):
        with mock.patch.object(gate, "__file__", str(self.repo / "producer.py")):
            with mock.patch.object(gate, "execute", wraps=gate.execute) as executed:
                try:
                    gate.produce(self.args, FakeLedger())
                except gate.UnknownInput:
                    pass
                self.assertEqual(executed.call_count, 0, "candidate producer reached execution")

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_tracked_internal_links_bind_their_text_mode_and_reachable_input(self):
        folder = self.repo / "tools/computer-bench/long-form/bin"
        folder.mkdir(parents=True)
        (folder / "zerocode-door").write_text("fixture door\n")
        for name in ("zerocode-browser", "zerocode-computer"):
            (folder / name).symlink_to("zerocode-door")
        self.git("add", ".")
        self.git("commit", "-qm", "internal links")
        before = self.fingerprint()
        self.assertIsNotNone(before["source"], "safe tracked internal links lost source identity")
        self.assertIsNotNone(before["tree"])
        (folder / "zerocode-door").write_text("changed reachable input\n")
        self.assertIsNone(self.fingerprint()["source"])

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_outside_dangling_cyclic_and_untracked_link_targets_remain_unknown(self):
        for target in (str(self.deps / "dependency"), "absent", "link", "ignored-input"):
            with self.subTest(target=target):
                link = self.repo / "link"
                if link.is_symlink():
                    link.unlink()
                link.symlink_to(target)
                (self.repo / ".gitignore").write_text("ignored-input\n")
                (self.repo / "ignored-input").write_text("undeclared input\n")
                self.git("add", ".")
                self.git("commit", "-qm", "unsafe link")
                self.assertIsNone(self.fingerprint()["source"])

    def test_complete_mixed_runner_output_counts_zero_skip_and_cancel_summaries(self):
        for log, count in ((TAP, 1), (SPEC_OUTPUT, 1), (UNITTEST, 2), (PASS + TAP + SPEC_OUTPUT + UNITTEST, 6)):
            with self.subTest(log=log):
                self.assertEqual(gate.test_count(log), count)

    def test_mixed_runner_failures_skips_cancellation_and_truncation_are_unknown(self):
        for log in (
            TAP.replace(b"# fail 0", b"# fail 1"),
            TAP.replace(b"# cancelled 0", b"# cancelled 1"),
            TAP.replace(b"# skipped 0", b"# skipped 1"),
            TAP.replace(b"# todo 0", b"# todo 1"),
            TAP.replace(b"# duration_ms 1.25\n", b""),
            SPEC_OUTPUT.replace("ℹ pass 1".encode(), "ℹ pass 0".encode()),
            UNITTEST.replace(b"OK\n", b"OK (skipped=1)\n"),
            UNITTEST.replace(b"OK\n", b"FAILED (failures=1)\n"),
            b"Ran 2 tests in 0.001s\n", b"TAP version 13\nok 1 - truncated\n",
        ):
            with self.subTest(log=log):
                self.assertEqual(gate.test_count(PASS + log), 0)

    def test_real_fixture_process_emits_complete_mixed_runner_summaries(self):
        output = PASS + TAP + SPEC_OUTPUT + UNITTEST
        self.args.command = [sys.executable, "-c", f"import sys; sys.stdout.buffer.write({output!r})"]
        ledger = FakeLedger()
        result = gate.produce(self.args, ledger)
        self.assertEqual(result["tests"], 6)
        self.assertEqual(Path(self.args.log).read_bytes(), output)

    def test_zero_executed_tests_cannot_make_the_producer_exit_successfully(self):
        self.args.command = [sys.executable, "-c", "print('no tests executed')"]
        argv = [str(SCRIPT), "run", "--repo", self.args.repo, "--gate", "gate-root", "--task", "t-1",
                "--attempt", "none", "--output", self.args.output, "--", *self.args.command]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(gate, "Ledger", return_value=FakeLedger()):
            self.assertEqual(gate.main(), 1, "rc0 with zero tests reported gate success")

    def test_fingerprint_head_and_source_are_from_the_same_snapshot(self):
        original = gate.source_identity
        observed_source = []

        def changed_source(repo):
            self.git("commit", "--allow-empty", "-qm", "move before snapshot")
            identity = original(repo)
            observed_source.append(identity)
            return identity

        with mock.patch.object(gate, "source_identity", side_effect=changed_source):
            result = self.fingerprint()
        self.assertTrue(result["source"] is None or result["head"] == observed_source[0][0],
                        "HEAD and source identity name different commits")


if __name__ == "__main__":
    unittest.main()
