#!/usr/bin/env python3
"""The probe's own tests: the row is read from the Rust source, and the
counter tells a CLI that answers a slash command itself from one that asks
a model — with a fake CLI, so no vendor program and no network is needed."""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import probe  # noqa: E402

FAKE_CLI = """#!/usr/bin/env python3
import json, os, sys, urllib.request
said = sys.stdin.read()
if said.startswith("/"):
    print(json.dumps({"num_turns": 0, "usage": {"input_tokens": 0, "output_tokens": 0}}))
    sys.exit(0)
try:
    urllib.request.urlopen(urllib.request.Request(
        os.environ["ANTHROPIC_BASE_URL"] + "/v1/messages?beta=true", data=b"{}", method="POST"))
except Exception:
    pass
print(json.dumps({"num_turns": 1, "usage": {"input_tokens": 9, "output_tokens": 1}}))
"""


# The same CLI, still on its feet after the model call landed: the probe has
# to end it, which is where a kill can be refused.
LINGERING_CLI = FAKE_CLI.replace(
    'print(json.dumps({"num_turns": 1,', 'import time; time.sleep(30)\nprint(json.dumps({"num_turns": 1,')


class ProbeTest(unittest.TestCase):
    def test_the_row_is_read_from_the_rust_source(self) -> None:
        argv, stdin = probe.claude_row(
            probe.HEADLESS_SOURCE.read_text(), probe.RENEWAL_SOURCE.read_text())
        self.assertEqual(argv[0], "-p")
        self.assertIn("--no-session-persistence", argv)
        self.assertNotIn("--disable-slash-commands", argv)
        self.assertTrue(stdin.startswith("/"))

    def test_a_row_that_stops_naming_the_headless_words_is_refused(self) -> None:
        with self.assertRaises(ValueError):
            probe.claude_row(
                probe.HEADLESS_SOURCE.read_text(),
                'pub const CLAUDE_RENEWAL: LoginRenewal = LoginRenewal { argv: &["-p"], stdin: "/cost" };')

    def test_the_counter_sees_the_control_and_not_the_slash_command(self) -> None:
        with tempfile.TemporaryDirectory() as bin_dir:
            cli = Path(bin_dir) / "claude"
            cli.write_text(FAKE_CLI)
            cli.chmod(0o755)
            result = probe.probe(str(cli))
        self.assertEqual(result["row"]["messages"], 0)
        self.assertEqual(result["row"]["tokens"], 0)
        self.assertGreaterEqual(result["control"]["messages"], 1)
        self.assertTrue(result["ok"])

    def test_a_child_that_ended_before_the_kill_does_not_fail_the_run(self) -> None:
        """The control's child answers and is gone while the probe is still
        reaching for it: its group then holds nothing a signal may reach, and
        macOS refuses the kill (EPERM) where Linux finds no group (ESRCH). The
        run has what it came for either way."""
        kill = probe.os.killpg

        def refused(group: int, signal_number: int) -> None:
            kill(group, signal_number)  # nothing is left running behind the test
            raise PermissionError(1, "Operation not permitted")

        with tempfile.TemporaryDirectory() as bin_dir:
            cli = Path(bin_dir) / "claude"
            cli.write_text(LINGERING_CLI)
            cli.chmod(0o755)
            with mock.patch.object(probe.os, "killpg", refused):
                result = probe.probe(str(cli))
        self.assertGreaterEqual(result["control"]["messages"], 1)
        self.assertTrue(result["ok"])

    def test_the_fake_store_is_made_up(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            store = Path(root) / "store"
            probe.fake_store(store, 0)
            said = (store / ".credentials.json").read_text() + (store / ".claude.json").read_text()
        self.assertIn("example.com", said)
        self.assertNotIn(os.environ.get("USER", "\0"), said)


if __name__ == "__main__":
    unittest.main()
