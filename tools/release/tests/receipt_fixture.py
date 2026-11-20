"""A coordinator's gate on the build line, `zo line`, as files: what coord_receipt.py reads (t-47516, t-119757).

Each job is what the line keeps of it — its record <line>/jobs/<id>.json (owner, checkout,
command, state, rc, start and end, as zo writes one), what its command wrote
<line>/logs/<id>.log and how it ended <line>/logs/<id>.rc — and one <key>.log per entry in
the gate's log folder. The log is the line's own `zo line: …` line, then what
tools/release/coord-gate.sh prints: its `# who:` · `# run:` · `# cwd:` · `# logs:`, `head <sha>
tree <tree>`, `entry <key> <command>` and `RC <key>=<rc> (<n>s)` for each entry, and the
verdict — so the reader is pinned to the lines the line and the script write.
"""

import hashlib
import json
import os
import shlex
import time
from pathlib import Path

HEAD = "1" * 40
TREE = "2" * 40
RUN = "run-1001"
FIRST_JOB = 1792610090   # a job's id carries the moment it was put in; here a second, one more a job


def job_id(label, name, n=0):
    """The id write_gate gives the Nth job (from 0) named NAME of gate LABEL."""
    return f"j-{FIRST_JOB + n}-{label}-{name}"


def line_of(root):
    return Path(root) / "line"


def write_gate(root, label, jobs, head=HEAD, tree=TREE, run=RUN, who=None, rc=None, cwd=None, state=None, said=None):
    """JOBS: {name: [(key, command, rc), ...]} -> the records' paths, in order. RC: {name: the rc the
    line recorded} (by default the count of red entries). STATE: {name: how the line says it ended}
    (by default passed when that rc is 0, else failed). WHO: one owner for every job, or {name:
    owner} (a gate run as two jobs). CWD: the checkout the jobs ran in — by default the top of the
    gate's checkout, which holds its .git. SAID: {key: what that entry printed}, by default a green
    test run's two lines."""
    root = Path(root)
    line, logs, top = line_of(root), root / "gates" / label, root / "coord-gate"
    for folder in (line / "jobs", line / "logs", logs, top / ".git"):
        folder.mkdir(parents=True, exist_ok=True)
    cwd = Path(cwd) if cwd is not None else top
    cwd.mkdir(parents=True, exist_ok=True)
    owners = who if isinstance(who, dict) else {}
    default_who = who if isinstance(who, str) else f"coord-{label}"
    now = int(time.time())
    paths = []
    for n, (name, entries) in enumerate(jobs.items()):
        who = owners.get(name, default_who)
        ident = job_id(label, name, n)
        bad = sum(1 for _, _, code in entries if code != 0)
        code = (rc or {}).get(name, bad)
        ended = (state or {}).get(name, "passed" if code == 0 else "failed")
        log = [f"zo line: cargo builds in {root / 'slots' / '0'} — the workspace crates of {cwd / 'Cargo.toml'} removed from it first",
               f"# who: {who}", f"# run: {run}", f"# cwd: {cwd}", f"# logs: {logs}", f"head {head} tree {tree}"]
        for key, command, entry_rc in entries:
            (logs / f"{key}.log").write_text((said or {}).get(key, f"{key} ran\ntest result: ok. 120 passed; 0 failed\n"))
            log += [f"entry {key} {command}", f"RC {key}={entry_rc} (3s)"]
        log.append(f"verdict: bad={bad} ({name} {label} on {head[:9]})")
        record = {"id": ident, "project": str(top / ".git"), "checkout": str(cwd), "owner": who,
                  "what": f"gate {label} on {head} — {name}", "class": "first", "quiet": True, "cap_secs": 1800,
                  "command": ["bash", "tools/release/coord-gate.sh", "--who", who, "--logs", str(logs), "root"],
                  "state": ended, "queued_ms": (now - 200) * 1000, "started_ms": (now - 194) * 1000,
                  "ended_ms": now * 1000, "rc": code, "slot": 0, "pid": 4242}
        path = line / "jobs" / f"{ident}.json"
        path.write_text(json.dumps(record, indent=2, ensure_ascii=False) + "\n")
        (line / "logs" / f"{ident}.log").write_text("\n".join(log) + "\n")
        # The rc file says what `zo line wait` says (line_cli.rs code_of): 0 for a pass, else the code or 1.
        (line / "logs" / f"{ident}.rc").write_text(f"{0 if ended == 'passed' else (code or 1)}\n")
        paths.append(path)
    return paths


def cargo_failures(names, recipe="test"):
    """What `just <recipe>` prints when cargo's tests NAMES failed: their stdout, cargo's failures block
    with each name four spaces in — what the lane's failed_tests reads — and just's own last word."""
    lines = [f"running {len(names) + 2} tests"] + [f"test {name} ... FAILED" for name in names] + ["", "failures:", ""]
    for name in names:
        lines += [f"---- {name} stdout ----", f"thread '{name}' panicked at tests/x.rs:1:1:", "the child never said it ran", ""]
    lines += ["", "failures:"] + [f"    {name}" for name in names]
    lines += ["", f"test result: FAILED. 2 passed; {len(names)} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s",
              "", "error: test failed, to rerun pass `-p x --test y`", f"error: recipe `{recipe}` failed on line 32 with exit code 101"]
    return "\n".join(lines) + "\n"


def unnamed_failure(recipe):
    """What `just <recipe>` prints when the recipe failed before any test spoke (a compile error)."""
    return f"error[E0425]: cannot find value `x` in this scope\nerror: recipe `{recipe}` failed on line 41 with exit code 101\n"


def judged(path, runs, bad=0, rc=0):
    """Say in the log of the job at PATH that the lane's flakes judgment ran RUNS — (lane, unit, run, of, rc)
    each — as solo_judge says them (lane.sh --judge-flakes), just before its verdict, each run's output where
    run_solo puts it (<logs>/solo.<lane>.<at>.<run>.log, AT the unit's place among its gate's units); the
    verdict then counts BAD and the line's record and rc file say RC — by default the green job coord-gate.sh
    leaves when the judgment cleared every red entry."""
    log = log_of(path)
    lines = log.read_text().splitlines()
    logs = next(line[len("# logs: "):] for line in lines if line.startswith("# logs: "))
    at = next(n for n, line in enumerate(lines) if line.startswith("verdict: "))
    places, said = {}, []
    for lane, unit, run, of, code in runs:
        units = places.setdefault(lane, [])
        if unit not in units:
            units.append(unit)
        said.append(f"17:0{run % 10}:00 solo {lane} {unit} {run}/{of} rc={code}")
        Path(logs, f"solo.{lane}.{units.index(unit) + 1}.{run}.log").write_text(
            f"test {unit} ... {'ok' if code == '0' else 'FAILED'}\n")
    verdict = lines[at].split(" ", 2)
    lines[at:at + 1] = said + [f"verdict: bad={bad} {verdict[2]}"]
    log.write_text("\n".join(lines) + "\n")
    rewrite(path, state="passed" if rc == 0 else "failed", rc=rc)
    rc_of(path).write_text(f"{rc}\n")


def rewrite(path, **fields):
    """Change fields of the record at PATH, as zo would write it."""
    record = json.loads(Path(path).read_text())
    record.update(fields)
    Path(path).write_text(json.dumps(record, indent=2, ensure_ascii=False) + "\n")


def ended(path, at):
    """Say the job of PATH ended at AT (epoch seconds): its record's end, its start 194 s before."""
    rewrite(path, started_ms=int((at - 194) * 1000), ended_ms=int(at * 1000))


def stopped_below_the_top(path):
    """Make the log of the job at PATH what coord-gate.sh writes when put in below its checkout's top:
    its `#` lines, then its STOP line, with no head and tree and nothing after. The record is left as
    it was (green), so what refuses the job is the log alone."""
    log = log_of(path)
    lines = log.read_text().splitlines()
    at = next(n for n, line in enumerate(lines) if line.startswith("head "))
    cwd = next(line[len("# cwd: "):] for line in lines if line.startswith("# cwd: "))
    lines[at:] = [f"STOP the job runs in {cwd}, not the top of its checkout ({Path(path).parents[2] / 'coord-gate'})"]
    log.write_text("\n".join(lines) + "\n")


def log_of(path):
    """The log the line keeps for the job whose record is at PATH."""
    path = Path(path)
    return path.parent.parent / "logs" / f"{path.stem}.log"


def rc_of(path):
    path = Path(path)
    return path.parent.parent / "logs" / f"{path.stem}.rc"


def sealed(paths, made=None, **facts):
    """A receipt sealed over whatever the files now hold — no reading of what they say,
    the way a careless or broken coordinator would make one."""
    seen = {}
    logs = cwd = None
    for path in paths:
        path = Path(path)
        for one in (path, log_of(path), rc_of(path)):
            seen[str(one)] = hashlib.sha256(one.read_bytes()).hexdigest()
        cwd = json.loads(path.read_text()).get("checkout", cwd)
        for line in log_of(path).read_text().splitlines():
            if line.startswith("# logs: "):
                logs = line[len("# logs: "):]
            elif line.startswith("entry "):
                key = shlex.split(line)[1]
                entry = os.path.join(logs, f"{key}.log")   # as the reader names it, on every system
                seen[entry] = hashlib.sha256(Path(entry).read_bytes()).hexdigest()
    receipt = {"receipt": 2, "who": "coord-l9", "run": RUN, "tree": TREE, "logs": logs, "cwd": cwd,
               "made": int(made if made is not None else time.time()),
               "jobs": [str(path) for path in paths], "sealed": seen}
    receipt.update(facts)
    return json.dumps(receipt)
