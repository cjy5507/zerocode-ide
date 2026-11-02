#!/usr/bin/env python3
"""One workspace's lock, as `just pre-handin` asks: prebatch's own verdict, so a head cannot
pass one and stop at the other (t-84719 r1 P2-1).

    lock_answer.py <folder>

Exits 0 when the lock stands, 1 when cargo says it is behind a manifest (cargo's words on
standard error), and 3 when there is no answer here: a crate this machine never fetched, no
cargo, a cargo past the cap. Those say nothing of the lock, and the gate finds them.
"""

import subprocess
import sys

import prebatch

NO_ANSWER = 3
WORDS = ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"]


def main(argv):
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        red = prebatch.run_lock(WORDS, argv[1])
    except (OSError, subprocess.SubprocessError):
        return NO_ANSWER
    if red is None:
        return NO_ANSWER
    for line in red:
        print(line, file=sys.stderr)
    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
