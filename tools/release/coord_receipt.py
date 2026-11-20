#!/usr/bin/env python3
"""The coordinator's gate receipt: the verify recipes its build line already ran green on one tree.

A coordinator gates a landing on the build line, `zo line`: one job a gate, put in from the
top of the checkout it gates and running the tree's own `tools/release/coord-gate.sh`. The
line keeps each job's record (`<line>/jobs/<id>.json`: owner, checkout, command, state, rc,
start and end), what the command wrote (`<line>/logs/<id>.log`) and how it ended
(`<line>/logs/<id>.rc`). Under the line's own `zo line: …` lines, the gate script's log opens
with `# who:`, `# run:`, `# cwd:` and `# logs:` and then `head <sha> tree <tree>`; it says
`entry <key> <command>` before and `RC <key>=<rc> (<n>s)` after every entry it ran (the
entry's output in `<logs>/<key>.log`), and ends with a `verdict: bad=<n>` line. When the
coordinator cuts a release from that tree the receipt goes along (`enqueue.sh <sha> --receipt
FILE`, or `--keep-only` for a lane started with the sha), and the release lane runs only the
recipes it leaves owed (t-47516: v1.1.84's lane spent 3168 + 1719 s on gates the coordinator
had run green on the same tree an hour before; t-119757: v1.1.168's 2837 + 1618 s).

    coord_receipt.py make JOB.json [JOB.json ...] > receipt.json
    coord_receipt.py read --receipt FILE --line LINE --run RUN [--dir DIR]

The receipt only locates the evidence and seals its bytes (the sha256 of every file read).
`read` reads all of it again: a job counts when its record says passed with rc 0 and names
its start and end, its rc file says 0, its command is `bash tools/release/coord-gate.sh …`
(never its inner `--in-lane-env` step) with `--who` its owner and `--logs` its log's folder,
its log's `# who:` and `# cwd:` are the owner and checkout its record names, the log opens
with a head and a tree, every entry it ran closed with a green RC line and left its own log,
and its verdict counted no red; every job names one run, tree, logs folder and checkout,
no two jobs ran one entry, and the receipt was made after the last job ended. Every job must
be one the build line LINE ran: its record sits in LINE's jobs/, its log and rc file in
LINE's logs/. The run is only what the gate script printed (`# run:`, its pane's
`zerocode-orc run-current`) and the owner what the job was put in under — nothing signs
either — so they are held to the lane's own: a coordinator's `coord-<label>` of RUN — or,
for a gate run as two jobs, both `coord-<label>-root` and `coord-<label>-zo` of one label
(printed as `who <a>+<b>`). It prints facts, one a line:

    who <owner> · run <run> · tree <tree> · logs <folder> · cwd <the checkout the jobs ran in>
    head <sha> <the earliest end, epoch seconds, of a job on that head>
    recipe <name>    with --dir: a recipe of that folder an entry ran exactly as `just <name>`

and exits 1 with one line saying why when the receipt does not stand. Every job ran at the
top of its checkout by its own log: anywhere else the gate script stops before its head and
tree, and a log without them is refused. So a bare `just <name>` is the top's recipe and
`<shell> -c 'cd <folder> && just <name>'` that folder's (from zo-ide a bare `just test` would be
zo's, never the root's, and the script never runs there), and nothing reads the checkout
again: it may be removed once its jobs ended, before `make` too. How the tree relates
to the sha being cut, and which recipes a gate runs, are the lane's to judge (git and
verify-every-recipe.sh --list); so is anything an entry ran other than a recipe. A receipt
of the old build queue's jobs (version 1, 2026-10-22 to 11-09) is refused: that queue's
runner has run nothing since 2026-11-09.
"""

import argparse
import hashlib
import json
import os
import re
import shlex
import stat
import time
from pathlib import Path

VERSION = 2
OLD_QUEUE = 1   # the old build queue's receipts (its done/ and runner.log), refused since t-119757
MAX_BYTES = 64 * 1024 * 1024   # the largest file read; 10-22's largest gate log (window-browser-test) was 1.5 MB
JOB_ID = re.compile(r"j-[A-Za-z0-9-]{1,38}")   # a zo line job's id, as zo reads one (zerocode-core line.rs is_job_id)
LINE_SAYS = "zo line: "   # what the line itself writes into a job's log ahead of the command's output
GATE_SCRIPT = ["bash", "tools/release/coord-gate.sh"]   # a job's command: the gated checkout's own gate script
INNER_STEP = "--in-lane-env"   # coord-gate.sh's own second step, past lane-env.sh and run-current: never a job's command
GATE_FLAGS = ("--who", "--logs", "--calm-wait")   # the flags coord-gate.sh reads, each with a value
HEADER = re.compile(r"# ([a-z]+): ?(.*)")
HEADS = ("who", "run", "cwd", "logs")   # the gate script's own words at the head of its log
OPENING = re.compile(r"head ([0-9a-f]{40}|[0-9a-f]{64}) tree ([0-9a-f]{40}|[0-9a-f]{64})")
CLOSING = re.compile(r"RC ([^\s=]+)=(\S+) \(\d+s\)")
VERDICT = re.compile(r"verdict: bad=(\d+) \(.*")
IN_FOLDER = re.compile(r"cd (\S+) && just ([A-Za-z0-9_][A-Za-z0-9_-]*)")
RECIPE = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_-]*")
SHELLS = {"zsh", "bash", "sh"}
FACTS = ("run", "tree", "logs", "cwd")   # what every job of a receipt names alike (its owner may be one of two, see COORDINATOR)
COORDINATOR = re.compile(r"coord-([A-Za-z0-9]+)(?:-(root|zo))?")   # the coordinator's own jobs: coord-<the landing's label>, or
                                                                    # coord-<label>-root and coord-<label>-zo for a gate run as two jobs


class Refused(Exception):
    """The receipt does not stand; the message says why on one line."""


def plain_bytes(path):
    """A regular file's bytes; a missing, linked or oversized file is refused."""
    try:
        info = os.lstat(path)
    except OSError:
        raise Refused(f"{path} is missing") from None
    if not stat.S_ISREG(info.st_mode):
        raise Refused(f"{path} is not a plain file")
    if info.st_size > MAX_BYTES:
        raise Refused(f"{path} is larger than {MAX_BYTES} bytes")
    return Path(path).read_bytes()


def whole(value):
    """VALUE is a JSON integer (a bool is not one)."""
    return isinstance(value, int) and not isinstance(value, bool)


def record_of(path, seen):
    """The job record at PATH, as zo line writes one; anything else is refused."""
    data = plain_bytes(path)
    seen[path] = hashlib.sha256(data).hexdigest()
    try:
        record = json.loads(data)
    except ValueError:
        record = None
    name = os.path.basename(path)[:-len(".json")] if path.endswith(".json") else ""
    if not isinstance(record, dict) or not JOB_ID.fullmatch(name) or record.get("id") != name:
        raise Refused(f"{path} is not a zo line job's record")
    owner, checkout, command = record.get("owner"), record.get("checkout"), record.get("command")
    if not isinstance(owner, str) or not owner or not isinstance(checkout, str) or not checkout:
        raise Refused(f"job {name}'s record names no owner or checkout")
    if record.get("state") != "passed":
        raise Refused(f"job {name} ended {record.get('state')}")
    if not whole(record.get("rc")) or record["rc"] != 0:
        raise Refused(f"job {name} ended rc={record.get('rc')}")
    started, ended = record.get("started_ms"), record.get("ended_ms")
    if not whole(started) or not whole(ended) or ended < started:
        raise Refused(f"job {name}'s record has no start and end")
    if not isinstance(command, list) or not all(isinstance(word, str) for word in command) \
            or command[:len(GATE_SCRIPT)] != GATE_SCRIPT:
        said = shlex.join(command) if isinstance(command, list) and all(isinstance(w, str) for w in command) else "no command"
        raise Refused(f"job {name} ran {said}, not {shlex.join(GATE_SCRIPT)}")
    told = gate_flags(name, command[len(GATE_SCRIPT):])
    if told.get("--who") != owner:
        raise Refused(f"job {name} ran coord-gate.sh --who {told.get('--who')}, its record's owner {owner}")
    return name, record, told


def gate_flags(name, words):
    """The flags a job's command gave coord-gate.sh, {flag: value}. Its inner step is refused: that step
    skips lane-env.sh (the lane's environment) and run-current (the run the log names), and takes them
    from whoever called it (ZO's review of t-119757, F2)."""
    if INNER_STEP in words:
        raise Refused(f"job {name} ran coord-gate.sh {INNER_STEP}, its inner step past lane-env.sh and run-current")
    told, at = {}, 0
    while at < len(words):
        if words[at] in GATE_FLAGS:
            if words[at] in told or at + 1 >= len(words):
                raise Refused(f"job {name}'s command gives {words[at]} twice or no value")
            told[words[at]] = words[at + 1]
            at += 2
        else:
            at += 1
    return told


def job(path):
    """One `zo line` job, read: owner, run, checkout, head, tree, end, entries, logs folder and the bytes that showed them."""
    path, seen = os.path.abspath(path), {}
    name, record, told = record_of(path, seen)
    logs_of_line = os.path.join(os.path.dirname(os.path.dirname(path)), "logs")
    text = {}
    for kind in ("rc", "log"):
        where = os.path.join(logs_of_line, f"{name}.{kind}")
        data = plain_bytes(where)
        seen[where] = hashlib.sha256(data).hexdigest()
        text[kind] = data.decode("utf-8", errors="replace")
    rc = text["rc"].strip()
    if rc != "0":
        raise Refused(f"job {name} ended rc={rc or 'nothing'}")
    log = text["log"].splitlines()
    at = 0
    while at < len(log) and log[at].startswith(LINE_SAYS):   # only above the script's first line: the line's own words
        at += 1
    header = {}
    while at < len(log) and HEADER.fullmatch(log[at]):
        found = HEADER.fullmatch(log[at])
        if found[1] in header:
            raise Refused(f"job {name}'s log says # {found[1]}: twice")
        header[found[1]] = found[2].strip()
        at += 1
    if not all(header.get(key) for key in HEADS):
        raise Refused(f"job {name}'s log names no owner, run, folder or logs folder")
    opening = OPENING.fullmatch(log[at]) if at < len(log) else None
    if not opening:
        raise Refused(f"job {name}'s log does not open with its head and tree")
    if header["who"] != record["owner"]:
        raise Refused(f"job {name}'s log says who {header['who']}, its record {record['owner']}")
    if os.path.realpath(header["cwd"]) != os.path.realpath(record["checkout"]):
        raise Refused(f"job {name}'s log says cwd {header['cwd']}, its record ran it in {record['checkout']}")
    if os.path.realpath(told.get("--logs") or "") != os.path.realpath(header["logs"]) or not told.get("--logs"):
        raise Refused(f"job {name} ran coord-gate.sh --logs {told.get('--logs')}, its log says # logs: {header['logs']}")
    entries, closed, verdicts = {}, {}, []
    for line in log[at + 1:]:
        if line.startswith("entry "):
            try:
                words = shlex.split(line)
            except ValueError:
                raise Refused(f"job {name} has an entry the shell cannot read") from None
            if len(words) < 3 or words[1] in entries:
                raise Refused(f"job {name} has an entry without a command, or one key twice")
            entries[words[1]] = words[2:]
        elif CLOSING.fullmatch(line):
            found = CLOSING.fullmatch(line)
            if found[1] in closed:
                raise Refused(f"job {name} closed entry {found[1]} twice")
            closed[found[1]] = found[2]
        elif line.startswith("verdict: "):
            verdicts.append(line)
    if not entries:
        raise Refused(f"job {name} ran no entry")
    if set(closed) != set(entries):
        missing = sorted(set(entries) ^ set(closed))
        raise Refused(f"job {name}'s entries and RC lines differ at {missing[0]}")
    red = sorted(key for key, value in closed.items() if value != "0")
    if red:
        raise Refused(f"job {name} entry {red[0]} rc={closed[red[0]]}")
    verdict = VERDICT.fullmatch(verdicts[0]) if len(verdicts) == 1 else None
    if not verdict or verdict[1] != "0":
        raise Refused(f"job {name}'s verdict is not bad=0")
    folder = header["logs"]
    for key in entries:
        where = os.path.join(folder, f"{key}.log")
        seen[where] = hashlib.sha256(plain_bytes(where)).hexdigest()
    return {"name": name, "path": path, "who": record["owner"], "run": header["run"], "cwd": record["checkout"],
            "head": opening[1], "tree": opening[2], "ended": record["ended_ms"] / 1000, "entries": entries,
            "logs": folder, "seen": seen}


def split_gate(owners):
    """OWNERS are exactly the two jobs of one gate, coord-<label>-root and coord-<label>-zo."""
    found = [COORDINATOR.fullmatch(who) for who in owners]
    return len(found) == 2 and all(found) and {one[2] for one in found} == {"root", "zo"} and found[0][1] == found[1][1]


def agreed(jobs):
    """The owner, run, tree, logs folder and checkout that every job names; refused when they differ."""
    if not jobs:
        raise Refused("the receipt names no job")
    if len({one["name"] for one in jobs}) != len(jobs):
        raise Refused("the receipt names a job twice")
    for key in FACTS:
        values = sorted({one[key] for one in jobs})
        if len(values) != 1:
            raise Refused(f"the jobs name more than one {key}: {' '.join(values)}")
    ran = {}
    for one in jobs:   # one logs folder for all: an entry two jobs ran left one log, the later's
        for key in one["entries"]:
            if key in ran:
                raise Refused(f"jobs {ran[key]} and {one['name']} both ran entry {key}")
            ran[key] = one["name"]
    for one in jobs:   # a job's own owner is one name: a "+" would make the joined owners look like a pair
        if "+" in one["who"]:
            raise Refused(f"job {one['name']}'s owner {one['who']} joins owners with +")
    owners = sorted({one["who"] for one in jobs})
    for who in owners:   # every raw owner is a coordinator's name before the owners are counted or paired: one worker owning any job is no receipt
        if not COORDINATOR.fullmatch(who):
            raise Refused(f"the jobs say {who}, not a coordinator's coord-<label>")
    if len(owners) > 1 and not split_gate(owners):
        raise Refused(f"the jobs name more than one who: {' '.join(owners)}")
    return {"who": "+".join(owners), **{key: jobs[0][key] for key in FACTS}}


def on_the_line(jobs, line):
    """Every job is one the build line LINE ran: its record sits in LINE's jobs/ (its log and rc file beside it in logs/)."""
    kept = os.path.realpath(os.path.join(line, "jobs"))
    for one in jobs:
        if os.path.realpath(os.path.dirname(one["path"])) != kept:
            raise Refused(f"job {one['name']} is not in {kept}")


def held_to(facts, run):
    """The jobs say they are a coordinator's (coord-<label>, or its -root and -zo jobs) of RUN, the lane's own run."""
    if not run:
        raise Refused("the lane was given no coordinator run (RELEASE_COORD_RUN) to hold the jobs to")
    if facts["run"] != run:
        raise Refused(f"the jobs say run {facts['run']}, not {run}")
    if not all(COORDINATOR.fullmatch(who) for who in facts["who"].split("+")):
        raise Refused(f"the jobs say {facts['who']}, not a coordinator's coord-<label>")


def recipe_of(command, folder):
    """The recipe an entry run from the checkout's top ran in FOLDER when it ran exactly `just <recipe>` there, else None."""
    folder = os.path.normpath(folder)
    if folder == "." and len(command) == 2 and command[0] == "just" and RECIPE.fullmatch(command[1]):
        return command[1]
    if len(command) == 3 and command[0] in SHELLS and command[1] == "-c":
        found = IN_FOLDER.fullmatch(command[2].strip())
        if found and os.path.normpath(found[1]) == folder:
            return found[2]
    return None


def make(paths):
    jobs = [job(path) for path in paths]
    facts = agreed(jobs)
    seen = {}
    for one in jobs:
        seen.update(one["seen"])
    return {"receipt": VERSION, **facts, "made": int(time.time()), "jobs": [one["path"] for one in jobs], "sealed": seen}


def read(path, folder=None, line=None, run=None):
    """What a receipt stands for. LINE and RUN — the build line its jobs ran on and the run they
    must say — are the lane's, and the CLI asks for both; without them only the evidence is read."""
    data = plain_bytes(path)
    try:
        receipt = json.loads(data)
    except ValueError:
        raise Refused("the receipt is not JSON") from None
    if isinstance(receipt, dict) and receipt.get("receipt") == OLD_QUEUE:
        raise Refused("the receipt is a version 1 receipt of the old build queue, which has run nothing since "
                      "2026-11-09 — make one of the zo line jobs (coord_receipt.py make <line>/jobs/<id>.json)")
    if not isinstance(receipt, dict) or receipt.get("receipt") != VERSION:
        raise Refused(f"the receipt is not a version {VERSION} receipt")
    paths, sealed, made = receipt.get("jobs"), receipt.get("sealed"), receipt.get("made")
    if not isinstance(paths, list) or not all(isinstance(one, str) for one in paths) \
            or not isinstance(sealed, dict) or not whole(made):
        raise Refused("the receipt has no jobs, seal or time")
    jobs = [job(one) for one in paths]
    facts = agreed(jobs)
    for key, value in facts.items():
        if receipt.get(key) != value:
            raise Refused(f"the receipt's {key} is not its jobs'")
    seen = {}
    for one in jobs:
        seen.update(one["seen"])
    changed = sorted(name for name in set(seen) | set(sealed) if seen.get(name) != sealed.get(name))
    if changed:
        raise Refused(f"{changed[0]} is not what the receipt sealed")
    if made < int(max(one["ended"] for one in jobs)):
        raise Refused("the receipt was made before its last job ended")
    # The jobs ran at the top of their checkout by their own logs (job: no head and tree, no job), so the
    # checkout is not read, nor any word of the receipt on it: the stack is cleaned as soon as it lands,
    # before make as often as after (the review of t-119757's fix, R1).
    if line is not None:
        on_the_line(jobs, line)
    if run is not None:
        held_to(facts, run)
    heads = {}
    for one in jobs:
        heads[one["head"]] = min(heads.get(one["head"], one["ended"]), one["ended"])
    said = [f"{key} {value}" for key, value in facts.items()]
    said += [f"head {head} {int(ended)}" for head, ended in sorted(heads.items())]
    if folder is not None:
        recipes ={recipe_of(command, folder) for one in jobs for command in one["entries"].values()}
        said += [f"recipe {name}" for name in sorted(recipes - {None})]
    return said


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    actions = parser.add_subparsers(dest="action", required=True)
    making = actions.add_parser("make", help="seal a gate's zo line jobs into a receipt (JSON on stdout)")
    making.add_argument("jobs", nargs="+", help="the jobs' records, <line>/jobs/<id>.json; their .log and .rc sit in <line>/logs")
    reading = actions.add_parser("read", help="read a receipt's evidence again and print what it stands for")
    reading.add_argument("--receipt", required=True)
    reading.add_argument("--line", required=True, help="the build line's folder (zo line's, <zo home>/line): every record sits in its jobs/")
    reading.add_argument("--run", required=True, help="the run every job must say it is of (the lane's RELEASE_COORD_RUN)")
    reading.add_argument("--dir", help="a gate's folder in the checkout (. or zo-ide): also print the recipes it covers")
    args = parser.parse_args(argv)
    try:
        if args.action == "make":
            print(json.dumps(make(args.jobs), indent=1, sort_keys=True, ensure_ascii=False))
        else:
            print("\n".join(read(args.receipt, args.dir, args.line, args.run)))
    except Refused as why:
        print(" ".join(str(why).split()))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
