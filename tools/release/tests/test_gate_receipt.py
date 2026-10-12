#!/usr/bin/env python3
"""Producer/verifier boundary tests with an explicitly fake ledger and local child commands.

Run with: python3 tools/release/tests/test_gate_receipt.py
Native authorization, immutability and reuse decisions are tested in Rust, not simulated here.
"""
import argparse
import copy
import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
from gate_fixture import authorize, load_module

SCRIPT = Path(__file__).resolve().parents[1] / "gate_receipt.py"
SPEC = importlib.util.spec_from_file_location("gate_receipt", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
PASS = b"test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
TAP = b"TAP version 13\nok 1 - fixture\n1..1\n# tests 1\n# suites 0\n# pass 1\n# fail 0\n# cancelled 0\n# skipped 0\n# todo 0\n# duration_ms 1.25\n"
SPEC_OUTPUT = "✔ fixture (1.25ms)\nℹ tests 1\nℹ suites 0\nℹ pass 1\nℹ fail 0\nℹ cancelled 0\nℹ skipped 0\nℹ todo 0\nℹ duration_ms 1.25\n".encode()
UNITTEST = b"..\n----------------------------------------------------------------------\nRan 2 tests in 0.001s\n\nOK\n"


class RuntimeLocationTests(unittest.TestCase):
    def test_mismatched_engine_identities_are_refused_with_pinned_executable(self):
        launcher = load_module(SCRIPT.with_name("gate_launcher.py"), "engine_identity_launcher")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            runtime = root / "runtime"
            python = runtime / "bin/python"
            python.parent.mkdir(parents=True)
            python.write_bytes(b"pinned engine fixture")
            other = root / "other-engine"
            other.write_bytes(b"different engine fixture")
            stdlib = runtime / "lib/python-fixture"
            dynload = stdlib / "lib-dynload"
            dynload.mkdir(parents=True)
            modules = {name: stdlib / (name + ".py") for name in ("json", "encodings")}
            for path in modules.values():
                path.write_bytes(b"")
            paths = [str(stdlib), str(dynload)]
            Path(str(python) + "._pth").write_text("\n".join(paths) + "\n")
            facts = SimpleNamespace(
                executable=str(python), orig_argv=[str(python)], _base_executable=str(python), path=paths,
                prefix=str(python.parent), base_prefix=str(python.parent),
                exec_prefix=str(python.parent), base_exec_prefix=str(python.parent),
                _stdlib_dir=str(python.parent / "lib/python-fixture"),
            )
            policy = {"runtime": {"python": {"path": str(python)}, "stdlib": {"path": str(runtime)}}}
            with mock.patch.multiple(launcher, sys=facts,
                                     json=SimpleNamespace(__file__=str(modules["json"])),
                                     encodings=SimpleNamespace(__file__=str(modules["encodings"]))):
                launcher.runtime_locations(policy)
                for name, value in (("orig_argv", [str(other)]), ("_base_executable", str(other))):
                    with self.subTest(identity=name), mock.patch.object(facts, name, value):
                        self.assertEqual(facts.executable, str(python))
                        with self.assertRaises(launcher.Untrusted) as denied:
                            launcher.runtime_locations(policy)
                        self.assertEqual(denied.exception.code, "digest_mismatch")
                launcher.runtime_locations(policy)


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
            return {"registration": "m-1", "policyPin": "m-fixture-policy", "lease": "gx-1", "registeredMs": 0}
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
        self.root = Path(self.temp.name).resolve()
        (self.root / "git-home").mkdir()
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
        authorize(self.args, self.root, gate)

    def produce(self, ledger):
        authorize(self.args, self.root, gate)
        return gate.produce(self.args, ledger)

    def repin_policy(self, policy):
        launcher = load_module(SCRIPT.with_name("gate_launcher.py"), "repinned_fixture_launcher")
        path = self.root / "repinned-policy.json"
        raw = gate.encoded(policy)
        path.write_bytes(raw)
        context = launcher.load_policy(path, gate.digest(raw), self.repo)
        context["repo"] = str(self.repo)
        context["configure"] = lambda parsed: launcher.configure(context, parsed, gate)
        self.args.trust = context

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], check=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              env={"HOME": str(self.root / "git-home"), "PATH": os.defpath,
                                   "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}).stdout

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
        result = self.produce(ledger)
        self.assertEqual(result["state"], "recorded")
        self.assertEqual(result["tests"], 2)
        self.assertTrue(ledger.metadata["inputsUnchanged"])
        self.assertEqual([call[0] for call in ledger.calls], ["gate-register", "gate-record", "gate-record"])
        self.assertNotEqual(ledger.calls[0][2], ledger.calls[1][2])

    def test_a_native_refusal_before_execution_does_not_run_the_command(self):
        marker = self.root / "must-not-exist"
        self.args.command = [sys.executable, "-c", f"open({str(marker)!r}, 'w').close()"]
        with self.assertRaises(gate.UnknownInput):
            self.produce(FakeLedger(refused="gate-register"))
        self.assertFalse(marker.exists())

    def test_a_failed_native_completion_is_unattested_after_a_real_execution(self):
        with self.assertRaises(gate.ExecutionUnattested):
            self.produce(FakeLedger(refused="gate-finish"))
        self.assertTrue(Path(self.args.metadata).is_file())

    def test_verification_recomputes_bytes_and_never_calls_the_execution_function(self):
        ledger = FakeLedger()
        self.produce(ledger)
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
        self.produce(ledger)
        original = Path(self.args.metadata).read_bytes()
        Path(self.args.metadata).write_bytes(original + b"\n")
        Path(self.args.log).write_bytes(PASS + b"changed\n")
        self.assertEqual(gate.verify(self.args, ledger)["decision"], "miss")
        observations = ledger.calls[-1][1]
        self.assertNotEqual(observations["metadataSha256"], gate.digest(original))
        self.assertEqual(observations["logSha256"], gate.digest(PASS + b"changed\n"))

    def test_missing_native_ledger_has_no_json_file_fallback(self):
        self.produce(FakeLedger())
        with mock.patch.object(gate, "Ledger", side_effect=gate.UnknownInput("not connected")):
            with self.assertRaises(gate.UnknownInput):
                gate.verify(self.args)

    def test_failed_timeout_and_zero_test_executions_are_recorded_truthfully(self):
        for label, command, outcome, count in [
            ("failure", [sys.executable, "-c", "raise SystemExit(7)"], "failed", 0),
            ("zero", [sys.executable, "-c", "print('no tests')"], "unknown", 0),
            ("timeout", [sys.executable, "-c", "import time; time.sleep(10)"], "timed_out", 0),
        ]:
            with self.subTest(label=label):
                self.args.output = str(self.root / label)
                self.args.command = command
                self.args.timeout = 0.1 if label == "timeout" else 10
                ledger = FakeLedger()
                result = self.produce(ledger)
                self.assertEqual(result["outcome"], outcome)
                self.assertEqual(result["tests"], count)

    def test_source_changes_during_execution_are_not_hidden_by_rc_zero(self):
        self.args.command = [sys.executable, "-c",
            f"from pathlib import Path; import sys; Path('source.rs').write_text('changed'); sys.stdout.buffer.write({PASS!r})"]
        ledger = FakeLedger()
        self.produce(ledger)
        self.assertFalse(ledger.metadata["inputsUnchanged"])

    def test_each_real_fingerprint_dimension_reacts_to_its_input(self):
        before = self.fingerprint()
        self.assertTrue(all(before[key] for key in ("source", "deps", "lock", "script", "recipe", "toolchain", "platform", "profile", "env")))
        for path, key in ((self.deps / "dependency", "deps"), (self.tools / "runtime", "toolchain"),
                          (self.repo / "Cargo.lock", "lock")):
            with self.subTest(dimension=key):
                saved = path.read_bytes()
                self.assertEqual(self.fingerprint(), before)
                path.write_bytes(b"changed input\n")
                self.assertIsNone(self.fingerprint()["policy"])
                self.assertNotEqual(self.fingerprint()[key], before[key])
                path.write_bytes(saved)
                self.assertEqual(self.fingerprint(), before)
        changed_environment = {**gate.execution_environment(self.args), "LANG": "different"}
        self.assertIsNone(self.fingerprint(changed_environment)["env"])
        self.assertEqual(self.fingerprint(), before)
        self.args.profile = "test-release"
        authorize(self.args, self.root, gate)
        changed = self.fingerprint()
        self.assertIsNotNone(changed["policy"])
        self.assertNotEqual(changed["profile"], before["profile"])
        self.args.command += ["changed-command-argument"]
        authorize(self.args, self.root, gate)
        changed_command = self.fingerprint()
        self.assertIsNotNone(changed_command["recipe"])
        self.assertNotEqual(changed_command["recipe"], changed["recipe"])
        with mock.patch.object(gate.platform, "version", return_value="changed platform"):
            self.assertNotEqual(self.fingerprint()["platform"], before["platform"])
        extra_script = self.root / "gate-script.py"
        extra_script.write_text("pass\n")
        self.args.script_file = [str(extra_script)]
        authorize(self.args, self.root, gate)
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
        self.args.trust = None
        unknown = self.fingerprint()
        self.assertIsNone(unknown["deps"])
        self.assertIsNone(unknown["toolchain"])

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_a_link_or_missing_input_does_not_produce_a_known_digest(self):
        (self.deps / "linked").symlink_to(self.repo / "Cargo.lock")
        self.assertIsNone(self.fingerprint()["deps"])
        with self.assertRaises(gate.UnknownInput):
            gate.file_digest(self.deps / "linked")

    def test_ambient_private_environment_is_excluded_from_the_child(self):
        self.args.clean_env = False
        secret = "synthetic-test-secret-value"
        with mock.patch.dict(os.environ, {"GATE_TEST_SECRET": secret}):
            self.args.command = [sys.executable, "-c", f"import os; print(os.environ.get('GATE_TEST_SECRET', 'no inherited value')); print({PASS.decode()!r})"]
            ledger = FakeLedger()
            self.produce(ledger)
        self.assertNotIn(secret.encode(), Path(self.args.metadata).read_bytes())
        self.assertNotIn(secret.encode(), Path(self.args.log).read_bytes())
        self.assertNotIn("GATE_TEST_SECRET", json.dumps(ledger.metadata))
        self.assertIsNotNone(ledger.metadata["inputs"]["env"])

    def test_log_masking_has_a_control_that_really_contains_the_private_value(self):
        raw = b"fixture-private-value\n" + PASS
        self.assertEqual(gate.mask_log(raw, {}, self.args.trust), raw)
        masked = gate.mask_log(raw, {"GATE_TEST_SECRET": "fixture-private-value"}, self.args.trust)
        self.assertNotIn(b"fixture-private-value", masked)
        self.assertIn(b"[private]", masked)
        self.assertEqual(gate.test_count(masked), 2)

    def test_output_in_the_source_checkout_is_refused(self):
        self.args.output = str(self.repo / "output")
        with self.assertRaises(gate.UnknownInput):
            self.produce(FakeLedger())
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
            self.produce(ledger)
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

    @unittest.skipIf(os.name == "nt", "private inherited descriptors require POSIX")
    def test_the_first_gate_process_detects_a_deliberately_inherited_socket(self):
        import socket
        parent, child = socket.socketpair()
        with parent, child:
            command = self.args.command
            environment = gate.execution_environment(self.args)
            leaked = subprocess.run(command, cwd=self.repo, env=environment, pass_fds=(child.fileno(),),
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30, check=False)
            self.assertNotEqual(leaked.returncode, 0, leaked.stdout)
            self.assertIn(b"fixture first child inherited socket descriptor", leaked.stdout)
            sterile = subprocess.run(command, cwd=self.repo, env=environment,
                                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30, check=False)
            self.assertEqual(sterile.returncode, 0, sterile.stdout)
            self.assertEqual(gate.test_count(sterile.stdout), 2, sterile.stdout)

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
        (folder / "zerocode-door").write_text("fixture door\n")
        (folder / "zerocode-browser").unlink()
        (folder / "zerocode-browser").symlink_to("./zerocode-door")
        self.git("add", ".")
        self.git("commit", "-qm", "link text changed")
        changed_text = self.fingerprint()
        self.assertIsNotNone(changed_text["source"])
        self.assertNotEqual(changed_text["source"], before["source"])
        (folder / "zerocode-door").chmod(0o755)
        self.git("add", ".")
        self.git("commit", "-qm", "target mode changed")
        self.assertNotEqual(self.fingerprint()["source"], changed_text["source"])

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
            TAP + TAP.replace(b"1..1\n", b""),
            SPEC_OUTPUT.splitlines()[0] + b"\n",
        ):
            with self.subTest(log=log):
                self.assertEqual(gate.test_count(PASS + log), 0)

    def test_real_fixture_process_emits_complete_mixed_runner_summaries(self):
        output = PASS + TAP + SPEC_OUTPUT + UNITTEST
        self.args.command = [sys.executable, "-c", f"import sys; sys.stdout.buffer.write({output!r})"]
        ledger = FakeLedger()
        result = self.produce(ledger)
        self.assertEqual(result["tests"], 6)
        self.assertIn(output, Path(self.args.log).read_bytes())

    def test_zero_executed_tests_cannot_make_the_producer_exit_successfully(self):
        marker = self.root / "zero-run-started"
        self.args.command = [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).write_text('ran'); print('no tests executed')"]
        trust = authorize(self.args, self.root, gate)
        argv = [str(SCRIPT), "run", "--repo", self.args.repo, "--gate", "gate-root", "--task", "t-1",
                "--attempt", "none", "--output", self.args.output, "--profile", self.args.profile,
                "--timeout", str(self.args.timeout),
                "--scope", "deterministic", "--closure", "declared", "--", *self.args.command]
        ledger = FakeLedger()
        captured = io.StringIO()
        with mock.patch.object(sys, "argv", argv), mock.patch.object(gate, "Ledger", return_value=ledger), contextlib.redirect_stdout(captured):
            self.assertEqual(gate.main(trust=trust), 1, "rc0 with zero tests reported gate success")
        self.assertEqual(marker.read_text(), "ran")
        answer = json.loads(captured.getvalue())
        self.assertEqual(answer["state"], "recorded")
        self.assertEqual(answer["outcome"], "unknown")
        self.assertEqual(answer["tests"], 0)
        self.assertTrue(ledger.metadata["executed"])
        self.assertEqual(ledger.metadata["rc"], 0)
        self.args.command = [sys.executable, "-c", f"import sys; sys.stdout.buffer.write({PASS!r})"]
        self.args.output = str(self.root / "nonzero-control")
        self.assertEqual(self.produce(FakeLedger())["outcome"], "passed")

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

    def test_unverified_producer_cannot_register_or_execute(self):
        self.args.trust = None
        ledger = FakeLedger()
        with mock.patch.object(gate, "execute", side_effect=AssertionError("unverified code executed")):
            with self.assertRaises(gate.UnknownInput):
                gate.produce(self.args, ledger)
        self.assertEqual(ledger.calls, [])

    def test_an_external_copy_is_untrusted_until_its_bytes_match_independent_pins(self):
        launcher = load_module(SCRIPT.with_name("gate_launcher.py"), "external_copy_launcher")
        policy = copy.deepcopy(self.args.trust["policy"])
        copied = self.root / "external-producer.py"
        copied.write_text("raise AssertionError('untrusted producer imported')\n")
        policy["bundle"]["producer"]["path"] = str(copied)
        raw = gate.encoded(policy)
        path = self.root / "copy-policy.json"
        path.write_bytes(raw)
        with self.assertRaises(launcher.Untrusted):
            launcher.load_policy(path, gate.digest(raw), self.repo)

    def test_candidate_scanner_and_launcher_are_refused_even_with_declared_digests(self):
        launcher = load_module(SCRIPT.with_name("gate_launcher.py"), "candidate_bundle_launcher")
        for role in ("launcher", "producer", "scanner", "rules"):
            with self.subTest(role=role):
                policy = copy.deepcopy(self.args.trust["policy"])
                copied = self.repo / (role + ".py")
                copied.write_bytes(self.args.trust["bytes"][role])
                policy["bundle"][role] = {"path": str(copied), "sha256": gate.file_digest(copied)}
                raw = gate.encoded(policy)
                path = self.root / "candidate-policy.json"
                path.write_bytes(raw)
                with self.assertRaises(launcher.Untrusted):
                    launcher.load_policy(path, gate.digest(raw), self.repo)

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_runtime_library_aliases_bind_targets_bytes_and_policy(self):
        launcher = load_module(SCRIPT.with_name("gate_launcher.py"), "library_fixture_launcher")
        target = self.root / "runtime-library"
        other = self.root / "other-runtime-library"
        target.write_bytes(b"runtime library fixture bytes")
        other.write_bytes(b"other runtime library fixture bytes")
        aliases = [self.root / "loader-a", self.root / "loader-b"]
        for alias in aliases:
            alias.symlink_to(target)
        rows = [{"path": str(alias), "resolvedPath": str(target), "sha256": gate.file_digest(target)} for alias in aliases]
        expected = [[row["path"], row["resolvedPath"], row["sha256"]] for row in rows]
        self.assertEqual(launcher.library_bindings(rows, self.repo), expected)
        policy = copy.deepcopy(self.args.trust["policy"])
        policy["runtime"]["libraries"] = rows
        path = self.root / "library-policy.json"
        raw = gate.encoded(policy)
        path.write_bytes(raw)
        context = launcher.load_policy(path, gate.digest(raw), self.repo)
        self.assertEqual(context["policy"]["runtime"]["libraries"], rows)
        for bad in ([rows[0], rows[0]], [rows[0], {**rows[1], "sha256": "0" * 64}], rows * 9):
            with self.subTest(rows=bad):
                with self.assertRaises(launcher.Untrusted):
                    launcher.library_bindings(bad, self.repo)
        aliases[1].unlink()
        aliases[1].symlink_to(other)
        with self.assertRaises(launcher.Untrusted):
            launcher.library_bindings(rows, self.repo)
        aliases[1].unlink()
        aliases[1].symlink_to(target)
        target.write_bytes(b"changed runtime library fixture bytes")
        with self.assertRaises(launcher.Untrusted):
            launcher.load_policy(path, gate.digest(raw), self.repo)
        target.write_bytes(b"runtime library fixture bytes")
        self.assertEqual(launcher.library_bindings(rows, self.repo), expected)
        many = []
        for index in range(17):
            alias = self.root / ("count-loader-" + str(index))
            alias.symlink_to(target)
            many.append({**rows[0], "path": str(alias)})
        with self.assertRaises(launcher.Untrusted):
            launcher.library_bindings(many, self.repo)
        shared = self.root / "shared-budget-library"
        with shared.open("wb") as file:
            file.truncate(90 * 1024 * 1024)
        shared_rows = []
        for index in range(3):
            alias = self.root / ("budget-loader-" + str(index))
            alias.symlink_to(shared)
            shared_rows.append({"path": str(alias), "resolvedPath": str(shared), "sha256": gate.file_digest(shared)})
        self.assertEqual(len(launcher.library_bindings(shared_rows, self.repo)), 3)

    def test_a_small_command_cannot_claim_the_trusted_root_gate(self):
        self.args.command = [sys.executable, "-c", f"import sys; sys.stdout.buffer.write({PASS!r})"]
        ledger = FakeLedger()
        with self.assertRaises(gate.UnknownInput):
            gate.produce(self.args, ledger)
        self.assertEqual(ledger.calls, [])

    def test_a_changed_manifest_is_not_accepted_as_a_new_independent_pin(self):
        Path(self.args.trust["policyPath"]).write_bytes(b"{}")
        ledger = FakeLedger()
        with self.assertRaises(gate.UnknownInput):
            gate.produce(self.args, ledger)
        self.assertEqual(ledger.calls, [])

    def test_undeclared_ignored_inputs_make_the_closure_unknown(self):
        (self.repo / ".gitignore").write_text("hidden-dependency\n")
        (self.repo / "hidden-dependency").write_text("not in the trusted declaration\n")
        self.git("add", ".gitignore")
        self.git("commit", "-qm", "ignored dependency")
        self.assertIsNone(self.fingerprint()["deps"])
        with self.assertRaises(gate.UnknownInput):
            gate.produce(self.args, FakeLedger())

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_an_ignored_link_to_a_declared_dependency_binds_the_reachable_input(self):
        (self.repo / ".gitignore").write_text("node_modules\n")
        (self.repo / "node_modules").symlink_to(self.deps, target_is_directory=True)
        self.git("add", ".gitignore")
        self.git("commit", "-qm", "declared dependency alias")
        before = self.fingerprint()
        self.assertIsNotNone(before["deps"])
        self.assertIsNotNone(before["source"])
        (self.deps / "dependency").write_bytes(b"changed reachable dependency")
        self.assertIsNone(self.fingerprint()["deps"])

    def test_configuration_changes_and_missing_roles_remain_unknown(self):
        (self.root / "child-home/fixture-config").write_text("changed configuration\n")
        self.assertIsNone(self.fingerprint()["env"])
        self.assertIsNone(self.fingerprint()["toolchain"])

    def test_even_a_trusted_manifest_cannot_give_a_child_ledger_credentials(self):
        self.args.trust["policy"]["gates"]["gate-root"]["environment"]["ZEROCODE_AGENT_TEAM_TOKEN"] = "fixture-capability"
        with self.assertRaises(gate.UnknownInput):
            self.args.trust["configure"](self.args)

    def test_scanner_import_uses_the_verified_rules_bytes_without_reopening_them(self):
        with mock.patch.object(Path, "read_text", side_effect=AssertionError("rules reopened after verification")):
            self.assertEqual(gate.mask_log(PASS, {}, self.args.trust), PASS)

    def test_recipe_coverage_is_checked_against_the_independent_declaration(self):
        complete = b"==> verify recipe first\n" + PASS + b"<== verify recipe first rc=0 1s\n"
        self.assertEqual(gate.test_count(complete, ["first"]), 2)
        self.assertEqual(gate.test_count(complete, ["first", "missing"]), 0)
        self.assertEqual(gate.test_count(complete, ["other"]), 0)

    def test_real_node_tap_spec_and_unittest_runners_produce_complete_counts(self):
        import shutil
        node = shutil.which("node")
        self.assertIsNotNone(node, "actual node runner is required for this acceptance fixture")
        node_test = self.root / "fixture-node.cjs"
        node_test.write_text("const test=require('node:test'); const assert=require('node:assert/strict'); test('fixture',()=>assert.equal(1+1,2));\n")
        py_test = self.root / "fixture_unittest.py"
        py_test.write_text("import unittest\nclass Fixture(unittest.TestCase):\n    def test_fixture(self):\n        self.assertEqual(1+1,2)\n")
        commands = [[node, "--test", "--test-reporter=" + reporter, str(node_test)] for reporter in ("tap", "spec")]
        commands.append([sys.executable, "-I", "-S", "-B", "-m", "unittest", "discover", "-s", str(self.root), "-p", py_test.name])
        for command in commands:
            with self.subTest(runner=command[:4]):
                output = subprocess.run(command, env=gate.execution_environment(self.args),
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30, check=False)
                self.assertEqual(output.returncode, 0, output.stdout)
                self.assertEqual(gate.test_count(output.stdout), 1, output.stdout)

    def test_real_runner_failures_aborts_and_zero_tests_cannot_hide_behind_a_passing_runner(self):
        import shutil
        node = shutil.which("node")
        self.assertIsNotNone(node, "actual node runner is required")
        node_test = self.root / "runner-control.cjs"
        node_cases = {
            "pass": "const test=require('node:test'); test('control',()=>{});\n",
            "failure": "const test=require('node:test'); const assert=require('node:assert/strict'); for(let i=0;i<7;i++) test('control-'+i,()=>assert.ok(i<2));\n",
            "cancelled": "const test=require('node:test'); test('control',async()=>await new Promise(()=>{}));\n",
            "abort": "process.exit(3);\n",
            "zero": "",
        }
        for reporter in ("tap", "spec"):
            for label, source in node_cases.items():
                with self.subTest(runner="node", reporter=reporter, case=label):
                    node_test.write_text(source)
                    self.args.command = [node, "--test", "--test-reporter=" + reporter]
                    if label != "zero":
                        self.args.command.append(str(node_test))
                    self.args.script_file = [str(node_test)]
                    self.args.output = str(self.root / ("node-" + reporter + "-" + label))
                    ledger = FakeLedger()
                    result = self.produce(ledger)
                    log = (Path(self.args.output) / "gate.log").read_bytes()
                    self.assertTrue(ledger.metadata["executed"])
                    if label == "pass":
                        self.assertEqual(ledger.metadata["rc"], 0, log)
                        self.assertEqual(result["outcome"], "passed", log)
                        self.assertEqual(result["tests"], 1, log)
                    else:
                        self.assertEqual(result["outcome"], "unknown" if label == "zero" else "failed", log)
                        self.assertEqual(result["tests"], 0, log)
                        self.assertEqual(gate.test_count(PASS + log), 0, log)
                        if label == "zero":
                            self.assertEqual(ledger.metadata["rc"], 0, log)
                            self.assertRegex(log.decode(), r"(?:#|ℹ) tests 0\b")
                        else:
                            self.assertNotEqual(ledger.metadata["rc"], 0, log)
                        if label == "failure":
                            for name, count in (("tests", 7), ("pass", 2), ("fail", 5)):
                                self.assertRegex(log.decode(), rf"(?:#|ℹ) {name} {count}\b")
                        if label == "cancelled":
                            self.assertRegex(log.decode(), r"(?:#|ℹ) cancelled 1\b")
        py_test = self.root / "runner_control.py"
        empty = self.root / "empty-unittest"
        empty.mkdir()
        for label, assertion in (("pass", "True"), ("failure", "False"), ("zero", None)):
            with self.subTest(runner="unittest", case=label):
                py_test.write_text("import unittest\nclass Control(unittest.TestCase):\n    def test_control(self):\n        self.assertTrue(" + str(assertion) + ")\n")
                self.args.command = [sys.executable, "-I", "-S", "-B", "-m", "unittest", "discover",
                                     "-s", str(empty if label == "zero" else self.root), "-p", py_test.name]
                self.args.script_file = [str(py_test)]
                self.args.output = str(self.root / ("unittest-" + label))
                ledger = FakeLedger()
                result = self.produce(ledger)
                log = (Path(self.args.output) / "gate.log").read_bytes()
                self.assertEqual(ledger.metadata["rc"], 1 if label == "failure" else 0, log)
                self.assertEqual(result["outcome"], {"pass": "passed", "failure": "failed", "zero": "unknown"}[label], log)
                self.assertEqual(result["tests"], 1 if label == "pass" else 0, log)
                if label != "pass":
                    self.assertEqual(gate.test_count(PASS + log), 0, log)

    def test_real_source_ancestry_accepts_a_merge_and_rejects_an_unrelated_candidate(self):
        base = self.git("rev-parse", "HEAD").decode().strip()
        self.git("checkout", "-qb", "fixture-worker")
        (self.repo / "worker.rs").write_text("pub const FIXTURE: bool = true;\n")
        self.git("add", ".")
        self.git("commit", "-qm", "worker source")
        handed_in = self.git("rev-parse", "HEAD").decode().strip()
        self.git("checkout", "-qb", "fixture-main", base)
        (self.repo / "main.rs").write_text("fn main() {}\n")
        self.git("add", ".")
        self.git("commit", "-qm", "main source")
        self.git("merge", "--no-ff", "-qm", "merge candidate", "fixture-worker")
        self.args.handed_in_head = handed_in
        merged = self.fingerprint()
        self.assertTrue(merged["sourceAncestor"])
        self.assertNotEqual(merged["head"], handed_in)
        self.assertEqual(merged["handedInHead"], handed_in)
        self.git("checkout", "--orphan", "unrelated")
        self.git("commit", "-qm", "unrelated root")
        self.assertFalse(self.fingerprint()["sourceAncestor"])

    def test_isolated_launcher_ignores_candidate_sitecustomize_and_pythonpath(self):
        marker = self.root / "candidate-imported"
        (self.repo / "sitecustomize.py").write_text(f"from pathlib import Path\nPath({str(marker)!r}).write_text('imported')\n")
        environment = gate.execution_environment(self.args)
        environment["PYTHONPATH"] = str(self.repo)
        control = subprocess.run([sys.executable, "-c", "print('fixture import control')"],
            cwd=self.repo, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=False)
        self.assertEqual(control.returncode, 0, control.stderr)
        self.assertTrue(marker.is_file(), "import pollution positive control never ran")
        marker.unlink()
        trust = self.args.trust
        command = [sys.executable, "-I", "-S", "-B", str(SCRIPT.with_name("gate_launcher.py")),
            "--policy", trust["policyPath"], "--policy-sha256", trust["policySha256"], "--repo", str(self.repo), "--",
            "verify", "--repo", str(self.repo), "--gate", "gate-root", "--task", "t-1", "--attempt", "none",
            "--scope", "deterministic", "--closure", "declared", "--profile", self.args.profile,
            "--metadata", self.args.metadata, "--log", self.args.log, "--", *self.args.command]
        output = subprocess.run(command, cwd=self.repo, env=environment,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=False)
        self.assertEqual(output.returncode, 1, output.stderr)
        self.assertEqual(json.loads(output.stdout)["newExecutions"], 0)
        self.assertFalse(marker.exists(), "candidate import polluted the trusted launcher")

    def test_a_launcher_without_no_site_refuses_before_the_producer_runs(self):
        trust = self.args.trust
        output = subprocess.run([sys.executable, "-I", "-B", str(SCRIPT.with_name("gate_launcher.py")),
            "--policy", trust["policyPath"], "--policy-sha256", trust["policySha256"], "--repo", str(self.repo),
            "--", "run"], cwd=self.repo, env=gate.execution_environment(self.args),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=False)
        self.assertEqual(output.returncode, 1, output.stderr)
        answer = json.loads(output.stdout)
        self.assertEqual(answer["decision"], "miss")
        self.assertFalse(answer["executed"])
        self.assertEqual(answer["newExecutions"], 0)
        self.assertFalse(Path(self.args.output).exists())

    def test_credential_files_are_refused_before_their_contents_are_hashed(self):
        credential = self.deps / "credentials.toml"
        credential.write_text("fixture credential content\n")
        with mock.patch.object(gate, "file_digest", side_effect=AssertionError("credential contents hashed")):
            with self.assertRaises(gate.UnknownInput):
                gate.paths_digest([credential])

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_reachable_directory_link_cycles_are_unknown(self):
        folder = self.repo / "folder"
        folder.mkdir()
        (folder / "input").write_text("tracked input\n")
        (folder / "cycle").symlink_to(".", target_is_directory=True)
        self.git("add", ".")
        self.git("commit", "-qm", "directory cycle")
        self.assertIsNone(self.fingerprint()["source"])
        with self.assertRaises(gate.UnknownInput):
            gate.paths_digest([folder])

    def test_configuration_paths_cannot_be_exempted_as_output_roots(self):
        policy = copy.deepcopy(self.args.trust["policy"])
        other = self.root / "other-configuration"
        other.mkdir()
        (other / "public-input").write_text("unrelated configuration\n")
        selected = policy["gates"]["gate-root"]
        selected["configurationRoots"] = [{"path": str(other), "sha256": gate.paths_digest([other])}]
        selected["outputRoots"] = [str(self.root / "child-home")]
        self.repin_policy(policy)
        accepted = False
        try:
            gate.provenance(self.args)
            accepted = True
        except gate.UnknownInput:
            pass
        self.assertFalse(accepted, "configuration path escaped its input identity")

    def test_declared_cache_outputs_coexist_with_configuration_inputs(self):
        policy = copy.deepcopy(self.args.trust["policy"])
        cache = self.root / "child-home/cache"
        policy["gates"]["gate-root"]["outputRoots"] = [str(cache)]
        self.repin_policy(policy)
        accepted = False
        try:
            gate.provenance(self.args)
            accepted = True
        except gate.UnknownInput:
            pass
        self.assertTrue(accepted, "valid nested cache declaration was refused")
        before = self.fingerprint()
        self.assertIsNotNone(before["policy"])
        cache.mkdir()
        (cache / "output").write_text("allowed mutable output\n")
        after = self.fingerprint()
        self.assertEqual(after["deps"], before["deps"], "cache output changed an input identity")
        self.assertEqual(after["env"], before["env"])

    def test_every_path_tool_is_bound_before_it_can_run(self):
        tool = self.tools / "fixture-helper"
        marker = self.root / "path-tool-ran"
        tool.write_text(f"#!/bin/sh\nprintf ran > {str(marker)!r}\nprintf '%s\\n' {PASS.decode().strip()!r}\n")
        tool.chmod(0o755)
        self.args.fixture_tools = {"fixture-helper": tool}
        self.args.command = ["fixture-helper"]
        authorize(self.args, self.root, gate)
        before = self.fingerprint()
        self.assertIsNotNone(before["toolchain"])
        result = gate.produce(self.args, FakeLedger())
        self.assertEqual(result["outcome"], "passed")
        self.assertEqual(marker.read_text(), "ran")
        saved = tool.read_bytes()
        tool.write_text("#!/bin/sh\nexit 0\n")
        self.assertIsNone(self.fingerprint()["toolchain"])
        with mock.patch.object(gate, "execute", side_effect=AssertionError("changed PATH tool executed")):
            with self.assertRaises(gate.UnknownInput):
                gate.produce(self.args, FakeLedger())
        tool.write_bytes(saved)
        self.assertEqual(self.fingerprint(), before)

    def test_a_path_alias_change_is_observed_even_when_both_targets_are_declared(self):
        before = self.fingerprint()
        self.assertIsNotNone(before["toolchain"])
        alias = self.root / "path-bin/curl"
        self.assertTrue(alias.is_symlink(), "fixture curl control is unavailable")
        saved = os.readlink(alias)
        alias.unlink()
        alias.symlink_to(Path(sys.executable).resolve())
        changed = self.fingerprint()
        self.assertIsNotNone(changed["policy"])
        self.assertNotEqual(changed["toolchain"], before["toolchain"])
        alias.unlink()
        alias.symlink_to(saved)
        self.assertEqual(self.fingerprint(), before)

    def test_declared_configuration_absence_detects_a_new_ancestor_config(self):
        before = self.fingerprint()
        self.assertIsNotNone(before["deps"])
        folder = self.root / ".cargo"
        folder.mkdir()
        path = folder / "config.toml"
        path.write_text('[env]\nFIXTURE_INPUT = "changed"\n')
        self.assertIsNone(self.fingerprint()["deps"])
        with mock.patch.object(gate, "execute", side_effect=AssertionError("new ancestor configuration executed")):
            with self.assertRaises(gate.UnknownInput):
                gate.produce(self.args, FakeLedger())
        path.unlink()
        self.assertEqual(self.fingerprint(), before)

    def test_an_omitted_configuration_search_scope_is_refused(self):
        before = self.fingerprint()
        self.assertIsNotNone(before["policy"])
        policy = copy.deepcopy(self.args.trust["policy"])
        policy["gates"]["gate-root"]["configurationSearches"] = policy["gates"]["gate-root"]["configurationSearches"][1:]
        self.repin_policy(policy)
        with self.assertRaises(gate.UnknownInput):
            gate.provenance(self.args)

    def test_first_write_ignored_bytecode_is_output_and_preserves_actual_inputs(self):
        folder = self.repo / "tools/release"
        folder.mkdir(parents=True)
        (folder / "fixture_module.py").write_text("VALUE = 1\n")
        (self.repo / ".gitignore").write_text("tools/release/__pycache__/\n")
        self.git("add", ".")
        self.git("commit", "-qm", "first write fixture")
        cache = folder / "__pycache__"
        self.args.command = [sys.executable, "-c",
            "import importlib.util; from pathlib import Path; import sys; "
            "spec=importlib.util.spec_from_file_location('fixture_module', 'tools/release/fixture_module.py'); "
            "spec.loader.exec_module(importlib.util.module_from_spec(spec)); "
            f"sys.stdout.buffer.write({PASS!r})"]
        authorize(self.args, self.root, gate, output_roots=[str(cache)])
        self.assertFalse(cache.exists())
        before = self.fingerprint()
        self.assertIsNotNone(before["deps"])
        ledger = FakeLedger()
        result = gate.produce(self.args, ledger)
        self.assertEqual(result["outcome"], "passed")
        self.assertTrue(list(cache.glob("*.pyc")), "first-write control never created bytecode")
        self.assertTrue(ledger.metadata["inputsUnchanged"])
        self.assertEqual(self.fingerprint(), before)
        (folder / "fixture_module.py").write_text("VALUE = 2\n")
        self.assertIsNone(self.fingerprint()["source"])

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows developer mode")
    def test_output_links_cannot_hide_configuration_and_input_links_cannot_read_cache(self):
        cache = self.root / "child-home/cache"
        authorize(self.args, self.root, gate, output_roots=[str(cache)])
        before = self.fingerprint()
        self.assertIsNotNone(before["deps"])
        cache.mkdir()
        linked = cache / "configuration"
        linked.symlink_to(self.root / "child-home/fixture-config")
        self.assertIsNone(self.fingerprint()["deps"])
        linked.unlink()
        (cache / "output").write_text("mutable cache\n")
        self.assertEqual(self.fingerprint(), before)
        (self.root / "child-home/input-link").symlink_to(cache / "output")
        with self.assertRaises(gate.UnknownInput):
            gate.provenance(self.args)

    def test_ignored_libtest_cases_do_not_count_as_execution_or_hide_failures(self):
        output = b"running 3 tests\n" + PASS.replace(b"0 ignored", b"1 ignored")
        self.assertEqual(gate.test_count(output), 2)
        self.assertEqual(gate.test_count(output.replace(b"0 failed", b"1 failed")), 0)
        self.assertEqual(gate.test_count(output.replace(b"running 3", b"running 2")), 0)
        self.assertEqual(gate.test_count(b"running 1 test\n" + PASS.replace(b"2 passed", b"0 passed").replace(b"0 ignored", b"1 ignored")), 0)

    def test_manifest_target_and_timeout_must_match_the_exact_invocation(self):
        baseline = self.fingerprint()
        self.assertIsNotNone(baseline["policy"])
        original = copy.deepcopy(self.args.trust["policy"])
        for change in ("timeout", "head", "repo", "scope"):
            with self.subTest(dimension=change):
                policy = copy.deepcopy(original)
                selected = policy["gates"]["gate-root"]
                if change == "timeout":
                    selected["timeout"] += 1
                else:
                    selected["target"][change] = "f" * 40 if change == "head" else "different"
                self.repin_policy(policy)
                with self.assertRaises(gate.UnknownInput):
                    gate.provenance(self.args)
                self.repin_policy(original)
                self.assertEqual(self.fingerprint(), baseline)

    @unittest.skipIf(os.name == "nt", "private inherited descriptors require POSIX")
    def test_bound_policy_and_strict_arguments_precede_any_producer_code(self):
        import socket
        good_marker = self.root / "approved-loader-ran"
        bad_marker = self.root / "unapproved-loader-ran"
        good_producer = self.root / "approved-producer.py"
        bad_producer = self.root / "unapproved-producer.py"
        good_producer.write_text("from pathlib import Path\ndef main(trust=None, argv=None):\n    Path(" + repr(str(good_marker)) + ").write_text('ran')\n    return 0\n")
        bad_producer.write_text("from pathlib import Path\nPath(" + repr(str(bad_marker)) + ").write_text('ran')\ndef main(trust=None, argv=None):\n    return 0\n")
        launcher = SCRIPT.with_name("gate_launcher.py")
        fixture = load_module(SCRIPT.with_name("tests") / "gate_bootstrap_fixture.py", "loader_runtime_fixture")
        bootstrap = fixture.prepare(self.root / "loader-runtime", launcher)
        runtime = {role: {"path": bootstrap[bootstrap.index("--" + role) + 1],
                          "sha256": bootstrap[bootstrap.index("--" + role + "-sha256") + 1]}
                   for role in ("python", "stdlib")}
        runtime["libraries"] = json.loads((Path(runtime["stdlib"]["path"]) / fixture.LIBRARY_DECLARATION).read_bytes())
        policies = {}
        for label, producer in (("good", good_producer), ("bad", bad_producer)):
            policy = copy.deepcopy(self.args.trust["policy"])
            policy["runtime"] = runtime
            policy["bundle"]["producer"] = {"path": str(producer), "sha256": gate.file_digest(producer)}
            path = self.root / (label + "-loader-policy.json")
            path.write_bytes(gate.encoded(policy))
            policies[label] = (str(path), gate.digest(path.read_bytes()))

        def invoke(label, bound, malformed=None):
            parent, child = socket.socketpair()
            with parent, child:
                policy_path, policy_sha = policies[label]
                prefix = ["--bootstrap-launcher", str(launcher), "--authority-fd", str(child.fileno()),
                          "--bound-policy-sha256", bound, "--policy", policy_path, "--policy-sha256", policy_sha, "--repo", str(self.repo)]
                if malformed == "equals":
                    prefix[8:10] = ["--policy-sha256=" + policy_sha]
                elif malformed == "abbreviation":
                    prefix[8] = "--policy-s"
                elif malformed == "duplicate":
                    prefix += ["--policy", policies["bad"][0]]
                elif malformed == "reserved":
                    prefix += ["--bound-policy-sha256", policies["bad"][1]]
                return subprocess.run([runtime["python"]["path"], "-I", "-S", "-B", "-", *prefix, "--", "run"],
                                      input=launcher.read_bytes(), pass_fds=(child.fileno(),), cwd=self.repo,
                                      env={"HOME": str(self.root / "git-home"), "PATH": os.defpath},
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=30)

        good = invoke("good", policies["good"][1])
        self.assertEqual(good.returncode, 0, good.stderr)
        self.assertEqual(good_marker.read_text(), "ran")
        good_marker.unlink()
        copied = Path(runtime["python"]["path"])
        copied_bytes = copied.read_bytes()
        source_launcher = Path(sys.executable).resolve(strict=True)
        self.assertNotEqual(source_launcher, fixture.executable_image(), "separate launcher control is unavailable")
        good_policy = Path(policies["good"][0])
        good_policy_bytes = good_policy.read_bytes()
        good_pin = policies["good"]
        try:
            copied.write_bytes(source_launcher.read_bytes())
            redirected = json.loads(good_policy_bytes)
            redirected["runtime"]["python"]["sha256"] = gate.file_digest(copied)
            redirected["runtime"]["stdlib"]["sha256"] = fixture.tree_hash(runtime["stdlib"]["path"])
            good_policy.write_bytes(gate.encoded(redirected))
            policies["good"] = (str(good_policy), gate.digest(good_policy.read_bytes()))
            denied_image = invoke("good", policies["good"][1])
            self.assertEqual(denied_image.returncode, 1, denied_image.stderr)
            self.assertEqual(json.loads(denied_image.stdout)["code"], "digest_mismatch")
            self.assertFalse(good_marker.exists(), "redirected process image executed the producer")
            print("engine-image-control: rc=1 code=digest_mismatch producer-ran=false", file=sys.stderr)
        finally:
            copied.write_bytes(copied_bytes)
            good_policy.write_bytes(good_policy_bytes)
            policies["good"] = good_pin
        restored = invoke("good", policies["good"][1])
        self.assertEqual(restored.returncode, 0, restored.stderr)
        self.assertEqual(good_marker.read_text(), "ran")
        good_marker.unlink()
        denied = invoke("bad", policies["good"][1])
        self.assertFalse(bad_marker.exists(), "producer code ran before policy binding")
        self.assertEqual(denied.returncode, 1, denied.stderr)
        self.assertEqual(json.loads(denied.stdout)["code"], "digest_mismatch")
        for variant in ("equals", "abbreviation", "duplicate", "reserved"):
            denied = invoke("good", policies["good"][1], variant)
            self.assertEqual(denied.returncode, 1, denied.stderr)
            self.assertEqual(json.loads(denied.stdout)["code"], "invalid_request")
            self.assertFalse(bad_marker.exists())
            self.assertFalse(good_marker.exists())

    def test_actual_ui_runner_reports_require_matching_nonzero_complete_checks(self):
        import shutil
        node = shutil.which("node")
        self.assertIsNotNone(node)
        runner = self.root / "window-runner.mjs"
        runner.write_bytes((SCRIPT.parents[2] / "ui/tests/window-runner.mjs").read_bytes())
        script = self.root / "ui-report-fixture.mjs"
        script.write_text(r"""import { createRunner } from './window-runner.mjs';
const mode = process.argv[2];
const runner = createRunner({ env: {}, argv: [], log: (line) => {
  if (/^\d+\/\d+ passed$/.test(line)) {
    if (mode === 'abort') process.exit(3);
    if (mode === 'truncated') return;
    if (mode === 'mismatch') line = '3/3 passed';
  }
  console.log(line);
}});
if (mode !== 'zero') {
  if (mode === 'loose') {
    runner.ok('first actual check', 1 + 1 === 2);
    runner.ok('second actual check', true);
  } else {
    runner.suite('fixture', ({ ok }) => {
      ok('first actual check', 1 + 1 === 2);
      ok('second actual check', mode !== 'failure');
    });
  }
}
await runner.run();
process.exitCode = runner.report() ? 1 : 0;
""")
        for mode in ("passed", "loose", "failure", "zero", "abort", "truncated", "mismatch"):
            with self.subTest(mode=mode):
                self.args.command = [node, str(script), mode]
                self.args.script_file = [str(script), str(runner)]
                self.args.output = str(self.root / ("ui-" + mode))
                ledger = FakeLedger()
                result = self.produce(ledger)
                log = (Path(self.args.output) / "gate.log").read_bytes()
                expected_rc = 3 if mode == "abort" else 1 if mode == "failure" else 0
                self.assertEqual(ledger.metadata["rc"], expected_rc, log)
                self.assertTrue(ledger.metadata["executed"])
                if mode in ("passed", "loose"):
                    self.assertEqual(result["outcome"], "passed", log)
                    self.assertEqual(result["tests"], 2, log)
                    self.assertIn(b"2/2 passed", log)
                else:
                    self.assertEqual(result["outcome"], "failed" if expected_rc else "unknown", log)
                    self.assertEqual(result["tests"], 0, log)
                    self.assertEqual(gate.test_count(PASS + log), 0, log)
                    if mode == "zero":
                        self.assertIn(b"0/0 passed", log)
                        mixed = b"==> verify recipe rust\n" + PASS + b"<== verify recipe rust rc=0 0s\n==> verify recipe ui\n" + log + b"<== verify recipe ui rc=0 0s\n"
                        self.assertEqual(gate.test_count(mixed, ["rust", "ui"], ["rust", "ui"]), 0, mixed)
                        self.assertEqual(gate.test_count(mixed, ["rust", "ui"], ["rust"]), 0, mixed)

    def test_each_required_recipe_runs_tests_and_a_zero_command_cannot_hide(self):
        suite = self.root / "actual-suite"
        suite.mkdir()
        test_file = suite / "fixture_case.py"
        test_file.write_text("import unittest\nclass Fixture(unittest.TestCase):\n    def test_actual(self):\n        self.assertEqual(1+1,2)\n")
        script = self.root / "recipe-controls.py"
        command = [sys.executable, "-I", "-S", "-B", str(script)]
        for second_has_tests in (False, True):
            with self.subTest(second_has_tests=second_has_tests):
                tests = [sys.executable, "-I", "-S", "-B", "-m", "unittest", "discover", "-s", str(suite), "-p", test_file.name]
                second = tests if second_has_tests else [sys.executable, "-I", "-S", "-B", "-c", "print('actual zero-command ran')"]
                script.write_text("import subprocess\n" + "\n".join(
                    "print('==> verify recipe %s', flush=True)\nresult=subprocess.run(%r, check=False)\nprint('<== verify recipe %s rc=%%d 0s' %% result.returncode, flush=True)\nassert result.returncode == 0" % (name, child, name)
                    for name, child in (("one", tests), ("two", second))) + "\n")
                self.args.command = command.copy()
                self.args.script_file = [str(script), str(test_file)]
                authorize(self.args, self.root, gate)
                policy = copy.deepcopy(self.args.trust["policy"])
                self.args.command = command.copy()
                policy["gates"][self.args.gate].update(command=command, recipes=["one", "two"], testRecipes=["one", "two"])
                self.repin_policy(policy)
                self.args.output = str(self.root / ("recipe-positive" if second_has_tests else "recipe-zero"))
                ledger = FakeLedger()
                result = gate.produce(self.args, ledger)
                log = (Path(self.args.output) / "gate.log").read_bytes()
                self.assertEqual(ledger.metadata["rc"], 0, log)
                self.assertTrue(ledger.metadata["executed"])
                self.assertEqual(result["outcome"], "passed" if second_has_tests else "unknown", log)
                self.assertEqual(result["tests"], 2 if second_has_tests else 0, log)
                if not second_has_tests:
                    self.assertIn(b"actual zero-command ran", log)
                    self.assertEqual(gate.test_count(log, ["one", "two"], ["one"]), 1)

    @unittest.skipIf(os.name == "nt", "module link creation needs Windows developer mode")
    def test_declared_node_module_link_runs_and_module_changes_or_search_gaps_refuse(self):
        import shutil
        node = shutil.which("node")
        self.assertIsNotNone(node)
        package = self.deps / "fixture-module"
        package.mkdir()
        module = package / "index.js"
        module.write_text("module.exports = 1;\n")
        (self.repo / "node_modules").symlink_to(self.deps, target_is_directory=True)
        (self.repo / ".gitignore").write_text("node_modules\n")
        script = self.repo / "fixture_module.cjs"
        script.write_text("const test=require('node:test'); const assert=require('node:assert/strict'); test('declared module',()=>assert.equal(require('fixture-module'),1));\n")
        self.git("add", ".")
        self.git("commit", "-qm", "declared module fixture")
        self.args.command = [node, "--test", "--test-reporter=tap", str(script)]
        self.args.script_file = [str(script)]
        result = self.produce(FakeLedger())
        self.assertEqual(result["outcome"], "passed")
        self.assertEqual(result["tests"], 1)
        before = self.fingerprint()
        module.write_text("module.exports = 2;\n")
        self.assertIsNone(self.fingerprint()["policy"])
        self.assertNotEqual(self.fingerprint()["deps"], before["deps"])
        module.write_text("module.exports = 1;\n")
        self.assertEqual(self.fingerprint(), before)
        new_parent_modules = self.root / "node_modules"
        new_parent_modules.mkdir()
        self.assertIsNone(self.fingerprint()["policy"])
        new_parent_modules.rmdir()
        self.assertEqual(self.fingerprint(), before)
        policy = copy.deepcopy(self.args.trust["policy"])
        policy["gates"][self.args.gate]["configurationSearches"] = [row for row in policy["gates"][self.args.gate]["configurationSearches"] if "node_modules" not in row["names"]]
        with self.assertRaises(gate.UnknownInput):
            self.repin_policy(policy)
            gate.provenance(self.args)

    def test_git_configuration_absence_changes_and_unsupported_program_settings_refuse(self):
        script = self.root / "fixture_git_case.py"
        script.write_text("import os, subprocess, unittest\nclass Fixture(unittest.TestCase):\n    def test_git(self):\n        self.assertEqual(os.environ['GIT_CONFIG_NOSYSTEM'],'1')\n        self.assertEqual(os.environ['GIT_CONFIG_GLOBAL'],os.devnull)\n        self.assertEqual(os.environ['GIT_CONFIG_SYSTEM'],os.devnull)\n        self.assertNotIn('GIT_CONFIG_COUNT',os.environ)\n        self.assertEqual(subprocess.check_output(['git','--no-pager','config','--no-includes','--local','--get','core.repositoryformatversion']).strip(),b'0')\n")
        self.args.command = [sys.executable, "-I", "-S", "-B", "-m", "unittest", "discover", "-s", str(self.root), "-p", script.name]
        self.args.script_file = [str(script)]
        with mock.patch.dict(os.environ, {"GIT_CONFIG": str(self.root / "undeclared-config"), "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.fsmonitor", "GIT_CONFIG_VALUE_0": "fixture-monitor"}):
            result = self.produce(FakeLedger())
        self.assertEqual(result["outcome"], "passed")
        self.assertEqual(result["tests"], 1)
        before = self.fingerprint()
        attributes = self.repo / ".git/info/attributes"
        attributes.write_text("*.rs text\n")
        self.assertIsNone(self.fingerprint()["policy"])
        attributes.unlink()
        self.assertEqual(self.fingerprint(), before)
        config = self.repo / ".git/config"
        original = config.read_bytes()
        variants = (original + b"\n[include]\npath = fixture-include\n",
                    original.replace(b"[core]", b"[core]\nfsmonitor = fixture-monitor", 1),
                    original + b"\n[filter \"fixture\"]\nclean = fixture-filter\n",
                    original.replace(b"[core]", b"[core]\nhooksPath = fixture-hooks", 1),
                    b"[core]\n\tbare = false\n\t\t[include]\n\t\tpath = fixture-include\n",
                    b"[core]\n\tbare = false\n\t\thooksPath = fixture-hooks\n",
                    b"[core]\n\tbare = false\n\t\t[filter \"fixture\"]\n\t\tclean = fixture-filter\n")
        inspection = self.root / "git-parser-control"
        safe_environment = {"HOME": str(self.root / "git-home"), "PATH": os.defpath, "GIT_CONFIG_NOSYSTEM": "1",
                            "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull, "GIT_CONFIG_COUNT": "0"}
        for raw, key, expected in ((original, "core.repositoryformatversion", "0"),
                                   (variants[-3], "include.path", "fixture-include"),
                                   (variants[-2], "core.hookspath", "fixture-hooks"),
                                   (variants[-1], "filter.fixture.clean", "fixture-filter")):
            inspection.write_bytes(raw)
            actual = subprocess.run(["git", "config", "--file", str(inspection), "--no-includes", "--get", key],
                                    cwd=self.root, env=safe_environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=30)
            self.assertEqual(actual.returncode, 0, actual.stderr)
            self.assertEqual(actual.stdout.decode().strip(), expected)
        for changed in variants:
            self.assertNotEqual(changed, original)
            config.write_bytes(changed)
            with mock.patch.object(gate.subprocess, "run", side_effect=AssertionError("Git inspected unsupported configuration")) as executed:
                with self.assertRaises(gate.UnknownInput):
                    gate.git(self.repo, "rev-parse", "HEAD")
                self.assertEqual(executed.call_count, 0)
            config.write_bytes(original)
        self.assertEqual(self.fingerprint(), before)

    def test_phase_budgets_are_pinned_strict_and_leave_hashing_time(self):
        original = copy.deepcopy(self.args.trust["policy"])
        for key in ("overheadTimeoutSeconds", "verificationTimeoutSeconds", "maxJobTtlSeconds"):
            for value in (True, 0, 1.0, -1, 7201):
                with self.subTest(key=key, value=value):
                    bad = copy.deepcopy(original)
                    bad["gates"][self.args.gate][key] = value
                    with self.assertRaises(gate.UnknownInput):
                        self.repin_policy(bad)
                        gate.provenance(self.args)
        for overhead, verification, maximum in ((20, 1, 20), (1, 12, 11)):
            bad = copy.deepcopy(original)
            bad["gates"][self.args.gate].update(overheadTimeoutSeconds=overhead, verificationTimeoutSeconds=verification, maxJobTtlSeconds=maximum)
            with self.assertRaises(gate.UnknownInput):
                self.repin_policy(bad)
                gate.provenance(self.args)
        self.repin_policy(original)
        self.assertIsNotNone(gate.provenance(self.args))

    def test_real_offline_cargo_with_ignored_tests_and_first_writes_preserves_inputs(self):
        from gate_cargo_fixture import prepare
        environment, outputs = prepare(self.args, self.root, gate)
        self.git("add", ".")
        self.git("commit", "-qm", "offline cargo fixture")
        authorize(self.args, self.root, gate, environment_overrides=environment, output_roots=outputs)
        before = self.fingerprint()
        self.assertIsNotNone(before["deps"])
        self.assertIsNotNone(before["toolchain"])
        self.assertFalse((self.repo / "cargo-fixture/target").exists())
        ledger = FakeLedger()
        result = gate.produce(self.args, ledger)
        self.assertEqual(result["outcome"], "passed", Path(self.args.log).read_text())
        self.assertEqual(result["tests"], 1)
        self.assertTrue(ledger.metadata["inputsUnchanged"])
        self.assertTrue((self.repo / "cargo-fixture/target").is_dir())
        self.assertTrue(list((self.repo / "tools/release/__pycache__").glob("*.pyc")))
        self.assertEqual(self.fingerprint(), before)
        config = self.root / "child-home/cargo/config.toml"
        saved = config.read_bytes()
        config.write_text('[env]\nFIXTURE_INPUT = "changed"\n')
        self.assertIsNone(self.fingerprint()["deps"])
        config.write_bytes(saved)
        self.assertEqual(self.fingerprint(), before)
        source = self.repo / "cargo-fixture/src/lib.rs"
        source.write_text(source.read_text().replace('"declared"', '"different"'))
        self.assertIsNone(self.fingerprint()["source"])


if __name__ == "__main__":
    unittest.main()
