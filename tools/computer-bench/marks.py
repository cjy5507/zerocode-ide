#!/usr/bin/env python3
"""tools/computer-bench/marks.py — what numbering a look costs, and whether a
click by number lands (docs/design/computer-use-bench.md, marks). Run inside a
ZeroCode pane.

`cost` (looks only — no input, no pointer): for the desktop and each `--app`,
N rounds of `observe` and `observe --marks` interleaved, and answers each
place's median look, the marked look's median overhead,
candidates/placed/omitted, and the legend's size in the harness's measure
(chars/4 + 1 — the way every schema here is priced). A marked look that
finds nothing to number (the desktop under ZeroCode's own window) is timed
too: it is what that look costs there. Numbering is affordable when every
place's overhead is at most one unmarked look and the legend is at most half
the picture's tokens; the script prints that verdict and exits 0 only when it
holds. Whether zo's looks carry marks (`COMPUTER_LOOKS_CARRY_MARKS`) is then
the bench's A/B to say (`bench.py run --config default --config marks`).

`click` (drives Calculator — pass --i-am-here, and only when the person
agreed): a marked look of Calculator, each digit key clicked by its mark, the
display read back; answers digits_ok/10, how many presses took the
accessibility path (the rest moved the pointer), and the median click by mark
against the median `mouse-click` on the same key.
"""
import argparse
import json
import statistics
import subprocess
import sys
import time

SHIM = "zerocode-computer"
# A picture's price in the model's context, as CHANGELOG 1.3.35 measured it:
# the legend is weighed against this.
PICTURE_TOKENS = 1418


def call(argv):
    started = time.perf_counter()
    out = subprocess.run([SHIM, *argv, "--json"], capture_output=True, text=True)
    elapsed = (time.perf_counter() - started) * 1000
    line = next((line for line in (out.stdout + out.stderr).splitlines() if line.strip().startswith("{")), "{}")
    return elapsed, json.loads(line)


def legend_tokens(items):
    """The legend as the model would read it (the core's `legend_line`
    shape), priced chars/4 + 1."""
    lines = []
    for item in items:
        label = f" {item['label']}" if item.get("label") else ""
        role = f" {item['role']}" if item.get("role") else ""
        lines.append(f"{item['mark']}{role}{label} @{round(item['centerX'])},{round(item['centerY'])}")
    text = json.dumps(lines, ensure_ascii=False)
    return len(text) // 4 + 1


def cost(args):
    places = [[]] + [["--app", app] for app in args.app]
    report, holds = [], True
    for place in places:
        plain, marked, counts, tokens = [], [], None, []
        for _ in range(args.n):
            ms, answer = call(["observe", *place])
            if answer.get("ok"):
                plain.append(ms)
            ms, answer = call(["observe", *place, "--marks"])
            result = answer.get("result") or {}
            marks = result.get("marks") or {}
            if not answer.get("ok"):
                continue
            if "items" in marks:
                marked.append(ms)
                counts = (marks.get("candidates"), len(marks["items"]), marks.get("omitted"))
                tokens.append(legend_tokens(marks["items"]))
            elif marks.get("unavailable"):
                marked.append(ms)
                counts = marks["unavailable"]
                tokens.append(0)
        name = " ".join(place) or "desktop"
        if not plain or not marked:
            report.append({"place": name, "error": counts or "no marked look answered"})
            holds = False
            continue
        base, with_marks = statistics.median(plain), statistics.median(marked)
        legend = statistics.median(tokens)
        ok = with_marks - base <= base and legend <= PICTURE_TOKENS / 2
        holds = holds and ok
        report.append({
            "place": name, "n": args.n, "observe_ms": round(base, 1), "marked_ms": round(with_marks, 1),
            "overhead_ms": round(with_marks - base, 1), "legend_tokens": legend,
            "candidates_placed_omitted": counts, "holds": ok,
        })
    print(json.dumps({"cost": report, "affordable": holds}, ensure_ascii=False, indent=2))
    return 0 if holds else 1


def click(args):
    if not args.i_am_here:
        print("marks.py click drives Calculator on this desktop: pass --i-am-here once the person agreed", file=sys.stderr)
        return 2
    _, look = call(["observe", "--app", "Calculator", "--marks"])
    marks = ((look.get("result") or {}).get("marks") or {})
    items = {str(item.get("label")): item for item in marks.get("items") or []}
    look_id = marks.get("lookId")
    ok, paths, by_mark, by_point = 0, {}, [], []
    for digit in "1234567890":
        item = items.get(digit)
        if not item or not look_id:
            continue
        call(["key", "--key", "escape"])
        ms, answer = call(["click", "--mark", str(item["mark"]), "--look", look_id])
        by_mark.append(ms)
        path = ((answer.get("result") or {}).get("action") or {}).get("path", "refused")
        paths[path] = paths.get(path, 0) + 1
        _, read = call(["read", "--app", "Calculator"])
        text = (read.get("result") or {}).get("text") or ""
        ok += any(line.split()[-1:] == [digit] for line in text.splitlines() if " text " in line)
        ms, _ = call(["mouse-click", "--x", str(item["centerX"]), "--y", str(item["centerY"])])
        by_point.append(ms)
        _, look = call(["observe", "--app", "Calculator", "--marks"])
        look_id = ((look.get("result") or {}).get("marks") or {}).get("lookId") or look_id
    print(json.dumps({
        "digits_ok": f"{ok}/10", "paths": paths,
        "click_by_mark_ms": round(statistics.median(by_mark), 1) if by_mark else None,
        "mouse_click_ms": round(statistics.median(by_point), 1) if by_point else None,
    }, indent=2))
    return 0 if ok == 10 else 1


def main(argv):
    parser = argparse.ArgumentParser(prog="marks.py", description=__doc__.split("\n\n")[0])
    parser.add_argument("mode", choices=["cost", "click"])
    parser.add_argument("--app", action="append", default=[])
    parser.add_argument("--n", type=int, default=20)
    parser.add_argument("--i-am-here", action="store_true")
    args = parser.parse_args(argv[1:])
    return cost(args) if args.mode == "cost" else click(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
