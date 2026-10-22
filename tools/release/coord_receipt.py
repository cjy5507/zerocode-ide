#!/usr/bin/env python3
"""The coordinator's gate receipt: the verify recipes its build line already ran green on one tree.

A coordinator gates a landing on its build line: one zsh job per group of recipes, each
job's log opening with `head <sha> tree <tree>`, one `RC <key>=<rc> (<n>s)` line for every
`entry <key> <command>` the job ran, each entry's output in `<folder>/<key>.log`, and a
`verdict: bad=<n>` line. When the coordinator cuts a release from that tree the receipt
goes along (`enqueue.sh <sha> --receipt FILE`), and the release lane runs only the recipes
it leaves owed (t-47516: v1.1.84's lane spent 3168 + 1719 s on gates the coordinator had
run green on the same tree an hour before).

    coord_receipt.py make JOB.job [JOB.job ...] > receipt.json
    coord_receipt.py read --receipt FILE --line QUEUE --run RUN [--dir DIR]

The receipt only locates the evidence and seals its bytes (the sha256 of every file read).
`read` reads all of it again: a job counts when its rc file says 0, its log opened with a
head and a tree, every entry it ran closed with a green RC line and left its own log, and
its verdict counted no red; every job carries the receipt's owner and run and names one
tree and one folder it ran in, and the receipt was made after the last job ended. Every job
must be one the build line QUEUE ran: its files sit in QUEUE's done/, and the runner's log
there started it under its owner and last ended it with rc 0. Owner and run are only what
the jobs say of themselves (`# who:`, `# run:`) — nothing signs them — so they are held to
the lane's own: a coordinator's `coord-<label>` of RUN. It prints facts, one a line:

    who <owner> · run <run> · tree <tree> · logs <folder> · cwd <the folder the jobs ran in>
    head <sha> <the earliest end, epoch seconds, of a job on that head>
    recipe <name>    with --dir: a recipe of that folder an entry ran exactly as `just <name>`

and exits 1 with one line saying why when the receipt does not stand. An entry's `just` is
placed only when the jobs ran at the top of their checkout (their folder holds its .git): a
bare `just <name>` is then the top's recipe, and `<shell> -c 'cd <folder> && just <name>'`
that folder's — from zo-ide a bare `just test` is zo's, never the root's. How the tree
relates to the sha being cut, and which recipes a gate runs, are the lane's to judge
(git and verify-every-recipe.sh --list); so is anything an entry ran other than a recipe.
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

VERSION = 1
MAX_BYTES = 64 * 1024 * 1024   # the largest file read; 10-22's largest gate log (window-browser-test) was 1.5 MB
HEADER = re.compile(r"# ([a-z]+): ?(.*)")
OPENING = re.compile(r"head ([0-9a-f]{40}|[0-9a-f]{64}) tree ([0-9a-f]{40}|[0-9a-f]{64})")
CLOSING = re.compile(r"RC ([^\s=]+)=(\S+) \(\d+s\)")
VERDICT = re.compile(r"verdict: bad=(\d+) \(.*")
LOGGED = re.compile(r'"\$@" > (\S+)/\$key\.log ')     # where `entry()` sends each entry's output
IN_FOLDER = re.compile(r"cd (\S+) && just ([A-Za-z0-9_][A-Za-z0-9_-]*)")
RECIPE = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_-]*")
SHELLS = {"zsh", "bash", "sh"}
FACTS = ("who", "run", "tree", "logs", "cwd")   # what every job of a receipt names alike
COORDINATOR = re.compile(r"coord-[A-Za-z0-9]+")   # the coordinator's own jobs: coord-<the landing's label>
STARTED = re.compile(r"RUN (\S+) who=(\S*) .*")    # the runner's log: RUN <name> who=<who> lane=<n> cap=<n>s <time>
ENDED = re.compile(r"DONE (\S+) rc=(\S+) .*")      # …and DONE <name> rc=<rc> <n>s lane=<n> <time> load=<n>


class Refused(Exception):
    """The receipt does not stand; the message says why on one line."""


def plain_bytes(path):
    """A regular file's bytes and its modification time; a missing, linked or oversized file is refused."""
    try:
        info = os.lstat(path)
    except OSError:
        raise Refused(f"{path} is missing") from None
    if not stat.S_ISREG(info.st_mode):
        raise Refused(f"{path} is not a plain file")
    if info.st_size > MAX_BYTES:
        raise Refused(f"{path} is larger than {MAX_BYTES} bytes")
    return Path(path).read_bytes(), info.st_mtime


def job_stem(path):
    path = os.path.abspath(path)
    return path[:-len(".job")] if path.endswith(".job") else path


def job(stem):
    """One build-line job, read: owner, run, folder, head, tree, end, entries, logs folder and the bytes that showed them."""
    name, seen, text = os.path.basename(stem), {}, {}
    for kind in ("job", "rc", "log"):
        data, mtime = plain_bytes(f"{stem}.{kind}")
        seen[f"{stem}.{kind}"] = hashlib.sha256(data).hexdigest()
        text[kind] = data.decode("utf-8", errors="replace")
        if kind == "rc":
            ended = mtime
    rc = text["rc"].strip()
    if rc != "0":
        raise Refused(f"job {name} ended rc={rc or 'nothing'}")
    header, entries, logs = {}, {}, set()
    lines = text["job"].splitlines()
    for line in lines:
        found = HEADER.fullmatch(line)
        if not found:
            break
        header.setdefault(found[1], found[2].strip())
    for line in lines:
        if line.startswith("entry() "):
            logs.update(LOGGED.findall(line))
        elif line.startswith("entry "):
            try:
                words = shlex.split(line)
            except ValueError:
                raise Refused(f"job {name} has an entry the shell cannot read") from None
            if len(words) < 3 or words[1] in entries:
                raise Refused(f"job {name} has an entry without a command, or one key twice")
            entries[words[1]] = words[2:]
    if not header.get("who") or not header.get("run") or not header.get("cwd"):
        raise Refused(f"job {name} names no owner, run or folder")
    if not entries:
        raise Refused(f"job {name} ran no entry")
    if len(logs) != 1:
        raise Refused(f"job {name} does not say where its entries wrote their logs")
    log = text["log"].splitlines()
    opening = OPENING.fullmatch(log[0]) if log else None
    if not opening:
        raise Refused(f"job {name}'s log does not open with its head and tree")
    closed = {}
    for line in log:
        found = CLOSING.fullmatch(line)
        if found:
            if found[1] in closed:
                raise Refused(f"job {name} closed entry {found[1]} twice")
            closed[found[1]] = found[2]
    if set(closed) != set(entries):
        missing = sorted(set(entries) ^ set(closed))
        raise Refused(f"job {name}'s entries and RC lines differ at {missing[0]}")
    red = sorted(key for key, value in closed.items() if value != "0")
    if red:
        raise Refused(f"job {name} entry {red[0]} rc={closed[red[0]]}")
    verdicts = [line for line in log if line.startswith("verdict: ")]
    verdict = VERDICT.fullmatch(verdicts[0]) if len(verdicts) == 1 else None
    if not verdict or verdict[1] != "0":
        raise Refused(f"job {name}'s verdict is not bad=0")
    folder = logs.pop()
    for key in entries:
        data, _ = plain_bytes(os.path.join(folder, f"{key}.log"))
        seen[os.path.join(folder, f"{key}.log")] = hashlib.sha256(data).hexdigest()
    return {"name": name, "stem": stem, "who": header["who"], "run": header["run"], "cwd": header["cwd"],
            "head": opening[1], "tree": opening[2], "ended": ended, "entries": entries, "logs": folder, "seen": seen}


def agreed(jobs):
    """The owner, run, tree, logs folder and folder run in that every job names; refused when they differ."""
    if not jobs:
        raise Refused("the receipt names no job")
    if len({one["name"] for one in jobs}) != len(jobs):
        raise Refused("the receipt names a job twice")
    for key in FACTS:
        values = sorted({one[key] for one in jobs})
        if len(values) != 1:
            raise Refused(f"the jobs name more than one {key}: {' '.join(values)}")
    return {key: jobs[0][key] for key in FACTS}


def on_the_line(jobs, queue):
    """Every job is one the build line QUEUE ran: its files sit in QUEUE's done/, and the
    runner's log there started it under the job's owner and last ended it with rc 0."""
    done = os.path.realpath(os.path.join(queue, "done"))
    for one in jobs:
        if os.path.realpath(os.path.dirname(one["stem"])) != done:
            raise Refused(f"job {one['name']} is not in {done}")
    data, _ = plain_bytes(os.path.join(queue, "runner.log"))
    owner, last = {}, {}
    for line in data.decode("utf-8", errors="replace").splitlines():
        started, ended = STARTED.fullmatch(line), ENDED.fullmatch(line)
        if started:
            owner[started[1]] = started[2]
        elif ended:
            last[ended[1]] = (ended[2], owner.get(ended[1]))
    for one in jobs:
        if last.get(one["name"]) != ("0", one["who"]):
            raise Refused(f"the build line's runner.log never ended job {one['name']} green under {one['who']}")


def held_to(facts, run):
    """The jobs say they are a coordinator's (coord-<label>) of RUN, the lane's own run."""
    if facts["run"] != run:
        raise Refused(f"the jobs say run {facts['run']}, not {run}")
    if not COORDINATOR.fullmatch(facts["who"]):
        raise Refused(f"the jobs say {facts['who']}, not a coordinator's coord-<label>")


def at_top(folder):
    """FOLDER is the top of a checkout: it holds the checkout's .git (a worktree's file, a clone's folder)."""
    return os.path.lexists(os.path.join(folder, ".git"))


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
    jobs = [job(job_stem(path)) for path in paths]
    facts = agreed(jobs)
    seen = {}
    for one in jobs:
        seen.update(one["seen"])
    return {"receipt": VERSION, **facts, "made": int(time.time()),
            "jobs": [job_stem(path) for path in paths], "sealed": seen}


def read(path, folder=None, line=None, run=None):
    """What a receipt stands for. LINE and RUN — the build line its jobs ran on and the run they
    must say — are the lane's, and the CLI asks for both; without them only the evidence is read."""
    data, _ = plain_bytes(path)
    try:
        receipt = json.loads(data)
    except ValueError:
        raise Refused("the receipt is not JSON") from None
    if not isinstance(receipt, dict) or receipt.get("receipt") != VERSION:
        raise Refused("the receipt is not a version 1 receipt")
    stems, sealed, made = receipt.get("jobs"), receipt.get("sealed"), receipt.get("made")
    if not isinstance(stems, list) or not all(isinstance(stem, str) for stem in stems) \
            or not isinstance(sealed, dict) or not isinstance(made, int):
        raise Refused("the receipt has no jobs, seal or time")
    jobs = [job(stem) for stem in stems]
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
    if line is not None:
        on_the_line(jobs, line)
    if run is not None:
        held_to(facts, run)
    heads = {}
    for one in jobs:
        heads[one["head"]] = min(heads.get(one["head"], one["ended"]), one["ended"])
    said = [f"{key} {value}" for key, value in facts.items()]
    said += [f"head {head} {int(ended)}" for head, ended in sorted(heads.items())]
    if folder is not None and at_top(facts["cwd"]):
        recipes = {recipe_of(command, folder) for one in jobs for command in one["entries"].values()}
        said += [f"recipe {name}" for name in sorted(recipes - {None})]
    return said


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    actions = parser.add_subparsers(dest="action", required=True)
    making = actions.add_parser("make", help="seal a gate's build-line jobs into a receipt (JSON on stdout)")
    making.add_argument("jobs", nargs="+", help="the jobs' .job files; their .rc and .log sit beside them")
    reading = actions.add_parser("read", help="read a receipt's evidence again and print what it stands for")
    reading.add_argument("--receipt", required=True)
    reading.add_argument("--line", required=True, help="the build line's queue folder: every job sits in its done/ and its runner.log")
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
