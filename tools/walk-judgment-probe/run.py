#!/usr/bin/env python3
"""The same goal walk, timed before a change and after it, on a page of our own.

    TYPESAFE_API_KEY="$(security find-generic-password -s dev.zerocode.key.TYPESAFE_API_KEY -a "$(id -un)" -w)" \\
      python3 tools/walk-judgment-probe/run.py --before <sha> --out <dir> [--walks 8]

What runs is the product's own walk — the question, the Jev wire, the goal
world and, in the build after, the value seat — driven by an ignored test in
`crates/zerocode-shell/src/computer_use/errand/probe.rs`. The one thing that
is not the window's is the road: each call is the window's browser CLI, and a
typed value goes to it on stdin. The build BEFORE is `--before`'s product
files with this harness laid over them (the harness's `// after-only` lines
dropped, since the build before has no value seat); the build AFTER is HEAD.
Both are built in this checkout's own target, one after the other, and every
walk alternates the two (ABBA), each on a freshly loaded page. Each run of a
build walks twice in one process: the first walk pays for the process's cold
connections (the window keeps its warm), so it is reported on its own, and the
table is the second — the walk a long-lived window takes.

Scenarios (on `page.html` beside this file, `steps` and `later` on `steps.html`):

- press   — "press Advance once", until the page says so
- repeat  — the same, asked as a replay with the judgment cache on
- type    — "search for London": an entry, then a press
- observe — which container, image and row the goal is about (one step)
- steps   — "press Next until Step 4 of 4": three presses, each changing the
            page at once (t-9712)
- later   — the same walk on `steps.html?delay=120`: each press changes the
            page 120 ms after it answered, the step busy until then, so the
            page a press leaves is the page it was pressed on — the shape of a
            page that fetches (t-9712 r2)

Arms (`--arms`, t-9712): `before` and `after` walk as a plain `walk` does;
`before-ahead` and `after-ahead` walk as `walk --overlap` does, asking ahead. The
running window's door may predate the one a build expects (a v1.1.25 window
does not settle a press by number), so the harness stands that door in front
of it (`probe.rs` `Door`): a press by number settles, by the product's own
loop, each poll one `eval`; a press that leaves its settle for later answers
the page it left and the next look finishes the settle. What the stand-in did
inside a call is kept on that call (`inside`: `settleMs`, `previewMs`,
`standInMs`) and every table says it.

The page's snapshot the browser door does not carry yet (t-6721 U4) is stood
in for by `snapshot.js`, one more `eval` after each look, in both builds and
only in the scenarios that need it; its time is kept apart. Rows land in
`<out>/rows.jsonl`; the table is `<out>/table.md` and `<out>/summary.json`.
The keys are read from this command's environment only and written nowhere:
`TYPESAFE_API_KEY` for Jev, and — when a person has one — `ANTHROPIC_API_KEY`
for the value seat in the build after, the way a person's own key reaches it
in the window. Without it the build after offers no entry, as the window does
for a person who set no key; no subscription login is ever used.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
PROBE = ROOT / "crates/zerocode-shell/src/computer_use/errand/probe.rs"
ERRAND = ROOT / "crates/zerocode-shell/src/computer_use/errand.rs"
TEST_NAME = "computer_use::errand::probe::a_goal_walk_timed_on_a_page_of_our_own"
MODULE_LINE = "#[cfg(test)]\nmod probe;\n"

SCENARIOS = {
    "press": {
        "goal": "Press the Advance button once.",
        "until": "Advanced 1",
        "steps": 2,
        "stand_in": False,
    },
    "repeat": {
        "goal": "Press the Advance button once.",
        "until": "Advanced 1",
        "steps": 2,
        "stand_in": False,
        "replay": True,
        "cache": "on",
    },
    "type": {
        "goal": "Search for London: enter London as the destination, then press Search.",
        "until": "Searched: London",
        "steps": 3,
        "stand_in": True,
    },
    "observe": {
        "goal": "Find the iPhone 16 Pro in the search results.",
        "until": None,
        "steps": 1,
        "stand_in": True,
    },
    "steps": {
        "goal": "Go through the checkout: press Next until the page says Step 4 of 4.",
        "until": "Step 4 of 4",
        "steps": 4,
        "stand_in": False,
        "page": "steps.html",
        "ready": "#next",
    },
}
# The steps walk on the same page answering later: the step comes from a
# worker's timer, since a hidden pane holds its own to the next second.
SCENARIOS["later"] = {**SCENARIOS["steps"], "query": "delay=120"}

# The arms a run may walk: which build, and whether the walk asks ahead
# (`walk --overlap`).
ARMS = {
    "before": ("before", False),
    "after": ("after", False),
    "before-ahead": ("before", True),
    "after-ahead": ("after", True),
}

# What the observation heads should choose on page.html: the search results,
# the phone's own picture, and its row — the look's own numbers.
OBSERVE_ORACLE = {"container": 1, "image": 1, "row": 1}


def strip_after_only(source: str) -> str:
    """The harness as the build before sees it: every `// after-only {` …
    `// after-only }` region dropped, markers and all."""
    return re.sub(r"[ \t]*// after-only \{\n.*?// after-only \}\n", "", source, flags=re.S)


def page_url(spec: dict) -> str:
    """A scenario's own page beside this file, with its query when it has one."""
    url = (HERE / spec["page"]).as_uri()
    return f"{url}?{spec['query']}" if spec.get("query") else url


def percentile(values: list[float], share: float) -> float | None:
    """Nearest rank; `None` for no values."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(share * len(ordered)) - 1)]


def inside(call: dict, key: str) -> float:
    """What the harness did inside one call (the stand-in door, the stand-in
    snapshot), in ms — never the product's own road."""
    return float((call.get("inside") or {}).get(key) or 0.0)


def steps_of(row: dict) -> list[dict]:
    """A walk's hand steps, from its calls: a step opens at a look (`marks`)
    and ends where its last press or entry ended; the stand-in snapshot's time
    inside it is taken out, so a step is what the product's own road costs
    (a settle the door ran inside a call stays in: it is the product's)."""
    calls = row.get("calls") or []
    stand_ins = list(row.get("standInMs") or [])
    steps, current = [], None
    looks = 0
    for call in calls:
        if call["verb"] == "marks":
            if current and current["end"] is not None:
                steps.append(current)
            # A row that kept its stand-in times apart, one per look.
            stand_in = stand_ins[looks] if looks < len(stand_ins) else inside(call, "standInMs")
            looks += 1
            current = {"start": call["startMs"], "end": None, "kind": "press", "standIn": stand_in}
        elif call["verb"] in ("click", "type") and current is not None:
            current["end"] = call["startMs"] + call["ms"]
            current["standIn"] += inside(call, "standInMs")
            if call["verb"] == "type":
                current["kind"] = "type"
        elif call["verb"] == "find" and current is not None and current["end"] is not None:
            steps.append(current)
            current = None
    if current and current["end"] is not None:
        steps.append(current)
    return [
        {"kind": step["kind"], "ms": step["end"] - step["start"] - step["standIn"], "standInMs": step["standIn"]}
        for step in steps
    ]


def press_gaps(row: dict) -> list[float]:
    """The time from one hand to the next — a press's end to the next press's
    or entry's end — what a person watching the walk waits between two
    presses, the settle and the look between them included; the stand-in
    snapshot's time in between is taken out."""
    ends, gaps, stand_in = [], [], 0.0
    for call in row.get("calls") or []:
        stand_in += inside(call, "standInMs")
        if call["verb"] in ("click", "type") and call.get("exit", 0) == 0:
            ends.append((call["startMs"] + call["ms"], stand_in))
    for (was, before), (now, after) in zip(ends, ends[1:]):
        gaps.append(now - was - (after - before))
    return gaps


def succeeded(scenario: str, row: dict) -> bool:
    """The page's own word, never the walk's."""
    oracle = row.get("oracle") or {}
    if scenario in ("press", "repeat"):
        return oracle.get("count") == 1
    if scenario in ("steps", "later"):
        return oracle.get("count") == 3
    if scenario == "type":
        return oracle.get("searched") == "London"
    if scenario == "observe":
        observed = (row.get("rows") or [{}])[0].get("observed") or {}
        return all((observed.get(head) or {}).get("chosen") == number for head, number in OBSERVE_ORACLE.items())
    return False


# Walks one run of a build takes in its process: the first pays for cold
# connections, the second is the steady walk the table reports.
WALKS_A_PROCESS = 2


def summarize(rows: list[dict], first: bool = False) -> dict:
    """The table's cells, of the steady walks — or, `first`, of the walks
    that opened their process."""
    table = {}
    for row in rows:
        if (row.get("walk", 0) == 0) != first:
            continue
        key = (row["scenario"], row["label"])
        cell = table.setdefault(key, {"walks": [], "press": [], "type": [], "ok": 0, "n": 0,
                                      "requests": 0, "judged": 0, "cached": 0, "written": 0,
                                      "reused": 0, "large": 0, "standIn": [], "loads": [],
                                      "gaps": [], "settles": [], "previews": [], "asks": [],
                                      "overlapped": 0, "discarded": 0, "cancelled": 0,
                                      "settledReady": 0, "settledAll": 0})
        cell["n"] += 1
        cell["ok"] += succeeded(row["scenario"], row)
        # A walk's own time, less what the stand-in snapshot took inside it.
        cell["walks"].append(row["walkMs"] - sum(inside(call, "standInMs") for call in row.get("calls") or []))
        if row.get("load") is not None:
            cell["loads"].append(row["load"])
        for step in steps_of(row):
            cell[step["kind"]].append(step["ms"])
            if step["standInMs"]:
                cell["standIn"].append(step["standInMs"])
        cell["gaps"].extend(press_gaps(row))
        for call in row.get("calls") or []:
            if (call.get("inside") or {}).get("settle"):
                cell["settles"].append(inside(call, "settleMs"))
                cell["settledAll"] += 1
                cell["settledReady"] += call["inside"]["settle"] == "ready"
            if inside(call, "previewMs"):
                cell["previews"].append(inside(call, "previewMs"))
        if row.get("asked") is not None:
            cell["asks"].append(int(row.get("asked") or 0) + int(row.get("begun") or 0))
        for word in ("overlapped", "discarded", "cancelled"):
            cell[word] += int(row.get(word) or 0)
        for judged in row.get("rows") or []:
            if judged.get("outcome") == "answered" or judged.get("requests") is not None:
                cell["judged"] += 1
            cell["requests"] += int(judged.get("requests") or 0)
            cell["cached"] += bool(judged.get("cached"))
            model = judged.get("model")
            if model and not str(model).startswith("jev"):
                cell["large"] += 1
            typed = judged.get("typed") or {}
            cell["written"] += typed.get("source") == "written"
            cell["reused"] += typed.get("source") == "reused"
    summary = {}
    for (scenario, label), cell in sorted(table.items()):
        summary[f"{scenario}/{label}"] = {
            "n": cell["n"],
            "ok": cell["ok"],
            "walk_p50": percentile(cell["walks"], 0.5),
            "walk_p95": percentile(cell["walks"], 0.95),
            "press_p50": percentile(cell["press"], 0.5),
            "press_p95": percentile(cell["press"], 0.95),
            "type_p50": percentile(cell["type"], 0.5),
            "type_p95": percentile(cell["type"], 0.95),
            "jev_requests": cell["requests"],
            "judged_steps": cell["judged"],
            "memo_answers": cell["cached"],
            "large_model_calls": cell["large"],
            "values_written": cell["written"],
            "values_reused": cell["reused"],
            "stand_in_p50": percentile(cell["standIn"], 0.5),
            "load_min": min(cell["loads"]) if cell["loads"] else None,
            "load_max": max(cell["loads"]) if cell["loads"] else None,
            "gap_p50": percentile(cell["gaps"], 0.5),
            "gap_p95": percentile(cell["gaps"], 0.95),
            "gaps": len(cell["gaps"]),
            "settle_p50": percentile(cell["settles"], 0.5),
            "settle_p95": percentile(cell["settles"], 0.95),
            "settled_ready": cell["settledReady"],
            "settled": cell["settledAll"],
            "preview_p50": percentile(cell["previews"], 0.5),
            "asks_per_walk": (sum(cell["asks"]) / len(cell["asks"])) if cell["asks"] else None,
            "overlapped": cell["overlapped"],
            "discarded": cell["discarded"],
            "cancelled": cell["cancelled"],
        }
    return summary


def table_md(summary: dict, command: str, out: pathlib.Path) -> str:
    def ms(value):
        return "—" if value is None else f"{value:,.0f}"

    def mean(value):
        return "—" if value is None else f"{value:.2f}"

    lines = [
        f"`{command}` → `{out}`",
        "",
        "| scenario/arm | n | ok | walk p50/p95 | press step p50/p95 | press gap p50/p95 (n) | type step p50/p95 "
        "| settle p50/p95 (ready/all) | preview p50 | ahead used/dropped/cancelled | asked+begun per walk "
        "| Jev requests / judged steps | memo | large model | values written/reused | stand-in p50 | load |",
        "|---|---:|---:|---|---|---|---|---|---:|---|---:|---|---:|---:|---|---:|---|",
    ]
    for name, cell in summary.items():
        lines.append(
            f"| {name} | {cell['n']} | {cell['ok']}/{cell['n']} | {ms(cell['walk_p50'])} / {ms(cell['walk_p95'])} "
            f"| {ms(cell['press_p50'])} / {ms(cell['press_p95'])} "
            f"| {ms(cell.get('gap_p50'))} / {ms(cell.get('gap_p95'))} ({cell.get('gaps', 0)}) "
            f"| {ms(cell['type_p50'])} / {ms(cell['type_p95'])} "
            f"| {ms(cell.get('settle_p50'))} / {ms(cell.get('settle_p95'))} ({cell.get('settled_ready', 0)}/{cell.get('settled', 0)}) "
            f"| {ms(cell.get('preview_p50'))} "
            f"| {cell.get('overlapped', 0)}/{cell.get('discarded', 0)}/{cell.get('cancelled', 0)} "
            f"| {mean(cell.get('asks_per_walk'))} "
            f"| {cell['jev_requests']} / {cell['judged_steps']} | {cell['memo_answers']} | {cell['large_model_calls']} "
            f"| {cell['values_written']}/{cell['values_reused']} | {ms(cell['stand_in_p50'])} "
            f"| {cell['load_min']}–{cell['load_max']} |"
        )
    return "\n".join(lines) + "\n"


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def build(label: str, before: str | None, bins: pathlib.Path) -> pathlib.Path:
    """One test binary: HEAD, or `before`'s product files under this harness.
    The checkout is put back as HEAD has it whatever happens."""
    if git("status", "--porcelain", "--untracked-files=no").strip():
        sys.exit("the checkout has changes; commit them before a build swaps files")
    touched = []
    try:
        if before:
            changed = [name for name in git("diff", "--name-only", before, "HEAD", "--", "crates").split()
                       if pathlib.Path(name) != PROBE.relative_to(ROOT)]
            present = set(git("ls-tree", "-r", "--name-only", before, "--", "crates").split())
            for name in changed:
                path = ROOT / name
                touched.append(name)
                if name in present:
                    path.write_text(git("show", f"{before}:{name}"))
                elif path.exists():
                    path.unlink()
            errand = ERRAND.read_text()
            if "mod probe;" not in errand:
                errand = errand.replace("pub mod team;\n", "pub mod team;\n" + MODULE_LINE, 1)
                ERRAND.write_text(errand)
                touched.append(str(ERRAND.relative_to(ROOT)))
            PROBE.write_text(strip_after_only(PROBE.read_text()))
            touched.append(str(PROBE.relative_to(ROOT)))
        env = dict(os.environ, CARGO_INCREMENTAL="0")
        done = subprocess.run(
            ["cargo", "test", "-p", "zerocode-shell", "--bin", "zerocode-shell", "--no-run",
             "--message-format=json"],
            cwd=ROOT, env=env, capture_output=True, text=True)
        if done.returncode != 0:
            sys.stderr.write(done.stderr[-4000:])
            sys.exit(f"the {label} build failed ({done.returncode})")
        executable = None
        for line in done.stdout.splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("reason") == "compiler-artifact" and message.get("executable") \
                    and message.get("target", {}).get("name") == "zerocode-shell" and message.get("profile", {}).get("test"):
                executable = message["executable"]
        if not executable:
            sys.exit(f"the {label} build named no test binary")
        bins.mkdir(parents=True, exist_ok=True)
        kept = bins / label
        shutil.copy2(executable, kept)
        return kept
    finally:
        # Put back every file HEAD has, and drop one it does not have.
        for name in touched:
            if git("ls-files", "--", name).strip():
                git("checkout", "HEAD", "--", name)
            else:
                (ROOT / name).unlink(missing_ok=True)


def walk_once(binary: pathlib.Path, label: str, scenario: str, pane: str, url: str,
              out: pathlib.Path, home: pathlib.Path, value_key: str | None, overlap: bool = False) -> int:
    spec = SCENARIOS[scenario]
    env = dict(os.environ)
    env.update({
        "ZEROCODE_WALK_PROBE_PANE": pane,
        "ZEROCODE_WALK_PROBE_URL": page_url(spec) if spec.get("page") else url,
        "ZEROCODE_WALK_PROBE_OVERLAP": "1" if overlap else "0",
        "ZEROCODE_WALK_PROBE_READY": spec.get("ready", "#search"),
        "ZEROCODE_WALK_PROBE_KEY": os.environ["TYPESAFE_API_KEY"],
        "ZEROCODE_WALK_PROBE_OUT": str(out / "rows.jsonl"),
        "ZEROCODE_WALK_PROBE_HOME": str(home),
        "ZEROCODE_WALK_PROBE_GOAL": spec["goal"],
        "ZEROCODE_WALK_PROBE_UNTIL": spec["until"] or "",
        "ZEROCODE_WALK_PROBE_STEPS": str(spec["steps"]),
        "ZEROCODE_WALK_PROBE_WALKS": str(WALKS_A_PROCESS),
        "ZEROCODE_WALK_PROBE_LABEL": label,
        "ZEROCODE_WALK_PROBE_SCENARIO": scenario,
        "ZEROCODE_WALK_PROBE_REPLAY": "1" if spec.get("replay") else "0",
        "ZEROCODE_WALK_PROBE_CACHE": spec.get("cache", "off"),
        "ZO_CONFIG_HOME": str(home),
    })
    env.pop("TYPESAFE_API_KEY", None)
    env.pop("ANTHROPIC_API_KEY", None)
    if spec["stand_in"]:
        env["ZEROCODE_WALK_PROBE_SNAPSHOT_JS"] = str(HERE / "snapshot.js")
    else:
        env.pop("ZEROCODE_WALK_PROBE_SNAPSHOT_JS", None)
    if value_key:
        env["ZEROCODE_WALK_PROBE_VALUE_KEY"] = value_key
    else:
        env.pop("ZEROCODE_WALK_PROBE_VALUE_KEY", None)
    done = subprocess.run([str(binary), "--exact", TEST_NAME, "--ignored", "--nocapture", "--test-threads=1"],
                          cwd=ROOT, env=env, capture_output=True, text=True)
    said = [line.split("... ")[-1] for line in done.stdout.splitlines() if " walk " in line]
    print(f"  {label:6} {scenario:7} rc={done.returncode} {' | '.join(said) or done.stderr[-300:]}", flush=True)
    return done.returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--before", required=True, help="the commit whose product files make the build before")
    parser.add_argument("--out", required=True)
    parser.add_argument("--walks", type=int, default=8, help="walks per scenario per build")
    parser.add_argument("--scenarios", default="press,repeat,type,observe")
    parser.add_argument("--arms", default="before,after",
                        help="which of " + ",".join(ARMS) + " to walk, turn about")
    parser.add_argument("--pane", help="a pane of your own; one is opened (and closed) when absent")
    parser.add_argument("--bins", help="reuse the binaries a previous run built here")
    args = parser.parse_args()
    if "TYPESAFE_API_KEY" not in os.environ:
        sys.exit("TYPESAFE_API_KEY is read from this command's environment only")
    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    bins = pathlib.Path(args.bins).resolve() if args.bins else out / "bin"
    head = git("rev-parse", "HEAD").strip()
    if not args.bins:
        build("before", args.before, bins)
        build("after", None, bins)
    arms = [arm for arm in args.arms.split(",") if arm]
    unknown = [arm for arm in arms if arm not in ARMS]
    if unknown:
        sys.exit(f"unknown arms {unknown}; the arms are {list(ARMS)}")
    url = (HERE / "page.html").as_uri()
    pane, opened = args.pane, False
    if not pane:
        said = subprocess.run(["zerocode-browser", "open", url], capture_output=True, text=True, check=True).stdout
        pane = said.split()[-1]
        opened = True
    homes = {label: pathlib.Path(tempfile.mkdtemp(prefix=f"walk-probe-{label}-", dir=out)) for label in arms}
    try:
        for scenario in args.scenarios.split(","):
            print(f"{scenario}:", flush=True)
            for walk in range(args.walks):
                # Turn about: A B C, then C B A.
                order = arms if walk % 2 == 0 else list(reversed(arms))
                for label in order:
                    built, overlap = ARMS[label]
                    walk_once(bins / built, label, scenario, pane, url, out, homes[label],
                              os.environ.get("ANTHROPIC_API_KEY") if built == "after" else None,
                              overlap)
    finally:
        if opened:
            subprocess.run(["zerocode-browser", "close", pane], capture_output=True, text=True)
    rows = [json.loads(line) for line in (out / "rows.jsonl").read_text().splitlines() if line.strip()]
    steady, first = summarize(rows), summarize(rows, first=True)
    command = (f"python3 tools/walk-judgment-probe/run.py --before {args.before} --out {out} --walks {args.walks}"
               f" --scenarios {args.scenarios} --arms {args.arms}")
    (out / "summary.json").write_text(json.dumps(
        {"before": args.before, "after": head, "steady": steady, "firstInProcess": first}, indent=1))
    text = table_md(steady, command, out) + "\nFirst walk of each process (cold connections):\n\n" \
        + table_md(first, command, out)
    (out / "table.md").write_text(text)
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
