#!/usr/bin/env python3
"""tools/computer-bench/batch_rtt.py — what one judgement buys: the same five
steps as five lone commands, as one `sh -c 'a && b && ...'` chain (a CLI
agent can already chain), and as one `batch`. Run inside a ZeroCode pane.

Two variants. `wait` (the gate, and the default): five `wait --ms 1` — no
input, no pace tokens, no pointer. `move` (asked for by name, since it puts
the person's pointer where it already is — 0 points): the pointer is re-read
before every move, the batch and the chain, and a round the person moved it
in is dropped whole; rounds are spaced so the pace's bucket refills what a
round spends (every way's moves), the pace read from `status`.

The bar: a lone command pays one shim spawn and trip (~24.6 ms measured); a
batch of N pays one trip plus N in-window steps (~1 ms each). For N = 5 that
is (24.6 + 5) / (5 x 24.6) ~ 0.24 of the singles; BAR = 0.4 leaves room for a
loaded machine. Exit 0 iff the `wait` variant's batch p50 <= BAR x singles p50."""
import argparse
import json
import shlex
import statistics
import subprocess
import sys
import time

STEPS = 5
BAR = 0.4
SHIM = "zerocode-computer"
# The three ways a round sends the same steps; each one is paced.
WAYS = ("singles", "batch", "chain")


def call(argv):
    started = time.perf_counter()
    out = subprocess.run([SHIM, *argv], capture_output=True, text=True)
    elapsed = (time.perf_counter() - started) * 1000
    line = next((l for l in (out.stdout + out.stderr).splitlines() if l.strip().startswith("{")), "{}")
    return elapsed, json.loads(line)


def chain(commands):
    started = time.perf_counter()
    script = " && ".join(" ".join(shlex.quote(w) for w in [SHIM, *c]) for c in commands)
    done = subprocess.run(["sh", "-c", script], capture_output=True, text=True)
    return (time.perf_counter() - started) * 1000, done.returncode == 0


def summary(samples):
    if not samples:
        raise SystemExit("every round was dropped: the pointer moved in each")
    ordered = sorted(samples)
    p95 = ordered[max(0, int(round(len(ordered) * 0.95)) - 1)]
    return {"p50_ms": round(statistics.median(ordered), 1), "p95_ms": round(p95, 1)}


def cursor():
    _, answer = call(["cursor-position", "--json"])
    result = answer.get("result", {})
    return result.get("x"), result.get("y")


def pace():
    """The window's pace table, which `status` answers whether or not a
    helper session stands."""
    _, answer = call(["status", "--json"])
    table = answer.get("result", {}).get("pace")
    if not isinstance(table, dict):
        raise SystemExit(f"status answered no pace (a window older than this bench?): {answer}")
    if table.get("mode") == "unlimited":
        if table.get("perSecond") is not None or table.get("burst") is not None:
            raise SystemExit("unlimited pace must not contain a rate or burst")
        return None, None
    if table.get("mode", "paced") != "paced":
        raise SystemExit("unknown pace mode")
    rate, burst = table.get("perSecond"), table.get("burst")
    if (type(rate) is not int or type(burst) is not int or rate <= 0 or burst <= 0):
        raise SystemExit("paced mode needs a positive integer rate and burst")
    return rate, burst


class Moved(Exception):
    """The person's hand wins: a pointer that moved since the round began is
    not dragged back."""


def run(variant, rounds):
    samples, dropped = {way: [] for way in WAYS}, 0
    per_second, spend = 0.0, 0
    if variant == "move":
        per_second, burst = pace()
        spend = len(WAYS) * STEPS
        if burst is not None and spend > burst:
            raise SystemExit(f"a round spends {spend} actions but the pace's burst is {burst}")
    for n in range(rounds):
        point = None
        if variant == "wait":
            commands = [["wait", "--ms", "1"] for _ in range(STEPS)]
        else:
            point = cursor()
            if point[0] is None:
                raise SystemExit("cursor-position answered no point")
            commands = [["mouse-move", "--x", str(point[0]), "--y", str(point[1])] for _ in range(STEPS)]

        def still():
            if point is not None and cursor() != point:
                raise Moved

        halves = ["singles", "batch"] if n % 2 == 0 else ["batch", "singles"]
        taken = {}
        try:
            for half in halves:
                if half == "singles":
                    total = 0.0
                    for command in commands:
                        still()
                        elapsed, answer = call([*command, "--json"])
                        if not answer.get("ok"):
                            raise SystemExit(f"a single {command[0]} was refused: {answer}")
                        total += elapsed
                    taken["singles"] = total
                else:
                    still()
                    elapsed, answer = call(["batch", "--commands", json.dumps(commands), "--json"])
                    if not answer.get("ok") or answer.get("result", {}).get("ran") != STEPS:
                        raise SystemExit(f"the batch did not run its {STEPS} steps: {answer}")
                    taken["batch"] = elapsed
            still()
        except Moved:
            dropped += 1
            continue
        elapsed, ok = chain(commands)
        if not ok:
            raise SystemExit("the sh -c chain failed")
        taken["chain"] = elapsed
        for way in WAYS:
            samples[way].append(taken[way])
        if per_second is not None and spend and n + 1 < rounds:
            time.sleep(spend / per_second)
    report = {"singles": {**summary(samples["singles"]), "trips": STEPS},
              "chain": {**summary(samples["chain"]), "trips": STEPS},
              "batch": {**summary(samples["batch"]), "trips": 1},
              "rounds": len(samples["batch"]), "dropped": dropped}
    report["ratio_p50"] = round(report["batch"]["p50_ms"] / report["singles"]["p50_ms"], 3)
    report["ratio_to_chain_p50"] = round(report["batch"]["p50_ms"] / report["chain"]["p50_ms"], 3)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--variant", choices=["wait", "move", "both"], default="wait",
                        help="`move` and `both` put the person's pointer where it already is")
    parser.add_argument("--bar", type=float, default=BAR)
    args = parser.parse_args(argv)
    variants = ["wait", "move"] if args.variant == "both" else [args.variant]
    report = {variant: run(variant, args.rounds) for variant in variants}
    print(json.dumps(report, indent=2))
    gate = report.get("wait") or report[variants[0]]
    return 0 if gate["ratio_p50"] <= args.bar else 1


if __name__ == "__main__":
    sys.exit(main())
