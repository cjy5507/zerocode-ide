#!/usr/bin/env python3
"""Before a batch is gated: the contracts counted from text, read in seconds and with no build.

10-25's batch gate went round four times on reds a read of the text finds: a bundled
skill's text with no line in skill_releases.tsv (l78d, about 7 minutes in), a tools seed's
copy of the ledger's own kinds behind the Rust source (l78d, about 21 minutes in), a launch
road more than the shell's literal count allows (l78g, about 13 minutes in) and a call at
load the settings test's fake backend has no case for (l78h, about 90 minutes in). Each is
a rule a test already holds; this reads the text that test reads, before anything is built:

    prebatch.py --base <rev> --head <rev> [--repo DIR] [--slow]

It takes the paths that differ between the two trees and runs only the checks that read
one of them. Every check is one entry of CHECKS below, and what it reads is said there:

    skill-release       every skills/<name>/SKILL.md of head has its `<name>\\t<blob>`
                        line in crates/zerocode-core/src/skill_releases.tsv
                        (core every_bundled_skill_text_is_a_recorded_release)
    backend-count       an `assert_eq!(<shipped>.matches("<text>").count(), <n>, …)` over
                        a crate's shipped_backend() counts <n> in head's BACKEND_PARTS:
                        the n is the test's own, read from the test
    settings-ipc        a statement at the top of a script ui/index.html loads that calls
                        invoke("<name>") names a command ui/tests/settings*.mjs has a
                        `case` for (settings: the stateful backend refused every command
                        the fixture did not define)
    tools-test:<file>   the justfile's tools-test line for <file>, run in a checkout of
                        head made for it, when a path that test reads changed; four at
                        once, each capped, and the SLOW ones only with --slow

A changed path no check reads is printed `unchecked <path>`: nothing here passed it, and
the gate still runs all it owes. Nothing here runs cargo, and nothing here stands in for a
gate. Prints `RC <check>=<rc> (<s>s)` for every check it ran with its reasons under it,
`unchecked …` lines, and `RC prebatch=<rc> (<s>s)` last; exits 1 when a check is red and 2
when the repository or a revision cannot be read.
"""

import argparse
import concurrent.futures
import os
import posixpath
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# A tools test that runs past this is the gate's to run, not a quick check's: said
# unchecked, never red. On 10-25 the seeds took under 2 s each, test_lane and the
# computer bench over 120 s.
TEST_CAP_S = 30
# The tools tests the quick check leaves to the gate unless --slow asks: each ran past
# the cap above in the 10-25 replays of l78d-l78i, and the coordinator keeps the
# default run under 10 s (m-74613). Said unchecked, never run, never passed.
SLOW = ("tools/release/tests/test_lane.py", "tools/computer-bench/test_bench.py")
# Tools tests run side by side, as many at once: the 10-25 heads' picks took 19 s one
# after another and under 10 s four at a time.
AT_ONCE = 4

SKILL = re.compile(r"^skills/[^/]+/SKILL\.md$")
SKILL_RELEASES = "crates/zerocode-core/src/skill_releases.tsv"
INDEX = "ui/index.html"
SETTINGS_TESTS = re.compile(r"^ui/tests/settings[^/]*\.mjs$")
SCRIPT_TAG = re.compile(r'<script\b[^>]*\bsrc="\./([^"]+\.js)"')
# A call at the top of a script runs when the page loads it: `invoke(…)`, `void invoke(…)`
# or `await invoke(…)` opening a line. One inside a handler is indented or follows `=>`.
LOAD_TIME_CALL = re.compile(r'^(?:void\s+|await\s+)?invoke\(\s*"([A-Za-z0-9_:.-]+)"', re.M)
FAKE_CASE = re.compile(r'\bcase\s+"([A-Za-z0-9_:.-]+)"\s*:')
RUST_STR = r'"((?:[^"\\]|\\.)*)"'
COUNT_ASSERT = re.compile(
    r"assert_eq!\(\s*(\w+|shipped_backend\(\))\.matches\(\s*" + RUST_STR + r"\s*\)\s*\.count\(\)\s*,\s*(\d+)\s*,"
)
LET_SHIPPED = re.compile(r"\blet\s+(?:mut\s+)?(\w+)\s*(?::[^=;]+)?=\s*shipped_backend\(\)\s*;")
LET_BEFORE_TESTS = re.compile(
    r"\blet\s*\(\s*(\w+)\s*,\s*_\s*\)\s*=\s*(\w+)\s*\.split_once\(\s*\"#\[cfg\(test\)\]\"\s*\)"
)
LET_ANY = re.compile(r"\blet\s+(?:mut\s+)?\(?\s*(\w+)")
FN_START = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+\w+", re.M)
# What shipped_backend() does to BACKEND_PARTS, as this reads it; a book whose function
# no longer says all of it is unchecked rather than counted another way.
BOOK_STEPS = ('split_once("#[cfg(test)]")', '.join("\\n")', '.replace("pub(super) ", "")')


class Unreadable(Exception):
    pass


class Head:
    """One revision's files, read through a single `git cat-file --batch`."""

    def __init__(self, repo: Path, rev: str):
        self.repo, self.rev = repo, rev
        self.cache = {}
        self.batch = subprocess.Popen(
            ["git", "-C", str(repo), "cat-file", "--batch"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        )
        listed = git(repo, "ls-tree", "-r", "-z", rev)
        self.blobs = {}
        for entry in listed.split("\0"):
            if not entry:
                continue
            meta, path = entry.split("\t", 1)
            mode, kind, blob = meta.split()
            if kind == "blob" and mode in ("100644", "100755"):
                self.blobs[path] = blob
        self.tops = {path.split("/", 1)[0] for path in self.blobs}

    def read(self, path):
        if path not in self.cache:
            text = None
            if path in self.blobs:
                self.batch.stdin.write(f"{self.blobs[path]}\n".encode())
                self.batch.stdin.flush()
                header = self.batch.stdout.readline().split()
                size = int(header[2])
                body = self.batch.stdout.read(size + 1)[:size]
                text = body.decode("utf-8", errors="replace")
            self.cache[path] = text
        return self.cache[path]

    def grep(self, pattern, *pathspec):
        said = subprocess.run(
            ["git", "-C", str(self.repo), "grep", "-l", "-F", pattern, self.rev, "--", *pathspec],
            capture_output=True, text=True,
        )
        return [line.split(":", 1)[1] for line in said.stdout.splitlines() if ":" in line]

    def close(self):
        self.batch.stdin.close()
        self.batch.wait()


def git(repo, *args):
    said = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)
    if said.returncode != 0:
        raise Unreadable(f"git {' '.join(args[:2])}: {said.stderr.strip() or said.returncode}")
    return said.stdout


def line_of(text, at):
    return text.count("\n", 0, at) + 1


def rust_unescape(literal):
    simple = {"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}
    return re.sub(r"\\(.)", lambda m: simple.get(m.group(1), m.group(0)), literal)


class Check:
    """What one check reads (exact paths and folders) and how it runs."""

    def __init__(self, name, paths=(), folders=(), run=None, why_not=None, fresh=False):
        self.name, self.paths, self.folders, self.run = name, set(paths), tuple(folders), run
        # A check that cannot be made from head says why instead of running.
        self.why_not = why_not
        # A tools-test line head has and base does not: the justfile itself asks for it.
        self.fresh = fresh

    def reads(self, path):
        return path in self.paths or any(path == f or path.startswith(f + "/") for f in self.folders)


# -- skill-release -----------------------------------------------------------------

def skill_release(head, base_paths, base_justfile):
    skills = sorted(p for p in head.blobs if SKILL.match(p))
    watched = set(skills) | {p for p in base_paths if SKILL.match(p)} | {SKILL_RELEASES}

    def run():
        book = head.read(SKILL_RELEASES)
        if book is None:
            return [f"head has no {SKILL_RELEASES}"]
        lines = {tuple(line.split("\t")[:2]) for line in book.splitlines()}
        return [
            f"{path} (blob {head.blobs[path]}) has no line in {SKILL_RELEASES}: "
            f"add `{path.split('/')[1]}\\t{head.blobs[path]}`"
            for path in skills if (path.split("/")[1], head.blobs[path]) not in lines
        ]

    return [Check("skill-release", watched, run=run)]


# -- backend-count -----------------------------------------------------------------

def backend_books(head):
    """{book file: (its parts' paths, its shipped text) or the reason it cannot be read}."""
    books = {}
    for path in head.grep("const BACKEND_PARTS", "crates"):
        text = head.read(path) or ""
        start = text.find("const BACKEND_PARTS")
        end = text.find("\n];", start)
        body = re.search(r"\bfn\s+shipped_backend\s*\(\)[^{]*\{(.*?)\n\}", text, re.S)
        if end < 0 or body is None or not all(step in body.group(1) for step in BOOK_STEPS):
            books[path] = "its shipped_backend() no longer reads as split, join and replace"
            continue
        parts = [
            posixpath.normpath(posixpath.join(posixpath.dirname(path), part))
            for part in re.findall(r'include_str!\(\s*"([^"]+)"\s*\)', text[start:end])
        ]
        missing = [part for part in parts if head.read(part) is None]
        if missing:
            books[path] = f"head has no {missing[0]}"
            continue
        texts = [head.read(part).split("#[cfg(test)]", 1)[0] for part in parts]
        books[path] = (parts, "\n".join(texts).replace("pub(super) ", ""))
    return books


def book_for(path, books):
    """A contract counts over its own file's book, else over the nearest folder's above it."""
    if path in books:
        return path
    folder = posixpath.dirname(path)
    while folder:
        here = [book for book in books if posixpath.dirname(book) == folder]
        if len(here) == 1:
            return here[0]
        if here:
            return None
        folder = posixpath.dirname(folder)
    return None


def count_asserts(text):
    """(line, literal, n) for every equality count over the shipped backend in TEXT."""
    found = []
    starts = [m.start() for m in FN_START.finditer(text)] + [len(text)]
    for begin, end in zip(starts, starts[1:]):
        chunk = text[begin:end]
        shipped = {}
        for m in LET_ANY.finditer(chunk):
            shipped[m.start()] = (m.group(1), False)
        for m in LET_SHIPPED.finditer(chunk):
            shipped[m.start()] = (m.group(1), True)
        for m in LET_BEFORE_TESTS.finditer(chunk):
            shipped[m.start()] = (m.group(1), None, m.group(2))
        for m in COUNT_ASSERT.finditer(chunk):
            receiver, literal, n = m.group(1), rust_unescape(m.group(2)), int(m.group(3))
            if receiver != "shipped_backend()" and not binds_shipped(shipped, receiver, m.start()):
                continue
            found.append((line_of(text, begin + m.start()), literal, n))
    return found


def binds_shipped(lets, name, at):
    """Whether NAME, where it is used at AT, was last bound to the shipped backend."""
    for pos in sorted((p for p in lets if p < at), reverse=True):
        binding = lets[pos]
        if binding[0] != name:
            continue
        if binding[1] is None:
            return binds_shipped(lets, binding[2], pos)
        return binding[1]
    return False


def backend_count(head, base_paths, base_justfile):
    books = backend_books(head)
    contracts = {}
    for path in head.grep("shipped_backend()", "crates"):
        asserts = count_asserts(head.read(path) or "")
        if asserts:
            contracts[path] = asserts
    watched = set(books) | set(contracts)
    for book in books.values():
        if isinstance(book, tuple):
            watched |= set(book[0])
    unread = [f"{path}: {why}" for path, why in books.items() if isinstance(why, str)]

    def run():
        red = []
        for path, asserts in sorted(contracts.items()):
            book = book_for(path, books)
            if book is None or not isinstance(books[book], tuple):
                continue
            shipped = books[book][1]
            for line, literal, n in asserts:
                counted = shipped.count(literal)
                if counted != n:
                    red.append(f"{path}:{line} counts {counted} of `{literal}` in {book}'s "
                               f"shipped_backend(), the test says {n}")
        return red

    return [Check("backend-count", watched, run=run, why_not="; ".join(unread) or None)]


# -- settings-ipc ------------------------------------------------------------------

def settings_ipc(head, base_paths, base_justfile):
    index = head.read(INDEX)
    fakes = sorted(p for p in head.blobs if SETTINGS_TESTS.match(p))
    if index is None or not fakes:
        return []
    scripts = [f"ui/{src}" for src in SCRIPT_TAG.findall(index)]
    watched = {INDEX, *scripts, *fakes} | {p for p in base_paths if SETTINGS_TESTS.match(p)}

    def run():
        known = set()
        for fake in fakes:
            known |= set(FAKE_CASE.findall(head.read(fake) or ""))
        red = []
        for script in scripts:
            text = head.read(script)
            if text is None:
                continue  # a bundle the build writes (ui/vendor/cm6.js), not a source
            for m in LOAD_TIME_CALL.finditer(text):
                if m.group(1) not in known:
                    red.append(f"{script}:{line_of(text, m.start())} calls invoke(\"{m.group(1)}\") as the page "
                               f"loads, and no ui/tests/settings*.mjs has a case for it")
        return red

    return [Check("settings-ipc", watched, run=run)]


# -- tools-test --------------------------------------------------------------------

def recipe_lines(justfile, recipe):
    lines, inside = [], False
    for line in justfile.splitlines():
        if re.match(rf"^{re.escape(recipe)}\s*(?::|\s)", line) and not line.startswith(" "):
            inside = True
            continue
        if inside:
            if line.strip() and not line[0].isspace():
                break
            if line.strip() and not line.strip().startswith("#"):
                lines.append(line.strip())
    return lines


def references(text, tops):
    """The repository paths a Python test names, as literals or as `X / "a" / "b"` chains."""
    found = set()
    for literal in re.findall(r"[\"']([A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)+)/?[\"']", text):
        found.add(literal)
    for chain in re.findall(r"\b\w+((?:\s*/\s*[\"'][A-Za-z0-9_./-]+[\"'])+)", text):
        found.add("/".join(seg.strip("/") for seg in re.findall(r"[\"']([^\"']+)[\"']", chain)))
    return {path for path in found if path.split("/", 1)[0] in tops}


def tools_test_words(line):
    """The python3 command one tools-test line runs, or None. A line is a plain `python3 …`,
    or a call of the recipe's helpers that t-74384 brought (every script runs after one
    fails): `t …; \\` runs `python3 …`, and `macos "<why>" …; \\` runs it on a Mac only.
    The helpers' own definitions and the recipe's tally lines run no script."""
    line = line.removesuffix("\\").strip().removesuffix(";").strip()
    try:
        words = shlex.split(line)
    except ValueError:
        return None
    if not words:
        return None
    if words[0] == "python3":
        return words
    if words[0] == "t" and len(words) > 1:
        return ["python3", *words[1:]]
    if words[0] == "macos" and len(words) > 2 and sys.platform == "darwin":
        return ["python3", *words[2:]]
    return None


def tools_tests(head, base_paths, base_justfile):
    justfile = head.read("justfile")
    if justfile is None:
        return []
    before = set(recipe_lines(base_justfile or "", "tools-test"))
    checks = []
    for line in recipe_lines(justfile, "tools-test"):
        fresh = line not in before
        words = tools_test_words(line)
        test = next((w for w in words or () if w.endswith(".py")), None)
        if test is None:
            continue
        name = f"tools-test:{test}"
        folder = posixpath.dirname(test)
        folders = [folder]
        if posixpath.basename(folder) == "tests":
            folders.append(posixpath.dirname(folder))
        text = head.read(test)
        if text is None:
            def missing(test=test):
                return [f"the justfile's tools-test runs {test}, which head does not have"]
            checks.append(Check(name, {test}, folders=[folder], run=missing, fresh=fresh))
            continue
        reads = references(text, head.tops)
        for ref in sorted(reads):
            if ref.endswith(".py"):
                reads |= references(head.read(ref) or "", head.tops)
        # Only the files it names: a folder a test spells is where it builds a fixture
        # (`REPO / "zo-ide"` as a cwd), and would pick it for every change under it.
        paths = {test} | {r for r in reads if r in head.blobs}
        # The test's own folder and, for a tests/ folder, the tool beside it: the modules
        # it imports by name are files there, not paths it spells.
        siblings = {p for p in head.blobs if posixpath.dirname(p) in folders}
        checks.append(Check(name, paths | siblings, run=("tools", words), fresh=fresh))
    return checks


def run_tools_test(words, checkout, cap=TEST_CAP_S):
    try:
        said = subprocess.run(
            words, cwd=checkout, capture_output=True, text=True, timeout=cap,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
    except subprocess.TimeoutExpired:
        return None
    if said.returncode == 0:
        return []
    tail = (said.stdout + said.stderr).strip().splitlines()[-6:]
    return [f"rc={said.returncode}: {shlex.join(words)}", *("  " + line for line in tail)]


def checkout_of(repo, head, into):
    """A checkout of HEAD that shares REPO's objects: the tests read files and git as there."""
    git(repo, "clone", "-q", "--shared", "--no-checkout", str(repo), str(into))
    git(into, "-c", "advice.detachedHead=false", "checkout", "-q", "--detach", head)
    return into


def timed(run, *args):
    started = time.monotonic()
    return run(*args), time.monotonic() - started


# -- the table ---------------------------------------------------------------------

CHECKS = (skill_release, backend_count, settings_ipc, tools_tests)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", required=True, help="the tree the batch starts from (main)")
    parser.add_argument("--head", required=True, help="the tree the batch would gate")
    parser.add_argument("--repo", default=".", help="the repository (default: here)")
    parser.add_argument("--slow", action="store_true", help="also run the slow tools tests (SLOW), uncapped")
    args = parser.parse_args(argv)
    # A stopped run still removes the checkout it made.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    began = time.monotonic()
    repo = Path(args.repo).resolve()
    try:
        base = git(repo, "rev-parse", "--verify", f"{args.base}^{{commit}}").strip()
        head_sha = git(repo, "rev-parse", "--verify", f"{args.head}^{{commit}}").strip()
        changed = [p for p in git(repo, "diff", "--name-only", "--no-renames", "-z", base, head_sha).split("\0") if p]
        base_paths = set(git(repo, "ls-tree", "-r", "--name-only", "-z", base).split("\0")) - {""}
        head = Head(repo, head_sha)
        listed = subprocess.run(["git", "-C", str(repo), "show", f"{base}:justfile"], capture_output=True, text=True)
        base_justfile = listed.stdout if listed.returncode == 0 else None
    except Unreadable as error:
        print(f"prebatch: {error}")
        return 2
    print(f"base {base} · head {head_sha} · {len(changed)} paths changed")
    red_any, covered, scratch = False, set(), None
    try:
        picked = []
        for make in CHECKS:
            for check in make(head, base_paths, base_justfile):
                touched = [p for p in changed if check.reads(p)]
                if check.fresh and "justfile" in changed:
                    touched.append("justfile")
                if touched:
                    picked.append((check, touched))
        tools = [check for check, _ in picked if isinstance(check.run, tuple) and not check.why_not]
        slow = {check.name for check in tools if check.name.split(":", 1)[1] in SLOW}
        running = {}
        if any(check.name not in slow or args.slow for check in tools):
            scratch = Path(tempfile.mkdtemp(prefix="prebatch-"))
            checkout_of(repo, head_sha, scratch / "head")
        pool = concurrent.futures.ThreadPoolExecutor(max_workers=AT_ONCE)
        for check in tools:
            if check.name in slow and not args.slow:
                continue
            cap = None if check.name in slow else TEST_CAP_S
            running[check.name] = pool.submit(timed, run_tools_test, check.run[1], scratch / "head", cap)
        for check, touched in picked:
            if check.why_not:
                print(f"unchecked check {check.name} — {check.why_not}")
                continue
            if check.name in slow and not args.slow:
                print(f"unchecked check {check.name} — slow, --slow runs it; the gate runs it")
                continue
            red, took = running[check.name].result() if check.name in running else timed(check.run)
            if red is None:
                print(f"unchecked check {check.name} — ran past {TEST_CAP_S} s; the gate runs it")
                continue
            covered.update(touched)
            red_any = red_any or bool(red)
            print(f"RC {check.name}={1 if red else 0} ({took:.1f}s)")
            for reason in red:
                print(f"  {reason}")
        pool.shutdown()
    except Unreadable as error:
        print(f"prebatch: {error}")
        return 2
    finally:
        head.close()
        if scratch is not None:
            shutil.rmtree(scratch, ignore_errors=True)
    for path in changed:
        if path not in covered:
            print(f"unchecked {path}")
    rc = 1 if red_any else 0
    print(f"RC prebatch={rc} ({time.monotonic() - began:.1f}s)")
    return rc


if __name__ == "__main__":
    sys.exit(main())
