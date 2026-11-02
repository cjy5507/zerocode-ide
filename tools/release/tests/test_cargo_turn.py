#!/usr/bin/env python3
"""A build turn ends with its command (t-89544, tools/release/cargo-turn.sh).

On 11-02 a timer set around the turn ended the turn and left five node tests and
their Chromes running for five hours. Each case here runs the real script over a
synthetic command, in a temporary lock folder and a session of its own, and reads
whether the command's processes are still there. Only processes this test started
are ever signalled.

`CARGO_TURN_SCRIPT` runs the cases against another copy, which is how the installed
script was shown red before it was replaced.
"""

import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SCRIPT = Path(os.environ.get("CARGO_TURN_SCRIPT", Path(__file__).parents[1] / "cargo-turn.sh"))
STEP_SECS = 0.05
WAIT_SECS = 15.0


def alive(pid: int) -> bool:
    """Whether the pid stands and is not a zombie its parent has yet to take."""
    try:
        state = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, check=False)
    except OSError:
        return False
    return bool(state.stdout.strip()) and not state.stdout.strip().startswith("Z")


def wait_until(check, secs: float = WAIT_SECS) -> bool:
    deadline = time.monotonic() + secs
    while time.monotonic() < deadline:
        if check():
            return True
        time.sleep(STEP_SECS)
    return check()


@unittest.skipUnless(shutil.which("zsh") and shutil.which("pgrep"), "the build line runs on zsh with pgrep")
class CargoTurn(unittest.TestCase):
    def setUp(self):
        self.scene = tempfile.TemporaryDirectory()
        self.addCleanup(self.scene.cleanup)
        self.root = Path(self.scene.name).resolve()
        self.env = {**os.environ, "CARGO_TURN_LOCKS": str(self.root), "CARGO_TURN_GRACE_SECS": "1"}
        self.turn = None
        self.addCleanup(self.end_what_is_left)
        self.pid_files = []

    def end_what_is_left(self):
        """Whatever a red case leaves is this test's own, by the pids it wrote."""
        for path in self.pid_files:
            if path.exists():
                try:
                    os.kill(int(path.read_text().strip()), signal.SIGKILL)
                except (OSError, ValueError):
                    pass
        if self.turn is not None and self.turn.poll() is None:
            self.turn.kill()
            self.turn.wait()

    def lock(self) -> Path:
        return self.root / "cargo-turn-r.lock"

    def start(self, command: str, before=()):
        """The turn over `sh -c command`, with `before` around it: a timer, as on 11-02."""
        self.turn = subprocess.Popen(
            [*before, "zsh", str(SCRIPT), "r", "sh", "-c", command],
            env=self.env, start_new_session=True, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        return self.turn

    def scene_command(self, ignores_term: bool = False):
        """A command with a child under it, as a node test and its Chrome are; each writes its pid."""
        top, below = self.root / "top.pid", self.root / "below.pid"
        self.pid_files += [top, below]
        trap = 'trap "" TERM; ' if ignores_term else ""
        command = (
            f"{trap}echo $$ > {top}; "
            f"sh -c '{trap}echo $$ > {below}; while :; do sleep 1; done' & wait"
        )
        return command, top, below

    def read_pids(self, *paths):
        self.assertTrue(wait_until(lambda: all(path.exists() and path.read_text().strip() for path in paths)))
        return [int(path.read_text().strip()) for path in paths]

    def assert_all_gone(self, pids):
        self.assertTrue(wait_until(lambda: not any(alive(pid) for pid in pids)), f"still running: {pids}")

    def test_a_timer_around_the_turn_takes_the_command_with_it(self):
        command, top, below = self.scene_command()
        self.start(command, before=("perl", "-e", "alarm 1; exec @ARGV"))
        pids = self.read_pids(top, below)
        self.turn.wait(timeout=WAIT_SECS)
        self.assert_all_gone(pids)
        self.assertFalse(self.lock().exists(), "the lock is freed")

    def test_a_term_takes_the_command_with_it(self):
        command, top, below = self.scene_command()
        self.start(command)
        pids = self.read_pids(top, below)
        self.turn.send_signal(signal.SIGTERM)
        self.turn.wait(timeout=WAIT_SECS)
        self.assert_all_gone(pids)
        self.assertFalse(self.lock().exists(), "the lock is freed")

    def test_what_ignores_the_term_gets_the_kill(self):
        command, top, below = self.scene_command(ignores_term=True)
        self.start(command)
        pids = self.read_pids(top, below)
        self.turn.send_signal(signal.SIGTERM)
        self.turn.wait(timeout=WAIT_SECS)
        self.assert_all_gone(pids)

    def test_a_group_kill_still_reaches_the_command(self):
        """Whoever ends the turn's process group, as an agent's interrupt does, reaches the command."""
        command, top, below = self.scene_command()
        self.start(command)
        pids = self.read_pids(top, below)
        os.killpg(self.turn.pid, signal.SIGKILL)
        self.turn.wait(timeout=WAIT_SECS)
        self.assert_all_gone(pids)

    def test_a_turn_says_its_commands_code_and_frees_the_lock(self):
        turn = self.start("exit 7")
        self.assertEqual(turn.wait(timeout=WAIT_SECS), 7)
        self.assertFalse(self.lock().exists())

    def test_a_command_can_still_read_what_the_turn_was_given(self):
        said = self.root / "said"
        self.env["SAID"] = str(said)
        turn = subprocess.Popen(
            ["zsh", str(SCRIPT), "r", "sh", "-c", 'cat > "$SAID"'],
            env=self.env, start_new_session=True, stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        self.turn = turn
        turn.communicate(b"held\n", timeout=WAIT_SECS)
        self.assertEqual(said.read_text(), "held\n")

    def test_the_command_starts_with_interrupt_and_quit_at_their_defaults(self):
        """zsh starts a `&` command with INT and QUIT ignored when job control is off, and
        exec and every child keep that: a Python test would never see KeyboardInterrupt
        and a node test would not end on INT under the turn alone. (Python answers its own
        KeyboardInterrupt handler for an INT it found at the default, so the probe asks for ignored.)"""
        said = self.root / "signals"
        probe = (
            "import signal, sys; "
            "open(sys.argv[1], 'w').write(' '.join("
            "'ignored' if signal.getsignal(s) is signal.SIG_IGN else 'default' "
            "for s in (signal.SIGINT, signal.SIGQUIT)))"
        )
        turn = subprocess.Popen(
            ["zsh", str(SCRIPT), "r", sys.executable, "-c", probe, str(said)],
            env=self.env, start_new_session=True, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        self.turn = turn
        self.assertEqual(turn.wait(timeout=WAIT_SECS), 0)
        self.assertEqual(said.read_text(), "default default")

    def test_a_one_word_command_with_shell_characters_is_run_as_that_program(self):
        """The command is handed on as words, never through a shell a second time: a one-word
        program whose path holds a space and a `;` runs as itself."""
        said = self.root / "said"
        program = self.root / "run me;x"
        program.write_text(f"#!/bin/sh\necho ran > {said}\n")
        program.chmod(0o755)
        turn = self.start_words([str(program)])
        self.assertEqual(turn.wait(timeout=WAIT_SECS), 0)
        self.assertEqual(said.read_text(), "ran\n")

    def start_words(self, words):
        self.turn = subprocess.Popen(
            ["zsh", str(SCRIPT), "r", *words],
            env=self.env, start_new_session=True, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        return self.turn

    def test_a_turn_waits_for_the_one_before_it(self):
        first = self.root / "first"
        self.start(f"touch {first}; sleep 2")
        self.assertTrue(wait_until(first.exists))
        began = time.monotonic()
        second = subprocess.run(
            ["zsh", str(SCRIPT), "r", "sh", "-c", "exit 0"],
            env=self.env, stdin=subprocess.DEVNULL, capture_output=True, timeout=WAIT_SECS, check=False,
        )
        self.assertEqual(second.returncode, 0)
        self.assertGreaterEqual(time.monotonic() - began, 1.0)


if __name__ == "__main__":
    unittest.main()
