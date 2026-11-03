#!/usr/bin/env python3
"""lock_answer.py: `just pre-handin`'s lock stage says what prebatch says of the same command.
A crate this machine never fetched made the stage red and asked the worker to commit a lock
that had not changed, on a head prebatch passes (t-84719 r1 P2-1).

Run with: python3 tools/release/tests/test_lock_answer.py
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from posix_shell import fake_program, posix  # noqa: E402

SCRIPT = Path(__file__).resolve().parents[1] / "lock_answer.py"
BEHIND = (
    "error: the lock file /x/Cargo.lock needs to be updated but --locked was passed to prevent this\n"
    "If you want to try to generate the lock file without accessing the network, remove the --locked flag\n"
)
UNFETCHED = "error: failed to download `rand v0.8.5`\n\nCaused by:\n  attempting to make an HTTP request, but --offline was specified\n"


class LockAnswer(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.addCleanup(self.home.cleanup)
        self.root = Path(self.home.name)

    def answer(self, rc, stderr):
        """The script's answer, with a cargo on PATH that exits `rc` having said `stderr`."""
        bin_dir = self.root / "bin"
        bin_dir.mkdir(exist_ok=True)
        (bin_dir / "said").write_text(stderr)
        # fake_program: on Windows a bare sh file is nothing shutil.which or CreateProcess starts, so the cargo found was
        # the runner's own (or none) and every verdict was 3, no answer (t-90089).
        fake_program(bin_dir, "cargo", f'echo "$PWD $*" > "{posix(bin_dir)}/called"\ncat "{posix(bin_dir)}/said" >&2\nexit {rc}\n')
        env = {**os.environ, "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}"}
        return subprocess.run(
            [sys.executable, str(SCRIPT), str(self.root)],
            capture_output=True, text=True, env=env, timeout=30,
        )

    def test_a_lock_that_stands_is_green(self):
        said = self.answer(0, "")
        self.assertEqual(said.returncode, 0, said.stderr)
        called = (self.root / "bin" / "called").read_text()
        self.assertIn("metadata --locked --offline --format-version 1", called)

    def test_a_lock_behind_its_manifest_is_red_and_says_cargos_words(self):
        said = self.answer(101, BEHIND)
        self.assertEqual(said.returncode, 1, said.stderr)
        self.assertIn("--locked was passed to prevent this", said.stderr)

    def test_a_crate_this_machine_never_fetched_is_no_answer(self):
        said = self.answer(101, UNFETCHED)
        self.assertEqual(said.returncode, 3, said.stderr)

    def test_no_cargo_at_all_is_no_answer(self):
        bin_dir = self.root / "empty"
        bin_dir.mkdir()
        said = subprocess.run(
            [sys.executable, str(SCRIPT), str(self.root)],
            capture_output=True, text=True, env={**os.environ, "PATH": str(bin_dir)}, timeout=30,
        )
        self.assertEqual(said.returncode, 3, said.stderr)

    @unittest.skipUnless(shutil.which("cargo"), "no cargo on this machine")
    def test_the_real_cargo_still_says_the_words_the_verdict_reads(self):
        (self.root / "dep" / "src").mkdir(parents=True)
        (self.root / "src").mkdir()
        (self.root / "dep" / "src" / "lib.rs").write_text("")
        (self.root / "dep" / "Cargo.toml").write_text('[package]\nname = "dep"\nversion = "0.1.0"\nedition = "2021"\n')
        (self.root / "src" / "lib.rs").write_text("")
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "top"\nversion = "0.1.0"\nedition = "2021"\n[dependencies]\ndep = { path = "dep" }\n[workspace]\n'
        )
        (self.root / "Cargo.lock").write_text('version = 3\n\n[[package]]\nname = "top"\nversion = "0.1.0"\n')
        said = subprocess.run(
            [sys.executable, str(SCRIPT), str(self.root)], capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(said.returncode, 1, said.stderr)


if __name__ == "__main__":
    unittest.main()
