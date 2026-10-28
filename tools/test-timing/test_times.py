#!/usr/bin/env python3
"""How long each test of one Rust test target takes, and where the wall clock goes.

    test_times.py [show] [--package P] [--bin B | --test T] [--top N]   # the table; measures first when no result is kept
    test_times.py measure [--again] [--jobs N] [--threads N] [--mode json|groups] [-- FILTER…]
    test_times.py table RESULT.json [--top N] [--depth D]
    test_times.py compare BEFORE.json AFTER.json [--top N]

`measure` builds the target once (`cargo test --no-run`, timed apart from the
run), then starts the test executable itself and keeps every test's seconds in
a result file named after the package, the target and the commit — so a second
run on the same commit only reads the table, and the commit before a fix and
the commit after it each keep their own file for `compare`.

Two ways to time a test, both on the same built executable:

  json    (default) the whole suite at once, with libtest's own per-test clock
          (`-Z unstable-options --format json --report-time`). Stable libtest
          takes those flags when `RUSTC_BOOTSTRAP` is set in ITS environment;
          the variable is given to the test executable only, never to cargo, so
          the build and its fingerprint are the ones every other check uses.
          The seconds are measured among the other tests running at the same
          time, which is the run a gate pays for.
  groups  the fallback: `--list` names the tests, they are run one module group
          at a time with one test thread, and a test's seconds are the time
          between its line and the line before it. Slower, and alone on the
          machine; use it when the json mode is refused.

The machine's load average before and after, the cargo and rust variables in
the environment, the thread count and the commit are written beside the
seconds: a before and an after are comparable only when those agree.

    nice -n 19 env CARGO_INCREMENTAL=0 tools/test-timing/test_times.py measure --jobs 4
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import tomllib
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_PACKAGE = "zerocode-shell"
DEFAULT_TOP = 50
DEFAULT_DEPTH = 2          # module path segments a group or a module sum is cut at
RESULTS = Path(tempfile.gettempdir()) / "zerocode-test-times"
# The environment that changes what is built or how fast it runs; kept with the result.
CONDITION_PREFIXES = ("CARGO_", "RUSTFLAGS", "RUSTC", "RUST_TEST_THREADS")


def git(repo: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True, check=True).stdout.strip()


def commit_label(repo: Path) -> str:
    """The commit the tree is at, and `-dirty<hash of the diff>` when the tree holds changes the commit does not.

    The diff's hash, not `-dirty` alone: two different edits on one commit are two different trees to measure.
    """
    head = git(repo, "rev-parse", "--short=12", "HEAD")
    diff = subprocess.run(["git", "-C", str(repo), "diff", "HEAD"], capture_output=True, check=True).stdout
    return head + (f"-dirty{hashlib.sha256(diff).hexdigest()[:8]}" if diff else "")


def target_args(args: argparse.Namespace) -> list[str]:
    if args.test:
        return ["--test", args.test]
    return ["--bin", args.bin or args.package]


def result_path(args: argparse.Namespace, commit: str | None = None) -> Path:
    """Named by what was measured: a filtered or single-thread run never stands in for the whole suite."""
    kind, name = target_args(args)
    commit = commit or commit_label(args.checkout)
    variant = "".join([f"-threads{args.threads}" if args.threads else "", f"-{args.mode}" if args.mode != "json" else "",
                       *(f"-{re.sub(r'[^A-Za-z0-9_]+', '_', word)}" for word in args.filters)])
    return RESULTS / f"{args.package}-{kind.lstrip('-')}-{name}{variant}-{commit}.json"


def load_average() -> list[float]:
    try:
        return [round(value, 2) for value in os.getloadavg()]
    except OSError:
        return []


def debug_setting(checkout: Path) -> dict:
    """The debug info the dev and test builds carried: the environment's override, else the checkout's Cargo.toml.

    A before and an after on two sides of a profile change differ in it, and a table that does not say so
    reads the profile's effect as the code's.
    """
    try:
        profiles = tomllib.loads((checkout / "Cargo.toml").read_text()).get("profile", {})
    except (OSError, tomllib.TOMLDecodeError):
        profiles = {}
    def own(profile: str) -> str | None:
        variable = f"CARGO_PROFILE_{profile.upper()}_DEBUG"
        if variable in os.environ:
            return f"{os.environ[variable]} ({variable})"
        if "debug" in profiles.get(profile, {}):
            return f"{profiles[profile]['debug']} (Cargo.toml)"
        return None

    dev = own("dev") or "cargo's default"
    return {"dev": dev, "test": own("test") or f"{dev}, from dev"}   # the test profile inherits dev


def power() -> dict:
    """Where the power comes from and the power mode (macOS `pmset`): a run on battery is not comparable with one on AC."""
    try:
        source = subprocess.run(["pmset", "-g", "batt"], capture_output=True, text=True).stdout.splitlines()
        settings = subprocess.run(["pmset", "-g"], capture_output=True, text=True).stdout.split()
    except OSError:
        return {}
    mode = settings[settings.index("powermode") + 1] if "powermode" in settings else None
    return {"source": source[0].strip() if source else None, "powermode": mode}


def cargo(args: argparse.Namespace, *words: str) -> list[str]:
    return ["cargo", *words, *(["-j", str(args.jobs)] if args.jobs else [])]


def clean(args: argparse.Namespace) -> None:
    """A cold build: the checkout's whole target directory goes first (`cargo clean`, which honours CARGO_TARGET_DIR)."""
    print("clean: cargo clean", file=sys.stderr)
    subprocess.run(["cargo", "clean"], cwd=args.checkout, check=True)


def clippy(args: argparse.Namespace) -> dict:
    """The lint a worker runs, after the build, on the same target directory: its seconds and its exit code."""
    command = cargo(args, "clippy", "-p", args.package, "--all-targets") + ["--", "-D", "warnings"]
    print("clippy:", " ".join(command), file=sys.stderr)
    started = time.monotonic()
    done = subprocess.run(command, cwd=args.checkout)
    return {"seconds": round(time.monotonic() - started, 1), "exit": done.returncode}


def build(args: argparse.Namespace) -> tuple[Path, Path, float]:
    """Build the target's test executable; its path, the directory cargo runs it from, and the seconds."""
    command = cargo(args, "test", "-p", args.package, *target_args(args), "--no-run",
                    "--message-format=json-render-diagnostics")
    if args.timings:
        command.append("--timings")   # cargo writes target/cargo-timings/cargo-timing.html: the slowest crates
    print("build:", " ".join(command), file=sys.stderr)
    started = time.monotonic()
    done = subprocess.run(command, cwd=args.checkout, stdout=subprocess.PIPE, text=True)
    seconds = time.monotonic() - started
    if done.returncode != 0:
        sys.exit(f"build failed (exit {done.returncode}) after {seconds:.0f} s")
    executable, manifest = find_executable(done.stdout.splitlines(), target_args(args)[1])
    return executable, manifest.parent, seconds


def find_executable(lines: list[str], target: str) -> tuple[Path, Path]:
    """The test-profile executable cargo built for `target`, out of its JSON messages, and its manifest."""
    for line in lines:
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue   # the build line's own chatter
        if (message.get("reason") == "compiler-artifact" and message.get("profile", {}).get("test")
                and message.get("executable") and message.get("target", {}).get("name") == target):
            return Path(message["executable"]), Path(message["manifest_path"])
    sys.exit(f"cargo built no test executable for {target}")


def run_environment(manifest_dir: Path, json_mode: bool) -> dict[str, str]:
    """What `cargo test` would hand the executable, plus the libtest switch in json mode."""
    env = dict(os.environ, CARGO_MANIFEST_DIR=str(manifest_dir))
    if json_mode:
        env["RUSTC_BOOTSTRAP"] = "1"
    return env


def list_tests(executable: Path, cwd: Path, filters: list[str]) -> list[str]:
    listed = subprocess.run([str(executable), "--list", "--format", "terse", *filters], cwd=cwd,
                            env=run_environment(cwd, False), capture_output=True, text=True, check=True)
    return [line[: -len(": test")] for line in listed.stdout.splitlines() if line.endswith(": test")]


def read_json_events(lines, clock) -> dict:
    """Fold libtest's JSON event lines into tests and a suite summary.

    `clock()` is read as each line arrives: a test's `started`/`finished` are
    seconds since the first line, which is what says which tests the run's last
    seconds waited on.
    """
    origin = None
    tests: dict[str, dict] = {}
    suite: dict = {}
    for line in lines:
        now = clock()
        if origin is None:
            origin = now
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue   # a test's own print that escaped capture
        if not isinstance(event, dict):
            continue
        at = round(now - origin, 3)
        if event.get("type") == "suite" and event.get("event") != "started":
            suite = {key: event[key] for key in ("event", "passed", "failed", "ignored", "filtered_out", "exec_time")
                     if key in event}
        elif event.get("type") == "test":
            name = event.get("name")
            row = tests.setdefault(name, {"name": name})
            if event.get("event") == "started":
                row["started"] = at
            else:
                row["outcome"] = event.get("event")
                row["finished"] = at
                if "exec_time" in event:
                    row["seconds"] = round(float(event["exec_time"]), 4)
    return {"tests": list(tests.values()), "suite": suite}


def run_json(executable: Path, cwd: Path, threads: int | None, filters: list[str]) -> dict:
    command = [str(executable), "-Z", "unstable-options", "--format", "json", "--report-time", *filters]
    if threads:
        command += ["--test-threads", str(threads)]
    print("run:", " ".join(command), file=sys.stderr)
    with subprocess.Popen(command, cwd=cwd, env=run_environment(cwd, True), stdout=subprocess.PIPE,
                          text=True, bufsize=1) as child:
        folded = read_json_events(child.stdout, time.monotonic)
    folded["exit"] = child.returncode
    if not folded["tests"]:
        sys.exit(f"the executable reported no tests (exit {child.returncode}); try --mode groups")
    return folded


def group_of(name: str, depth: int) -> str:
    return "::".join(name.split("::")[:depth])


def read_serial_lines(lines, clock, started: float) -> list[dict]:
    """One test thread prints `test NAME ... ok` as each test ends; a test's seconds are the gap before its line."""
    rows, last = [], started
    for line in lines:
        now = clock()
        if not line.startswith("test ") or " ... " not in line:
            continue
        name, outcome = line[len("test "):].rstrip("\n").rsplit(" ... ", 1)
        rows.append({"name": name, "outcome": outcome.strip(), "seconds": round(now - last, 4)})
        last = now
    return rows


def run_groups(executable: Path, cwd: Path, depth: int, filters: list[str]) -> dict:
    groups: dict[str, list[str]] = defaultdict(list)
    for name in list_tests(executable, cwd, filters):
        groups[group_of(name, depth)].append(name)
    tests, failed = [], 0
    for at, (group, names) in enumerate(sorted(groups.items()), 1):
        print(f"group {at}/{len(groups)}: {group} ({len(names)} tests)", file=sys.stderr)
        command = [str(executable), "--test-threads", "1", "--exact", *names]
        started = time.monotonic()
        with subprocess.Popen(command, cwd=cwd, env=run_environment(cwd, False), stdout=subprocess.PIPE,
                              text=True, bufsize=1) as child:
            tests += read_serial_lines(child.stdout, time.monotonic, started)
        failed += child.returncode != 0
    return {"tests": tests, "suite": {"groups": len(groups), "groups_failed": failed}, "exit": int(failed > 0)}


def measure(args: argparse.Namespace) -> Path:
    # Read once, before anything runs: a commit made while the suite runs is not the tree that was built.
    commit = commit_label(args.checkout)
    out = result_path(args, commit)
    if out.exists() and not args.again:
        print(f"kept: {out} (--again measures anew)", file=sys.stderr)
        return out
    load_before = load_average()
    if args.cold:
        clean(args)
    executable, cwd, build_seconds = build(args)
    started = time.monotonic()
    if args.mode == "json":
        ran = run_json(executable, cwd, args.threads, args.filters)
    else:
        ran = run_groups(executable, cwd, args.depth, args.filters)
    run_seconds = time.monotonic() - started
    result = {
        "package": args.package, "target": " ".join(target_args(args)), "commit": commit,
        "cold": args.cold,
        "mode": args.mode, "threads": args.threads or os.cpu_count(), "jobs": args.jobs,
        "filters": args.filters, "measured_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "load_before": load_before, "load_after": load_average(), "power": power(),
        "debug": debug_setting(args.checkout),
        "condition": {key: value for key, value in sorted(os.environ.items()) if key.startswith(CONDITION_PREFIXES)},
        "build_seconds": round(build_seconds, 1), "run_seconds": round(run_seconds, 1),
        "clippy": clippy(args) if args.clippy else None,
        **ran,
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, ensure_ascii=False, indent=1) + "\n")
    print(f"wrote: {out}", file=sys.stderr)
    return out


def timed(result: dict) -> list[dict]:
    return sorted((row for row in result["tests"] if "seconds" in row), key=lambda row: -row["seconds"])


def summary(result: dict) -> dict:
    rows = timed(result)
    total = sum(row["seconds"] for row in rows)
    outcomes: dict[str, int] = defaultdict(int)
    for row in result["tests"]:
        outcomes[row.get("outcome", "unfinished")] += 1
    return {"tests": len(result["tests"]), "timed": len(rows), "sum": total, "outcomes": dict(outcomes),
            "build": result.get("build_seconds"), "run": result.get("run_seconds"),
            "clippy": (result.get("clippy") or {}).get("seconds"),
            "parallel": total / result["run_seconds"] if result.get("run_seconds") else None}


def table(result: dict, top: int, depth: int) -> str:
    rows, facts = timed(result), summary(result)
    total = facts["sum"] or 1.0
    lines = [
        f"{result['package']} {result['target']} at {result['commit']} ({result['mode']}, {result['threads']} threads,"
        f" load {result['load_before']} → {result['load_after']}, power {result.get('power')},"
        f" debug {result.get('debug')})",
        f"build {facts['build']} s{' (cold)' if result.get('cold') else ''} · run {facts['run']} s"
        + (f" · clippy {facts['clippy']} s (exit {result['clippy']['exit']})" if facts["clippy"] is not None else "")
        + f" · {facts['tests']} tests "
        f"({', '.join(f'{k} {v}' for k, v in sorted(facts['outcomes'].items()))}) · sum of test seconds {facts['sum']:.1f} s"
        + (f" · {facts['parallel']:.1f} tests at once on average" if facts["parallel"] else ""),
        "",
        "| # | seconds | % of sum | cumulative % | started at s | test |",
        "|---:|---:|---:|---:|---:|---|",
    ]
    running = 0.0
    for rank, row in enumerate(rows[:top], 1):
        running += row["seconds"]
        lines.append(f"| {rank} | {row['seconds']:.2f} | {100 * row['seconds'] / total:.1f} | "
                     f"{100 * running / total:.1f} | {row.get('started', '')} | `{row['name']}` |")
    lines.append(f"\ntop {min(top, len(rows))}: {running:.1f} s, {100 * running / total:.1f} % of the sum")
    modules: dict[str, list[float]] = defaultdict(list)
    for row in rows:
        modules[group_of(row["name"], depth)].append(row["seconds"])
    lines += ["", f"| module (first {depth} segments) | tests | seconds | % of sum |", "|---|---:|---:|---:|"]
    for module, seconds in sorted(modules.items(), key=lambda item: -sum(item[1]))[:top]:
        lines.append(f"| `{module}` | {len(seconds)} | {sum(seconds):.1f} | {100 * sum(seconds) / total:.1f} |")
    tail = [row for row in rows if "finished" in row and facts["run"] and row["finished"] > 0.9 * facts["run"]]
    if tail:
        lines += ["", f"still running in the last 10 % of the run ({len(tail)} tests): "
                  + ", ".join(f"`{row['name']}` {row['seconds']:.1f} s" for row in tail[:10])]
    return "\n".join(lines)


def compare(before: dict, after: dict, top: int) -> str:
    one, two = summary(before), summary(after)

    def change(a, b) -> str:
        if a is None or b is None:
            return "—"
        return f"{a:.1f} → {b:.1f} ({100 * (b - a) / a:+.1f} %)" if a else f"{a:.1f} → {b:.1f}"

    lines = [
        f"{before['commit']} → {after['commit']}",
        "| | before → after |", "|---|---|",
        f"| tests | {one['tests']} → {two['tests']} |",
        f"| outcomes | {one['outcomes']} → {two['outcomes']} |",
        f"| build s | {change(one['build'], two['build'])} |",
        f"| run s | {change(one['run'], two['run'])} |",
        f"| clippy s | {change(one['clippy'], two['clippy'])} |",
        f"| sum of test s | {change(one['sum'], two['sum'])} |",
        f"| threads · load before | {before['threads']} · {before['load_before']} → {after['threads']} · {after['load_before']} |",
        f"| power | {before.get('power')} → {after.get('power')} |",
        f"| debug | {before.get('debug')} → {after.get('debug')} |",
    ]
    if before.get("condition") != after.get("condition"):
        lines.append(f"\nthe environments differ: {before.get('condition')} → {after.get('condition')}")
    seconds_after = {row["name"]: row["seconds"] for row in timed(after)}
    moved = sorted(((row["seconds"] - seconds_after.get(row["name"], 0.0), row["name"], row["seconds"],
                     seconds_after.get(row["name"])) for row in timed(before)), reverse=True)[:top]
    lines += ["", "| saved s | before s | after s | test |", "|---:|---:|---:|---|"]
    for saved, name, was, now in moved:
        lines.append(f"| {saved:.2f} | {was:.2f} | {'gone' if now is None else f'{now:.2f}'} | `{name}` |")
    return "\n".join(lines)


def read(path: str | Path) -> dict:
    return json.loads(Path(path).read_text())


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("command", nargs="?", default="show", choices=("show", "measure", "table", "compare"))
    parser.add_argument("files", nargs="*", help="result files for table and compare")
    parser.add_argument("--package", default=DEFAULT_PACKAGE)
    target = parser.add_mutually_exclusive_group()
    target.add_argument("--bin", help="the binary target (default: the package's name)")
    target.add_argument("--test", help="an integration test target instead")
    parser.add_argument("--jobs", type=int, help="cargo -j for the build")
    parser.add_argument("--timings", action="store_true", help="cargo --timings for the build")
    parser.add_argument("--threads", type=int, help="libtest --test-threads (default: libtest's own)")
    parser.add_argument("--mode", choices=("json", "groups"), default="json")
    parser.add_argument("--depth", type=int, default=DEFAULT_DEPTH)
    parser.add_argument("--top", type=int, default=DEFAULT_TOP)
    parser.add_argument("--again", action="store_true", help="measure even when a result for this commit is kept")
    parser.add_argument("--checkout", type=Path, default=REPO,
                        help="measure this checkout (another commit, by this script) instead of the one it stands in")
    parser.add_argument("--cold", action="store_true", help="cargo clean first, so the build seconds are a cold build's")
    parser.add_argument("--clippy", action="store_true", help="time clippy --all-targets -D warnings after the run")
    parser.add_argument("--json", action="store_true", help="print the summary as JSON instead of the table")
    own, filters = split_filters(argv)
    args = parser.parse_args(own)
    args.filters = filters
    if args.command == "compare":
        if len(args.files) != 2:
            parser.error("compare takes BEFORE.json AFTER.json")
        print(compare(read(args.files[0]), read(args.files[1]), args.top))
        return 0
    if args.command == "table":
        if len(args.files) != 1:
            parser.error("table takes one RESULT.json")
        path = Path(args.files[0])
    else:
        path = result_path(args)
        if args.command == "measure" or not path.exists():
            path = measure(args)
    result = read(path)
    print(json.dumps(summary(result)) if args.json else table(result, args.top, args.depth))
    return 0 if result.get("exit", 0) == 0 else 1


def split_filters(argv: list[str]) -> tuple[list[str], list[str]]:
    """Everything after `--` is a libtest name filter, as with `cargo test`."""
    if "--" in argv:
        at = argv.index("--")
        return argv[:at], argv[at + 1:]
    return argv, []


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
