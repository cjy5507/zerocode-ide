"""A coordinator's gate on its build line, as files: what coord_receipt.py reads (t-47516).

Each job is the three files the build line's runner leaves in done/ — <stem>.job (the
header and one `entry <key> <command>` line per entry), <stem>.rc (the job's exit) and
<stem>.log (`head <sha> tree <tree>`, one `RC <key>=<rc> (<n>s)` line per entry, the
verdict) — its `RUN` and `DONE` lines in the runner's runner.log beside done/, and one
<key>.log per entry in the gate's log folder. The lines are the ones the coordinator's
gate script (mkgate.sh) and the runner (runner10.sh) write, so the reader is pinned to them.
"""

import hashlib
import json
import os
import time
from pathlib import Path

HEAD = "1" * 40
TREE = "2" * 40
RUN = "run-4275"
FIRST_JOB = 1792610090   # the build line names a job by the second it was made


def write_gate(root, label, jobs, head=HEAD, tree=TREE, run=RUN, who=None, rc=None, cwd=None):
    """JOBS: {name: [(key, command, rc), ...]} -> the .job paths, in order. RC: {name: the job's rc file}.
    WHO: one owner for every job, or {name: owner} (a gate run as two lines). CWD: the folder the jobs ran in — by default the top of the gate's checkout, which holds its .git."""
    root = Path(root)
    queue, logs, top = root / "build-queue", root / "gates" / label, root / "coord-gate"
    (queue / "done").mkdir(parents=True, exist_ok=True)
    logs.mkdir(parents=True, exist_ok=True)
    (top / ".git").mkdir(parents=True, exist_ok=True)
    cwd = Path(cwd) if cwd is not None else top
    cwd.mkdir(parents=True, exist_ok=True)
    owners = who if isinstance(who, dict) else {}
    default_who = who if isinstance(who, str) else f"coord-{label}"
    paths, ran = [], []
    for n, (name, entries) in enumerate(jobs.items()):
        who = owners.get(name, default_who)
        stem = queue / "done" / f"0-{FIRST_JOB + n}-coord-{label}-{name}"
        bad = sum(1 for _, _, code in entries if code != 0)
        body = [
            f"# who: {who}", f"# cwd: {cwd}", f"# run: {run}", f"# notify: run:{run}",
            f"# what: gate {label} on {head} — {name}", "# cap: 1800",
            "setopt pipefail", "unset CARGO_TARGET_DIR",
            '[ -z "$(git status --porcelain)" ] || { echo "STOP tree dirty before checkout"; exit 7; }',
            f'git checkout -q --detach {head} || {{ echo "STOP checkout"; exit 8; }}',
            "t=$(git rev-parse 'HEAD^{tree}'); echo \"head $(git rev-parse HEAD) tree $t\"; "
            f'[ "$t" = {tree} ] || {{ echo "STOP tree"; exit 9; }}',
            "bad=0",
            'entry() { local key=$1; shift; local s=$SECONDS; "$@" > ' + f"{logs}/$key.log"
            + " 2>&1; local rc=$?; grep -E '^test result: (FAILED|ok\\. [0-9]{3,})|passed$|PASSED|^Ran [0-9]{2,}|"
            "^FAILED|^FAIL|^error(\\[|:)|Finished|SKIPPED|^total  ' " + f"{logs}/$key.log"
            + ' | cut -c1-170 | tail -8; echo "RC $key=$rc ($((SECONDS-s))s)"; [ $rc -eq 0 ] || bad=$((bad+1)); }',
            *[f"entry {key} {command}" for key, command, _ in entries],
            f'echo "verdict: bad=$bad ({name} {label} on {head})"',
            f'if [ $bad != 0 ]; then for j in {queue}/in/0-*-coord-{label}-*.job(N); do mv "$j" {queue}/withdrawn/; done; fi',
            "exit $bad",
        ]
        log = [f"head {head} tree {tree}"]
        for key, _, code in entries:
            (logs / f"{key}.log").write_text(f"{key} ran\ntest result: ok. 120 passed; 0 failed\n")
            log += ["test result: ok. 120 passed; 0 failed", f"RC {key}={code} (3s)"]
        log.append(f"verdict: bad={bad} ({name} {label} on {head})")
        code = (rc or {}).get(name, bad)
        Path(f"{stem}.job").write_text("\n".join(body) + "\n")
        Path(f"{stem}.log").write_text("\n".join(log) + "\n")
        Path(f"{stem}.rc").write_text(f"{code}\n")
        ran += [f"RUN {stem.name} who={who} lane=1 cap=1800s 04:14:53",
                f"DONE {stem.name} rc={code} 194s lane=1 04:18:06 load=3"]
        paths.append(Path(f"{stem}.job"))
    with (queue / "runner.log").open("a") as runner:
        runner.write("".join(f"{line}\n" for line in ran))
    return paths


def ended(path, at):
    """Say the job of PATH ended at AT (epoch seconds): its rc file's time."""
    os.utime(Path(str(path)[:-len(".job")] + ".rc"), (at, at))


def sealed(paths, made=None, **facts):
    """A receipt sealed over whatever the files now hold — no reading of what they say,
    the way a careless or broken coordinator would make one."""
    seen = {}
    logs = cwd = None
    for path in paths:
        stem = str(path)[:-len(".job")]
        for kind in ("job", "rc", "log"):
            seen[f"{stem}.{kind}"] = hashlib.sha256(Path(f"{stem}.{kind}").read_bytes()).hexdigest()
        for line in Path(f"{stem}.job").read_text().splitlines():
            if line.startswith("# cwd: "):
                cwd = line[len("# cwd: "):]
            elif line.startswith("entry() "):
                logs = line.split('"$@" > ', 1)[1].split("/$key.log", 1)[0]
            elif line.startswith("entry "):
                key = line.split()[1]
                seen[f"{logs}/{key}.log"] = hashlib.sha256(Path(f"{logs}/{key}.log").read_bytes()).hexdigest()
    receipt = {"receipt": 1, "who": "coord-l9", "run": RUN, "tree": TREE, "logs": logs, "cwd": cwd,
               "made": int(made if made is not None else time.time()),
               "jobs": [str(path)[:-len(".job")] for path in paths], "sealed": seen}
    receipt.update(facts)
    return json.dumps(receipt)
