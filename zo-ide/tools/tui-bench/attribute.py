#!/usr/bin/env python3
"""Attribute a macOS `sample` file's system calls to the code that made them.

    python3 zo-ide/tools/tui-bench/attribute.py run-1-streaming.sample.txt

For each leaf the kernel was in (`__open`, `stat`, `swtch_pri`, …) it prints
how many samples landed there, split by thread, and under each the nearest
frame of the program itself (the first frame whose image is the sampled
binary) — the line that asked for it. `tui_bench.py --sample N` writes these
files; `sample` counts wall time, so a thread parked in `swtch_pri` is a
thread that is not serving input, whatever the CPU meter says.
"""

from __future__ import annotations

import re
import sys
from collections import Counter, defaultdict

LEAVES = ("__open", "stat", "fstat", "lstat", "swtch_pri", "__getdirentries64", "__posix_spawn",
          "close", "read", "write", "__ioctl", "access", "__semwait_signal")
FRAME = re.compile(r"^(?P<indent>[ +!:|]*)(?P<count>\d+) (?P<symbol>.+?)\s+\(in (?P<image>[^)]+)\)"
                   r"(?: \+ \d+)?(?:\s+\[[^\]]*\])?(?:\s+(?P<where>\S+:\d+))?\s*$")
# The program's own crates (and the terminal library it drives): the frame that
# decided to make the call, past std, tokio and the allocator.
OWN = re.compile(r"^(zo_ide|zo|tools|runtime|api|commands|core_types|plugins|codegraph|crossterm)::")
THREAD = re.compile(r"^\s+(?P<count>\d+) (?P<name>Thread_\d+.*)$")


def short(symbol: str) -> str:
    symbol = re.sub(r"::h[0-9a-f]{16}$", "", symbol)
    symbol = symbol.replace("_$u7b$$u7b$closure$u7d$$u7d$", "{closure}")
    symbol = re.sub(r"_\$LT\$.*?\$GT\$", "<…>", symbol)
    return symbol


def parse(path: str) -> tuple[dict, Counter]:
    program = None
    by_leaf: dict[str, Counter] = defaultdict(Counter)
    thread_totals: Counter = Counter()
    thread = "?"
    stack: list[tuple[int, str, str, str | None]] = []
    in_graph = False
    with open(path, encoding="utf-8", errors="replace") as handle:
        for raw in handle:
            line = raw.rstrip("\n")
            if line.startswith("Call graph:"):
                in_graph = True
                continue
            if not in_graph:
                if line.startswith("Process:"):
                    program = line.split()[1]
                continue
            if line.startswith("Total number in stack"):
                break
            match = THREAD.match(line)
            if match:
                thread = match.group("name").split(":", 1)[-1].strip() or match.group("name")
                if "main-thread" in thread:
                    thread = "main"
                thread_totals[thread] += int(match.group("count"))
                stack = []
                continue
            match = FRAME.match(line)
            if not match:
                continue
            depth = len(match.group("indent"))
            count = int(match.group("count"))
            image, symbol, where = match.group("image"), match.group("symbol"), match.group("where")
            while stack and stack[-1][0] >= depth:
                stack.pop()
            stack.append((depth, symbol, image, where))
            name = symbol.strip()
            if name in LEAVES:
                caller = "?"
                for _depth, frame, frame_image, frame_where in reversed(stack[:-1]):
                    if program and frame_image.startswith(program) and OWN.match(short(frame)):
                        caller = f"{short(frame)} {frame_where or ''}".strip()
                        break
                by_leaf[(name, thread)][caller] += count
    return by_leaf, thread_totals


def main() -> int:
    for path in sys.argv[1:]:
        by_leaf, threads = parse(path)
        print(f"== {path}")
        for thread, total in threads.most_common():
            if thread == "main":
                print(f"   main thread: {total} samples")
        for (leaf, thread), callers in sorted(by_leaf.items(), key=lambda item: -sum(item[1].values())):
            total = sum(callers.values())
            if total < 3:
                continue
            print(f"{total:6d}  {leaf}  [{thread}]")
            for caller, count in callers.most_common(4):
                print(f"        {count:6d}  {caller}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
