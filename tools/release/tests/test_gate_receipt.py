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
from gate_fixture import authorize, load_module

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

    def test_private_environment_values_are_masked_and_their_identity_stays_unknown(self):
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
        self.args.command = [sys.executable, "-c", "print('no tests executed')"]
        trust = authorize(self.args, self.root, gate)
        argv = [str(SCRIPT), "run", "--repo", self.args.repo, "--gate", "gate-root", "--task", "t-1",
                "--attempt", "none", "--output", self.args.output, "--profile", self.args.profile,
                "--scope", "deterministic", "--closure", "declared", "--", *self.args.command]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(gate, "Ledger", return_value=FakeLedger()):
            self.assertEqual(gate.main(trust=trust), 1, "rc0 with zero tests reported gate success")

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


if __name__ == "__main__":
    unittest.main()
