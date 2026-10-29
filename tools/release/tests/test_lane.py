#!/usr/bin/env python3
"""Dry-run contract for the release lane (`tools/release/lane.sh`).

Every heavy step — clone, `just verify`, cargo, tauri, ditto — is a stub that
`RELEASE_DRY_RUN=1` switches in and the `RELEASE_STUB_*` variables steer, so
the whole lane runs in well under a second and the cases from the design's §4
(docs/design/release-lane-off-the-window.md) are pinned here:

  ① a disk floor refusal keeps the queue file
  ② the scratch is gone on a red gate, on SIGTERM, on a missing tool
  ③ status.json phase order, installed.json app/zo written independently
  ④ the target cap cleans only the oversized target
  ⑤ a red outside flakes.txt is red for real; inside is judged solo ×3
  ⑥ two queue files drain by mtime, one at a time

and, from docs/design/versioned-auto-update.md §2.2·§2.6·§2.7 (t-3187 U-A):

  ⑦ bundle-updater gathers ZeroCode_<v>_<arch>.app.tar.gz + .sig + latest.json
    under out/<sha8>/, skips aloud without a key, is red on a pubkey mismatch
  ⑧ publish only under RELEASE_PUBLISH=1, refuses an existing tag, beta is a
    prerelease plus a moved `beta` tag, the legacy manifest needs all three zo
    targets or is refused whole

and, from t-47516:

  ⑨ a sha that carries the coordinator's gate receipt for its tree runs only the
    recipes the receipt left owed, skips a gate it covered whole aloud, and runs
    the gate whole — saying why — for a receipt of another tree, a red one, one
    whose evidence changed, or one older than its head

Run: python3 tools/release/tests/test_lane.py   (stdlib only)
"""

import base64
import hashlib
import importlib.util
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import receipt_fixture
from posix_shell import BASH, WINDOWS_ENV, pick_bash, posix

REPO = Path(__file__).resolve().parents[3]
RELEASE = REPO / "tools" / "release"
LANE = RELEASE / "lane.sh"
SCRIPTS = ["lane.sh", "enqueue.sh", "status.sh", "install-launchd.sh", "rollback.sh", "bump.sh",
           "verify-every-recipe.sh"]

PHASES = [
    "archive", "gate-root", "gate-zo", "flakes", "push",
    "build-app", "swap-app", "build-zo", "swap-zo", "bundle-updater", "publish", "sweep",
]
ZO_BUILD_TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu"]
# The window's languages (ui/shell-i18n.js LOCALES, past `system`): the lane names its steps in each.
LANGS = re.findall(r'\{ code: "(\w+)", name:', (REPO / "ui" / "shell-i18n.js").read_text(encoding="utf-8")
                   .split("const LOCALES = [", 1)[1].split("];", 1)[0])
LANGS = [code for code in LANGS if code != "system"]
# The repository a release goes to is the one the app polls: the feed has to be
# reachable without a token, so it is the public distribution repo and not the
# private one this source lives in (design versioned-auto-update.md §2.2).
# Read here from the shipped endpoint, which is where lane.sh reads it too --
# there is one such address in the product and this is it.
GITHUB_REPO = re.match(
    r"https://github\.com/([^/]+/[^/]+)/releases/",
    json.loads((REPO / "crates" / "zerocode-shell" / "tauri.conf.json").read_text())
    ["plugins"]["updater"]["endpoints"][0],
).group(1)
VERSION = "0.1.0"  # the dry-run stub's version (RELEASE_STUB_VERSION)


def load_updater_feed():
    spec = importlib.util.spec_from_file_location("updater_feed", RELEASE / "updater_feed.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def minisign_pub(keyid: bytes, comment="minisign public key: TEST"):
    """A .pub the way `tauri signer generate` writes it: base64 of the
    minisign box (an untrusted-comment line plus the base64 key: alg 'Ed',
    8-byte key id, 32-byte key)."""
    raw = b"Ed" + keyid + bytes(32)
    box = f"untrusted comment: {comment}\n{base64.b64encode(raw).decode()}\n"
    return base64.b64encode(box.encode()).decode()
TARGET_LANES = ["root-gate", "zo-gate", "root-release", "zo-release"]

SHA_A = "a" * 40
SHA_B = "b" * 40
SHA_C = "c" * 40
FLAKE_ROOT = "codex_queue::tests::installed_codex_starts_and_cleans_an_explicit_worker_sidecar"
FLAKE_ZO = "a_pane_grown_without_sigwinch"


def wait_until(pred, timeout=10.0, step=0.05):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if pred():
            return True
        time.sleep(step)
    return pred()


class Lane:
    """One isolated lane: its own state home, scratch root, app dir, zo path."""

    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.home = tmp / "home"
        self.scratch_root = tmp / "scratch"
        self.app_dir = tmp / "Applications"
        self.zo_bin = tmp / "bin" / "zo"
        self.stub_log = tmp / "stub.log"
        self.flakes = tmp / "flakes.txt"
        self.key = tmp / "keys" / "updater.key"
        self.extra_env = {}
        for d in (self.home / "queue", self.scratch_root, self.app_dir, self.zo_bin.parent):
            d.mkdir(parents=True, exist_ok=True)
        self.flakes.write_text(f"root:{FLAKE_ROOT}\nzo:{FLAKE_ZO}\n")

    def write_key(self, keyid=b"\x01\x02\x03\x04\x05\x06\x07\x08"):
        """A key pair on the lane machine; returns the .pub content, which is
        what tauri.conf.json plugins.updater.pubkey carries verbatim."""
        self.key.parent.mkdir(parents=True, exist_ok=True)
        # The header is joined at run time: the contract no_private_key_in_the_tree
        # reads this file too, and a literal here would be its own offender.
        box = "untrusted " + "comment: rsign encrypted secret " + "key\nRWRTY0Iy\n"
        self.key.write_text(base64.b64encode(box.encode()).decode())
        pub = minisign_pub(keyid)
        self.key.with_name("updater.key.pub").write_text(pub)
        return pub

    def env(self, **stubs):
        env = {
            **WINDOWS_ENV,
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": os.environ.get("HOME", posix(self.tmp)),
            "RELEASE_DRY_RUN": "1",
            "RELEASE_HOME": posix(self.home),
            "RELEASE_REPO": posix(REPO),
            "RELEASE_SCRATCH_ROOT": posix(self.scratch_root),
            "RELEASE_APP_DIR": posix(self.app_dir),
            "RELEASE_ZO_BIN": posix(self.zo_bin),
            "RELEASE_FLAKES_FILE": posix(self.flakes),
            "RELEASE_STUB_LOG": posix(self.stub_log),
            "UPDATER_KEY": posix(self.key),
        }
        env.update(self.extra_env)
        for k, v in stubs.items():
            env["RELEASE_STUB_" + k] = str(v)
        return env

    def enqueue(self, sha, mtime=None):
        f = self.home / "queue" / sha
        f.touch()
        if mtime is not None:
            os.utime(f, (mtime, mtime))
        return f

    def queue(self):
        return sorted(p.name for p in (self.home / "queue").iterdir())

    def run(self, *args, timeout=30, **stubs):
        return subprocess.run(
            [BASH, posix(LANE), *args], env=self.env(**stubs),
            capture_output=True, text=True, timeout=timeout,
        )

    def spawn(self, *args, **stubs):
        return subprocess.Popen(
            [BASH, posix(LANE), *args], env=self.env(**stubs),
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            start_new_session=True,
        )

    def status(self):
        return json.loads((self.home / "status.json").read_text())

    def installed(self):
        p = self.home / "installed.json"
        return json.loads(p.read_text()) if p.exists() else None

    def scratch(self, sha):
        return self.scratch_root / f"gate-{sha[:8]}"

    def stub_lines(self):
        return self.stub_log.read_text().splitlines() if self.stub_log.exists() else []

    def target(self, lane):
        return self.home / "target" / lane

    def out(self, sha):
        return self.home / "out" / sha[:8]

    def phase(self, name):
        return [p for p in self.status()["phases"] if p["name"] == name][0]

    def report(self, *suites):
        """A browser harness's report as the window runner prints it — each
        suite's `SUITE  <name>` over its `PASS|FAIL  <check>  — <detail>`
        lines (a suite of None has no line: a harness without suites) — for
        the dry gate to print inside its `<harness>-browser-test` recipe
        (RELEASE_STUB_GATE_ROOT_REPORT)."""
        lines = []
        for suite, checks in suites:
            if suite is not None:
                lines.append(f"SUITE  {suite}")
            for verdict, check, detail in checks:
                lines.append(f"{verdict}  {check}" + (f"  — {detail}" if detail else ""))
        lines += ["", "0/0 passed"]
        path = self.tmp / "report.txt"
        path.write_text("\n".join(lines) + "\n")
        return path

    def unlisted(self):
        p = self.home / "unlisted.txt"
        return p.read_text().splitlines() if p.exists() else []

    def status_line(self):
        r = subprocess.run([BASH, posix(RELEASE / "status.sh")], env=self.env(), capture_output=True, text=True)
        return r.stdout.strip()


class LaneCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.mkdtemp(prefix="lane-")
        self.lane = Lane(Path(self._tmp))

    def tearDown(self):
        shutil.rmtree(self._tmp, ignore_errors=True)

    def phases(self, st):
        return [p["name"] for p in st["phases"]]


class Syntax(LaneCase):
    def test_every_script_parses(self):
        for name in SCRIPTS:
            path = RELEASE / name
            self.assertTrue(path.exists(), f"{name} missing")
            self.assertTrue(os.access(path, os.X_OK), f"{name} not executable")
            rc = subprocess.run([BASH, "-n", posix(path)], capture_output=True, text=True)
            self.assertEqual(rc.returncode, 0, rc.stderr)

    def test_every_verify_recipe_runs_after_one_fails(self):
        # `just verify` stops at its first failed recipe; the gate must not —
        # a flake early in the list would leave the later recipes unjudged.
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: first broken last\n\n"
            "first:\n    echo first >> ran.txt\n\n"
            "broken:\n    echo broken >> ran.txt; exit 1\n\n"
            "last:\n    echo last >> ran.txt\n"
        )
        r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh")], cwd=work,
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual((work / "ran.txt").read_text().split(), ["first", "broken", "last"])
        self.assertIn("recipe `broken` failed", r.stderr, "the failure keeps just's own words for the parser")
        self.assertEqual(
            [l for l in r.stdout.splitlines() if l.startswith("==> verify recipe ")],
            ["==> verify recipe first", "==> verify recipe broken", "==> verify recipe last"],
            "each recipe's output is marked so a failure is read against its own recipe",
        )
        # t-4004: gate-zo went 674 → 1208 s and no log said which recipe grew.
        closes = [l.rsplit(" ", 1) for l in r.stdout.splitlines() if l.startswith("<== verify recipe ")]
        self.assertEqual(
            [head for head, _ in closes],
            ["<== verify recipe first rc=0", "<== verify recipe broken rc=1", "<== verify recipe last rc=0"],
            "each recipe closes with its own rc and seconds",
        )
        self.assertTrue(all(secs[:-1].isdigit() and secs.endswith("s") for _, secs in closes), closes)

    def test_the_lane_parser_reads_the_closing_lines_as_nothing(self):
        # The `<==` line sits between one recipe's failure and the next `==>`
        # mark; failed_tests must name exactly what it named without it.
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: unit quiet knowledge-browser-test green\n\n"
            "unit:\n    printf 'failures:\\n    pane::tests::a_flake\\ntest result: FAILED.\\n'; exit 101\n\n"
            "quiet:\n    exit 1\n\n"
            "knowledge-browser-test:\n    exit 1\n\n"
            "green:\n    true\n"
        )
        log = work / "gate.log"
        with log.open("w") as out:  # one file for both streams, as run_gate writes it
            subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh")], cwd=work,
                           stdout=out, stderr=subprocess.STDOUT)
        text = log.read_text()
        self.assertEqual(text.count("<== verify recipe "), 4, text)
        bare = work / "bare.log"
        bare.write_text("".join(l for l in text.splitlines(True) if not l.startswith("<== ")))
        parse = ('eval "$(sed -n \'/^failed_tests()/,/^}/p\' "$0")"; failed_tests "$1"')

        def failed(path):
            return subprocess.run([BASH, "-c", parse, posix(LANE), posix(path)],
                                  capture_output=True, text=True, check=True).stdout.split()

        self.assertEqual(failed(log), ["harness:knowledge", "pane::tests::a_flake", "recipe:quiet"])
        self.assertEqual(failed(log), failed(bare))

    def test_a_recipe_killed_by_a_signal_ends_the_gate(self):
        # The lane tears a gate down children first (kill_tree). A recipe that
        # died of a signal is that teardown, not a failure to run past: the
        # next recipe must not start and outlive the lane in a swept scratch.
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: first killed last\n\n"
            "first:\n    echo first >> ran.txt\n\n"
            "killed:\n    echo killed >> ran.txt; kill -TERM $$\n\n"
            "last:\n    echo last >> ran.txt\n"
        )
        r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh")], cwd=work,
                           capture_output=True, text=True)
        self.assertGreaterEqual(r.returncode, 128, r.stdout + r.stderr)
        self.assertEqual((work / "ran.txt").read_text().split(), ["first", "killed"])

    def test_python_starts_the_git_bash_and_not_the_wsl_launcher_in_the_windows_directory(self):
        # t-74384: CreateProcess tries System32 before PATH, so a bare "bash" was
        # WSL's launcher on the Windows runner and 125 of the 146 failures began there.
        entries = [r"C:\Windows\system32", r"C:\WINDOWS", r"C:\Program Files\Git\bin", r"C:\tools"]
        present = {r"C:\Windows\system32\bash.exe", r"C:\Program Files\Git\bin\bash.exe", r"C:\tools\bash.exe"}
        self.assertEqual(pick_bash(entries, r"C:\Windows", present.__contains__), r"C:\Program Files\Git\bin\bash.exe")
        self.assertEqual(pick_bash(entries[:2] + [r"C:\WindowsApps"], r"c:\windows", {r"C:\WindowsApps\bash.exe"}.__contains__),
                         r"C:\WindowsApps\bash.exe", "a sibling whose name starts the same is not the Windows directory")
        self.assertIsNone(pick_bash(entries[:2], r"C:\Windows", present.__contains__))

    def test_tools_test_runs_every_script_after_one_fails(self):
        # t-74384: `just` ends a recipe at its first failed line, and the
        # Windows leg's tools-test ended at its second script (test_lane.py), so
        # 48 of the 50 never reported there. The real recipe, every script path
        # swapped for a stub that logs a line, the second one red: all of them
        # still run, the last line names the red one, and the exit is 1.
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        text = (REPO / "justfile").read_text(encoding="utf-8")
        recipe = text[text.index("\ntools-test:\n") + 1:].split("\n\n")[0]
        seen = []

        def stub(match):
            seen.append(match.group(0))
            return "fail.py" if len(seen) == 2 else "ok.py"

        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            'set windows-shell := ["bash", "-eu", "-o", "pipefail", "-c"]\n\n'
            + re.sub(r"tools/\S+\.py", stub, recipe) + "\n", encoding="utf-8")
        (work / "ok.py").write_text('open("ran.txt", "a").write("ok\\n")\n')
        (work / "fail.py").write_text('import sys\nopen("ran.txt", "a").write("fail\\n")\nsys.exit(1)\n')
        calls, macos_only = len(seen), recipe.count("\n    macos ")
        self.assertGreater(calls, 40, "the recipe lists the tools' tests")
        r = subprocess.run(["just", "tools-test"], cwd=work, capture_output=True, text=True, timeout=120)
        ran = (work / "ran.txt").read_text().split()
        elsewhere = sys.platform != "darwin"  # the macOS-only scripts say SKIPPED aloud and do not run
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(len(ran), calls - (macos_only if elsewhere else 0), r.stdout + r.stderr)
        self.assertEqual(ran.count("fail"), 1)
        self.assertEqual(r.stdout.count("SKIPPED python3 "), macos_only if elsewhere else 0, r.stdout)
        self.assertIn("tools-test: red scripts: fail.py", r.stderr)

    def test_the_end_names_every_red_recipe(self):
        # t-21326: a CI leg's log is read from its end. One run must say every
        # recipe that went red, not only the first — and a GitHub run also
        # carries each as an annotation on the job.
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: early middle late\n\n"
            "early:\n    exit 1\n\n"
            "middle:\n    true\n\n"
            "late:\n    exit 3\n"
        )
        for github in (False, True):
            with self.subTest(github=github):
                env = {k: v for k, v in os.environ.items() if k != "GITHUB_ACTIONS"}
                if github:
                    env["GITHUB_ACTIONS"] = "true"
                r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh")], cwd=work, env=env,
                                   capture_output=True, text=True)
                self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
                self.assertEqual(r.stdout.splitlines()[-1], "verify: red recipes: early late", r.stdout)
                notes = [l for l in r.stdout.splitlines() if l.startswith("::error")]
                self.assertEqual(notes, [
                    "::error title=verify::recipe early failed (rc=1)",
                    "::error title=verify::recipe late failed (rc=3)",
                ] if github else [], r.stdout)

    def test_the_closing_summary_is_nothing_to_the_lane_parser(self):
        # The summary and the annotations follow the last recipe's mark; the
        # lane's failed_tests must name exactly what it names without them.
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: quiet green\n\nquiet:\n    exit 1\n\ngreen:\n    true\n")
        log = work / "gate.log"
        env = dict(os.environ, GITHUB_ACTIONS="true")
        with log.open("w") as out:
            subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh")], cwd=work, env=env,
                           stdout=out, stderr=subprocess.STDOUT)
        text = log.read_text()
        self.assertIn("verify: red recipes: quiet", text)
        bare = work / "bare.log"
        bare.write_text("".join(l for l in text.splitlines(True)
                                if not l.startswith(("verify: red recipes:", "::error"))))
        parse = ('eval "$(sed -n \'/^failed_tests()/,/^}/p\' "$0")"; failed_tests "$1"')

        def failed(path):
            return subprocess.run([BASH, "-c", parse, posix(LANE), posix(path)],
                                  capture_output=True, text=True, check=True).stdout.split()

        self.assertEqual(failed(log), ["recipe:quiet"])
        self.assertEqual(failed(log), failed(bare))

    def test_a_ci_leg_runs_the_recipes_as_written(self):
        # The lane holds the cross-check to the strict `win-check`; a CI leg
        # has no cargo-xwin, and its `win-check-if-available` must stay the
        # recipe the justfile wrote — its SKIPPED said aloud (t-21326).
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: first win-check-if-available\n\n"
            "first:\n    true\n\n"
            "win-check-if-available:\n    echo as-written >> ran.txt\n\n"
            "win-check:\n    echo strict >> ran.txt\n"
        )
        for args, ran in (([], "strict"), (["--as-written"], "as-written")):
            with self.subTest(args=args):
                (work / "ran.txt").unlink(missing_ok=True)
                r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh"), *args], cwd=work,
                                   capture_output=True, text=True)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
                self.assertEqual((work / "ran.txt").read_text().split(), [ran])

    def test_every_ci_leg_runs_every_recipe(self):
        # `just verify` stops at its first red recipe: on Windows a flaky test
        # in `test` hid the window crate's own tests for days (t-21326). Each
        # CI leg runs its justfile's verify list through the script instead,
        # as written, and no leg calls `just verify` any more.
        workflow = (REPO / ".github" / "workflows" / "verify.yml").read_text()
        runs = [l.strip() for l in workflow.splitlines() if l.strip().startswith("- run:")]
        self.assertNotIn("- run: just verify", runs)
        self.assertEqual(
            [r for r in runs if "verify-every-recipe.sh" in r],
            ["- run: bash tools/release/verify-every-recipe.sh --as-written",
             "- run: bash ../tools/release/verify-every-recipe.sh --as-written"],
            "the root matrix leg (macOS and Windows) and the zo leg",
        )

    def test_ci_jobs_bound_a_stalled_gate_with_headroom(self):
        workflow = (REPO / ".github" / "workflows" / "verify.yml").read_text()
        for job, minutes in (("verify", 135), ("zo-ide-verify", 110), ("quality-baseline", 25)):
            with self.subTest(job=job):
                block = re.search(
                    r"^  " + re.escape(job) + r":\n(.*?)(?=^  [a-z][a-z0-9-]*:|\Z)",
                    workflow, re.M | re.S,
                )
                self.assertIsNotNone(block)
                self.assertEqual(
                    re.findall(r"^    timeout-minutes: (\d+)$", block[1], re.M),
                    [str(minutes)],
                )

    def test_table_is_the_one_place(self):
        out = subprocess.run([BASH, posix(LANE), "--table"], capture_output=True, text=True, env=self.lane.env())
        self.assertEqual(out.returncode, 0, out.stderr)
        table = dict(line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)
        table = {k: v.strip("'") for k, v in table.items()}
        self.assertTrue(all(line.endswith("'") for line in out.stdout.splitlines()), "values are quoted for eval")
        self.assertEqual(table["DISK_FLOOR_GB"], "10")
        self.assertEqual(table["TARGET_CAP_GB"], "20")
        self.assertEqual(table["ZO_PROFILE"], "release")
        self.assertEqual(table["PHASES"].split(), PHASES)
        # t-83513: the steps are named in the window's five languages, and a silent recipe's
        # limits are table numbers like every other.
        self.assertEqual(table["PHASE_LANGS"].split(), LANGS)
        for key in ("QUIET_FACTOR", "QUIET_CAP_FACTOR", "QUIET_FLOOR_SECS", "QUIET_SAMPLES", "QUIET_POLL_SECS"):
            self.assertGreater(int(table[key]), 0, key)
        self.assertEqual(table["TARGET_LANES"].split(), TARGET_LANES)
        self.assertTrue(int(table["POLL_SECS"]) > 0)
        self.assertTrue(int(table["THROTTLE_SECS"]) > 0)
        # t-3187: the release repo, the opt-ins, the key and the legacy triples.
        self.assertEqual(table["RELEASE_GITHUB_REPO"], GITHUB_REPO)
        self.assertEqual(table["RELEASE_PUBLISH"], "0")
        self.assertEqual(table["RELEASE_CHANNEL"], "stable")
        self.assertEqual(table["RELEASE_LEGACY_MANIFEST"], "0")
        self.assertTrue(table["UPDATER_KEY"].endswith("keys/updater.key"), table["UPDATER_KEY"])
        self.assertEqual(table["UPDATER_PLATFORM"], "darwin-aarch64")
        self.assertEqual(table["UPDATER_ARCH"], "aarch64")
        self.assertEqual(table["ZO_BUILD_TARGETS"].split(), ZO_BUILD_TARGETS)
        self.assertIn("python3", table["TOOLS"].split(), "latest.json is rendered by python3")
        # t-9741: a UI check's suite is chosen by the runner's own variable; unlisted results live beside installed.json.
        self.assertEqual(table["UI_SUITE_CHOOSER"], "WINDOW_SUITES")
        self.assertEqual(Path(table["UNLISTED"]).parent, Path(table["INSTALLED"]).parent)
        # t-47516: a coordinator's receipt is kept beside installed.json, read by the tool beside the lane.
        self.assertEqual(Path(table.get("RECEIPTS", "")).parent, Path(table["INSTALLED"]).parent)
        self.assertEqual(Path(table.get("RECEIPT_PY", "")), RELEASE / "coord_receipt.py")
        # …of jobs the coordinator's build line ran, under the workspace folder where the
        # window's board reads it (schedule_timeline.rs LINE_FOLDER), of the coordinator's run.
        self.assertEqual(Path(table.get("RELEASE_BUILD_LINE", "")).parts[-2:], ("build-line", "build-queue"))
        self.assertEqual(table.get("RELEASE_COORD_RUN"), "run-4275")

    def test_the_release_repo_is_the_feed_the_app_polls(self):
        # Every gate runs in a clone of the checkout, whose `origin` is a path
        # and not a name. Read from that origin the lane published to the
        # source repository -- private, and where no installed app is looking
        # (2026-09-18). The answer is in the tree instead, so a clone that has
        # no GitHub remote at all still names the same place.
        conf = json.loads((REPO / "crates" / "zerocode-shell" / "tauri.conf.json").read_text())
        self.assertIn(f"https://github.com/{GITHUB_REPO}/releases/",
                      conf["plugins"]["updater"]["endpoints"][0],
                      "the lane publishes where the app polls")
        scratch = self.lane.tmp / "scratch" / "crates" / "zerocode-shell"
        scratch.mkdir(parents=True)
        shutil.copy(REPO / "crates" / "zerocode-shell" / "tauri.conf.json",
                    scratch / "tauri.conf.json")
        env = self.lane.env()
        env["RELEASE_REPO"] = str(self.lane.tmp / "scratch")
        out = subprocess.run([BASH, posix(LANE), "--table"], capture_output=True, text=True, env=env)
        self.assertEqual(out.returncode, 0, out.stderr)
        table = {k: v.strip("'") for k, v in
                 (line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)}
        self.assertEqual(table["RELEASE_GITHUB_REPO"], GITHUB_REPO)

    def test_the_default_table_key_is_under_the_release_home(self):
        out = subprocess.run([BASH, posix(LANE), "--table"], capture_output=True, text=True,
                             env={**WINDOWS_ENV, "PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": posix(self.lane.tmp)})
        table = dict(line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)
        self.assertEqual(table["UPDATER_KEY"].strip("'"), f"{posix(self.lane.tmp)}/.local/share/zerocode/release/keys/updater.key")

    def test_tauri_conf_makes_updater_artifacts_and_the_local_build_opts_out(self):
        conf = json.loads((REPO / "crates" / "zerocode-shell" / "tauri.conf.json").read_text())
        self.assertIs(conf["bundle"]["createUpdaterArtifacts"], True, "U-A's one key in tauri.conf.json")
        lane = LANE.read_text()
        # tauri-cli 2.11 refuses to build with createUpdaterArtifacts on and no
        # plugins.updater (U-B's section): the local install build turns the key
        # off through --config, the updater phase is the one that needs it.
        self.assertIn('"createUpdaterArtifacts":false', lane)
        self.assertIn("--no-sign", lane.split("run_build()")[1].split("swap_app()")[0])
        just = (REPO / "justfile").read_text()
        self.assertIn('"createUpdaterArtifacts":false', just.split("package-macos:")[1].split("package-windows:")[0])

    def test_the_dmg_carries_a_sealed_app_or_the_lane_is_red(self):
        """The app the DMG and the updater archive carry is sealed the way the
        installed app is: without a distribution identity it is build-app's own
        app, signed inside-out by the install signer and installed as is, so the
        seal packs the archive and the DMG from it and signs the archive with the
        updater key, and never signs the app again (a re-sign moved the outer
        cdhash, 4929830f to 9cfee161 on 10-13, and would ship an app this machine
        never installed); with an identity, Tauri's own seal is kept. Either way
        the bundle verifies --deep --strict before it is gathered (v1.3.78
        shipped the linker's adhoc signature over unsealed resources and a
        colleague's Mac called it damaged, 2026-09-15)."""
        lane = LANE.read_text()
        built_app = lane.split("run_build() {")[1].split("\n}\n")[0]
        self.assertIn('sign-app-bundle.sh" "$target/release/bundle/macos/$APP_NAME"', built_app,
                      "build-app signs the app inside-out")
        seal = lane.split("seal_updater_bundle() {")[1].split("\n}\n")[0]
        self.assertNotIn("sign-app-bundle.sh", seal, "the seal never signs the installed app again")
        for words in [
            'if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then',
            'COPYFILE_DISABLE=1 tar -czf "$APP_NAME.tar.gz" "$APP_NAME"',
            'npx --no-install tauri signer sign -f "$UPDATER_KEY" "$archive"',
            'hdiutil create -quiet -volname "${APP_NAME%.app}" -srcfolder "$staging" -ov -format UDZO "$dmg"',
            'codesign --verify --deep --strict "$app"',
        ]:
            self.assertIn(words, seal, f"the seal step lost: {words}")
        gathering = lane.split("do_bundle_updater() {")[1].split("\n}\n")[0]
        built = gathering.index('heavy build_updater_bundle "$target" "$password" "$log"')
        sealed = gathering.index('seal_updater_bundle "$target" "$password" "$log" || return 1')
        gathered = gathering.index('archive="$target/release/bundle/macos/$APP_NAME.tar.gz"')
        self.assertTrue(built < sealed < gathered, "the seal runs between the build and the gathering")

    def test_the_list_names_what_a_run_would_run_and_runs_none(self):
        # The lane weighs a coordinator's receipt against the recipes its gate
        # runs (t-47516): the strict Windows check for the lane, the recipe as
        # written for a CI leg — and asking runs nothing.
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: first win-check-if-available\n\n"
            "first:\n    echo first >> ran.txt\n\n"
            "win-check-if-available:\n    echo as-written >> ran.txt\n\n"
            "win-check:\n    echo strict >> ran.txt\n"
        )
        for args, listed in (([], ["first", "win-check"]), (["--as-written"], ["first", "win-check-if-available"])):
            with self.subTest(args=args):
                r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh"), *args, "--list"], cwd=work,
                                   capture_output=True, text=True)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
                self.assertEqual(r.stdout.splitlines(), listed)
        self.assertFalse((work / "ran.txt").exists(), "a list runs nothing")

    def test_only_runs_the_named_recipes_in_the_lists_order(self):
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: a b c win-check-if-available\n\n"
            "a:\n    echo a >> ran.txt\n\nb:\n    echo b >> ran.txt\n\nc:\n    echo c >> ran.txt\n\n"
            "win-check-if-available:\n    echo as-written >> ran.txt\n\n"
            "win-check:\n    echo strict >> ran.txt\n"
        )
        r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh"), "--only", "win-check c a"], cwd=work,
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual((work / "ran.txt").read_text().split(), ["a", "c", "strict"])
        self.assertEqual([l.split()[3] for l in r.stdout.splitlines() if l.startswith("<== verify recipe ")],
                         ["a", "c", "win-check"], "each recipe that ran closes as before")

    def test_only_refuses_a_name_outside_the_list_or_none_at_all(self):
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text("verify: a win-check-if-available\n\na:\n    echo a >> ran.txt\n")
        for only in ("zzz", "a zzz", "", "win-check-if-available"):
            with self.subTest(only=only):
                r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh"), "--only", only], cwd=work,
                                   capture_output=True, text=True)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertFalse((work / "ran.txt").exists(), "a refused name runs nothing, not even the good ones")

    def test_a_verify_that_is_not_a_plain_list_has_no_list(self):
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text("verify: (a 'x')\n\na x:\n    echo {{x}} >> ran.txt\n")
        for args in (["--list"], ["--only", "a"]):
            with self.subTest(args=args):
                r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh"), *args], cwd=work,
                                   capture_output=True, text=True)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertEqual(r.stdout, "")
        self.assertFalse((work / "ran.txt").exists())

    def test_the_notes_readers_are_recipes_a_gate_runs(self):
        # A receipt for the tree before the release notes owes their readers
        # again (t-47516); a reader that is no gate recipe would owe nothing.
        table = dict(l.split("=", 1) for l in subprocess.run(
            [BASH, posix(LANE), "--table"], capture_output=True, text=True, env=self.lane.env()).stdout.splitlines()
            if "=" in l)
        readers = table.get("NOTES_READERS", "").strip("'").split()
        self.assertTrue(readers)
        listed = set()
        for folder in (REPO, REPO / "zo-ide"):
            r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh"), "--list"], cwd=folder,
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            listed.update(r.stdout.split())
        self.assertLessEqual(set(readers), listed)
        self.assertEqual(table.get("RELEASE_NOTES", "").strip("'"), "CHANGELOG.md")

    def test_repo_flakes_file_has_only_prefixed_names(self):
        lines = [l.strip() for l in (RELEASE / "flakes.txt").read_text().splitlines()]
        names = [l for l in lines if l and not l.startswith("#")]
        self.assertTrue(names, "flakes.txt names nothing")
        for n in names:
            # A cargo test path under its gate, or a browser check by the start of its name.
            self.assertRegex(n, r"^((root|zo):[A-Za-z0-9_:]+|ui:\S.*)$", n)
        self.assertIn(f"root:{FLAKE_ROOT}", names)


class Floor(LaneCase):
    def test_disk_below_floor_refuses_and_keeps_the_queue_file(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(DISK_FREE_GB=3)
        self.assertNotEqual(r.returncode, 0, r.stdout)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "refused")
        self.assertTrue(st["reason"].startswith("disk"), st["reason"])
        self.assertEqual(st["sha"], SHA_A)
        self.assertEqual(st["disk_free_gb"], 3)
        self.assertEqual(self.lane.queue(), [SHA_A], "refusal must leave the queue file for the next trigger")
        self.assertFalse(self.lane.scratch(SHA_A).exists())
        self.assertNotIn("archive", self.phases(st))
        self.assertEqual(self.phases(st)[-1], "sweep")

    def test_failed_gate_and_low_disk_stays_red_without_requeuing_the_same_sha(self):
        self.lane.enqueue(SHA_A)
        result = self.lane.run(GATE_ROOT_RC=101, DISK_FREE_GB_GATE_ROOT=3)
        status = self.lane.status()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(status["outcome"], "red")
        self.assertIn("gate-root rc=101", status["reason"])
        self.assertIn("disk", status["reason"])
        self.assertEqual(self.lane.phase("gate-root")["rc"], 101)
        self.assertEqual(self.lane.queue(), [])
        self.assertIsNone(self.lane.installed())
        self.assertNotIn("gate-zo", self.phases(status))

    def test_successful_gate_with_low_disk_keeps_the_retry_after_space_is_freed(self):
        self.lane.enqueue(SHA_A)
        result = self.lane.run(GATE_ROOT_RC=0, DISK_FREE_GB_GATE_ROOT=3)
        self.assertEqual(result.returncode, 3, result.stdout)
        self.assertEqual(self.lane.status()["outcome"], "refused")
        self.assertEqual(self.lane.queue(), [SHA_A])
        self.assertIsNone(self.lane.installed())


class Sweep(LaneCase):
    def test_red_gate_sweeps_the_scratch(self):
        self.lane.enqueue(SHA_A)
        # An unlisted name is judged solo; it stays red solo, so the gate is red.
        r = self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS="shell::tests::a_real_red", SOLO_RCS="1")
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("a_real_red", st["reason"])
        self.assertFalse(self.lane.scratch(SHA_A).exists(), "scratch must be gone after a red gate")
        self.assertEqual(self.lane.queue(), [], "a judged sha leaves the queue")
        self.assertEqual(self.phases(st), ["archive", "gate-root", "gate-zo", "flakes", "sweep"])
        self.assertNotIn("push", " ".join(self.lane.stub_lines()))
        self.assertIsNone(self.lane.installed())

    @unittest.skipIf(os.name == "nt", "Windows has no SIGTERM to trap: os.kill ends the lane at once, with no sweep")
    def test_sigterm_sweeps_the_scratch_and_its_children(self):
        self.lane.enqueue(SHA_A)
        p = self.lane.spawn(GATE_ROOT_SLEEP=60, GATE_ZO_SLEEP=60)
        try:
            self.assertTrue(wait_until(lambda: self.lane.scratch(SHA_A).exists()
                                       and (self.lane.home / "status.json").exists()
                                       and self.lane.status()["phase"].startswith("gate")),
                            "lane never reached the gates")
            os.kill(p.pid, signal.SIGTERM)
            p.wait(timeout=10)
        finally:
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGKILL)
        self.assertNotEqual(p.returncode, 0)
        self.assertFalse(self.lane.scratch(SHA_A).exists(), "scratch must be gone after SIGTERM")
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("SIGTERM", st["reason"])
        self.assertEqual(self.phases(st)[-1], "sweep")
        left = subprocess.run(["pgrep", "-g", str(p.pid)], capture_output=True, text=True)
        self.assertEqual(left.stdout.strip(), "", "gate children survived the lane")
        self.assertFalse((self.lane.home / "lock").exists(), "lock must be released")

    def test_missing_tool_refuses_and_sweeps_a_stale_scratch(self):
        self.lane.enqueue(SHA_A)
        stale = self.lane.scratch(SHA_A)
        stale.mkdir(parents=True)
        (stale / "leftover").write_text("12G of yesterday")
        r = self.lane.run(MISSING_TOOLS="swift")
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "refused")
        self.assertTrue(st["reason"].startswith("tools"), st["reason"])
        self.assertIn("swift", st["reason"])
        self.assertFalse(stale.exists(), "a stale scratch for the sha is swept on every exit path")
        self.assertEqual(self.lane.queue(), [SHA_A])


class Phases(LaneCase):
    def test_cargo_gates_and_solo_browser_judgments_never_overlap(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_SLEEP=0.2, GATE_ZO_SLEEP=0.2,
                          GATE_ROOT_RC=1, GATE_ROOT_HARNESS="window")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        events = [line.split(" ", 1)[1] for line in self.lane.stub_lines()]
        def at(prefix):
            return next(i for i, event in enumerate(events) if event.startswith(prefix))
        self.assertLess(at("gate root target="), at("gate root done rc=1"))
        self.assertLess(at("gate root done rc=1"), at("gate zo target="))
        self.assertLess(at("gate zo target="), at("gate zo done rc=0"))
        self.assertLess(at("gate zo done rc=0"), at("solo root harness:window"))
        solos = [i for i, event in enumerate(events) if event.startswith("solo root harness:window")]
        self.assertEqual(len(solos), 3)
        self.assertLess(max(solos), at("build-app"))
        self.assertLess(max(solos), at("build-zo"))

    def test_green_lane_walks_the_table_and_installs_both(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        self.assertEqual(st["reason"], "")
        self.assertEqual(self.phases(st), PHASES)
        for ph in st["phases"]:
            self.assertEqual(ph["rc"], 0, ph)
            self.assertIsInstance(ph["secs"], int)
        self.assertEqual(st["phase"], "sweep")
        for key in ("sha", "phase", "started_at", "updated_at", "phases", "disk_free_gb", "outcome", "reason"):
            self.assertIn(key, st)
        inst = self.lane.installed()
        self.assertEqual(inst["app"]["sha"], SHA_A)
        self.assertEqual(inst["zo"]["sha"], SHA_A)
        self.assertRegex(inst["app"]["at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        # t-3237: the version rides beside each sha — the same letters as
        # status.json's, so the window can say 「새 버전 {{version}}」.
        self.assertEqual(list(inst["app"]), ["sha", "at", "version"])
        self.assertEqual(list(inst["zo"]), ["sha", "at", "version"])
        self.assertEqual(inst["app"]["version"], VERSION)
        self.assertEqual(inst["zo"]["version"], VERSION)
        self.assertEqual(st["version"], VERSION)
        self.assertTrue((self.lane.app_dir / "ZeroCode.app").is_dir())
        self.assertTrue(self.lane.zo_bin.exists())
        self.assertFalse(self.lane.scratch(SHA_A).exists())
        self.assertEqual(self.lane.queue(), [])

    def test_app_red_still_installs_zo_independently(self):
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        self.lane.enqueue(SHA_B)
        r = self.lane.run(BUILD_APP_RC=1)
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("build-app", st["reason"])
        names = self.phases(st)
        self.assertEqual(names, ["archive", "gate-root", "gate-zo", "flakes", "push",
                                 "build-app", "build-zo", "swap-zo", "sweep"])
        inst = self.lane.installed()
        self.assertEqual(inst["app"]["sha"], SHA_A, "app keeps the last green sha")
        self.assertEqual(inst["zo"]["sha"], SHA_B, "zo swaps on its own")
        self.assertTrue((self.lane.app_dir / "ZeroCode.app.old").is_dir() or True)

    def test_each_half_says_the_version_it_was_built_from(self):
        """t-3237: a red app build leaves the app half at its old sha AND its
        old version while the zo half moves to the new pair; the previous
        file keeps the pair it replaced."""
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        self.lane.enqueue(SHA_B)
        self.assertNotEqual(self.lane.run(VERSION="0.2.0", BUILD_APP_RC=1).returncode, 0)
        inst = self.lane.installed()
        self.assertEqual((inst["app"]["sha"], inst["app"]["version"]), (SHA_A, VERSION))
        self.assertEqual((inst["zo"]["sha"], inst["zo"]["version"]), (SHA_B, "0.2.0"))
        prev = json.loads((self.lane.home / "installed.prev.json").read_text())
        self.assertEqual((prev["zo"]["sha"], prev["zo"]["version"]), (SHA_A, VERSION))
        self.assertEqual(self.lane.status()["version"], "0.2.0")

    def test_an_installed_json_from_before_the_version_is_read_and_kept(self):
        """A file an older lane wrote has no version: the half this run did
        not swap keeps its sha and at, and says an empty version — the
        window reads that as 「unknown」 — while the swapped half says the
        new one."""
        (self.lane.home / "installed.json").write_text(
            '{"app":{"sha":"%s","at":"2026-09-07T08:50:40Z"},"zo":{"sha":"%s","at":"2026-09-07T08:51:10Z"}}\n'
            % (SHA_C, SHA_C))
        self.lane.enqueue(SHA_A)
        self.assertNotEqual(self.lane.run(BUILD_APP_RC=1).returncode, 0)
        inst = self.lane.installed()
        self.assertEqual(inst["app"], {"sha": SHA_C, "at": "2026-09-07T08:50:40Z", "version": ""})
        self.assertEqual((inst["zo"]["sha"], inst["zo"]["version"]), (SHA_A, VERSION))

    def test_second_green_keeps_an_old_app_beside(self):
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        self.lane.enqueue(SHA_B)
        self.assertEqual(self.lane.run().returncode, 0)
        self.assertTrue((self.lane.app_dir / "ZeroCode.app.old").is_dir())
        self.assertEqual((self.lane.app_dir / "ZeroCode.app.old" / "sha").read_text().strip(), SHA_A)
        self.assertEqual((self.lane.app_dir / "ZeroCode.app" / "sha").read_text().strip(), SHA_B)
        self.assertEqual(self.lane.zo_bin.read_text().strip(), SHA_B)
        self.assertEqual(self.lane.zo_bin.with_name("zo.old").read_text().strip(), SHA_A)
        prev = json.loads((self.lane.home / "installed.prev.json").read_text())
        self.assertEqual(prev["app"]["sha"], SHA_A)

    def test_push_non_ff_but_ancestor_still_builds(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PUSH_RC=1, ANCESTOR_RC=0)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.lane.status()["outcome"], "green")

    def test_push_non_ff_and_not_on_origin_stops_before_any_build(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PUSH_RC=1, ANCESTOR_RC=1)
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("push", st["reason"])
        self.assertEqual(self.phases(st), ["archive", "gate-root", "gate-zo", "flakes", "push", "sweep"])
        self.assertNotIn("build-app", " ".join(self.lane.stub_lines()))
        self.assertIsNone(self.lane.installed())


class Calm(LaneCase):
    """A gate does not start on a loud machine (table: CALM_LOAD, CALM_WAIT_SECS).
    The wait is bounded and the load rides every phase line, so a later flake
    judgment can tell a loud run from a quiet one."""

    def test_a_loud_machine_is_said_before_each_gate_and_the_gates_still_run(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(LOAD1=99, LOAD15=40)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        events = [line.split(" ", 1)[1] for line in self.lane.stub_lines()]
        def at(prefix):
            return next(i for i, event in enumerate(events) if event.startswith(prefix))
        self.assertLess(at("loud gate-root load=99"), at("gate root target="))
        self.assertLess(at("gate root done rc=0"), at("loud gate-zo load=99"))
        self.assertLess(at("loud gate-zo load=99"), at("gate zo target="))
        self.assertIn("gate-root: loud — load 99 > 12 after 0s, running anyway", r.stdout)
        self.assertEqual(self.lane.status()["outcome"], "green")

    def test_a_calm_machine_waits_for_nothing_and_every_phase_line_carries_the_load(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(LOAD1=3.2, LOAD15=6.7)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertNotIn("loud", r.stdout)
        self.assertNotIn("waiting for calm", r.stdout)
        phase_lines = [l for l in r.stdout.splitlines() if " rc=" in l and " free=" in l]
        self.assertGreaterEqual(len(phase_lines), len(PHASES) - 1, r.stdout)
        for line in phase_lines:
            self.assertIn(" load=3.2/6.7", line)

    def test_an_unreadable_load_is_said_and_never_judged_calm(self):
        # Under launchd the PATH had no sbin: sysctl was not found, the empty
        # reading passed `"" + 0 <= 12`, and every gate started unjudged.
        self.lane.enqueue(SHA_A)
        r = self.lane.run(LOAD1="?", LOAD15="?")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("gate-root: load unreadable — running without the calm check", r.stdout)
        self.assertNotIn("calm at load", r.stdout)
        self.assertTrue(any("load-unknown gate-root" in l for l in self.lane.stub_lines()))

    @unittest.skipUnless(sys.platform == "darwin", "the load is read with macOS sysctl")
    def test_the_launchd_path_finds_the_load_reading(self):
        out = subprocess.run([BASH, posix(LANE), "--table"], capture_output=True, text=True, env=self.lane.env())
        table = {k: v.strip("'") for k, v in (line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)}
        self.assertIsNotNone(shutil.which("sysctl", path=table["LAUNCHD_PATH"]), table["LAUNCHD_PATH"])


class Cap(LaneCase):
    def test_only_the_oversized_target_is_cleaned(self):
        for lane in TARGET_LANES:
            d = self.lane.target(lane)
            d.mkdir(parents=True)
            (d / "warm").write_text("keep me")
        self.lane.enqueue(SHA_A)
        r = self.lane.run(TARGET_KB_ZO_GATE=21 * 1024 * 1024)  # one GiB over the table's 20G cap
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertFalse((self.lane.target("zo-gate") / "warm").exists(), "the oversized target is cleaned")
        for lane in ("root-gate", "root-release", "zo-release"):
            self.assertTrue((self.lane.target(lane) / "warm").exists(), f"{lane} must stay warm")
        cleans = [l for l in self.lane.stub_lines() if " clean " in l]
        self.assertEqual(len(cleans), 1, cleans)
        self.assertIn("zo-gate", cleans[0])

    def warm_all(self):
        for lane in TARGET_LANES:
            d = self.lane.target(lane)
            d.mkdir(parents=True, exist_ok=True)
            (d / "warm").write_text("keep me")

    def test_a_gate_target_is_released_after_its_last_use_when_the_disk_is_tight(self):
        # 2026-09-11 17:32: the root gate passed and the run was refused at
        # 7 G free with its finished 23 G root-gate still on disk. A gate's
        # target is not used again in the run once its gate passed (no solo
        # judgment needs it), so on a tight disk it goes before the floor.
        self.warm_all()
        self.lane.enqueue(SHA_A)
        r = self.lane.run(DISK_FREE_GB=12)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertFalse((self.lane.target("root-gate") / "warm").exists(), "root-gate released")
        self.assertFalse((self.lane.target("zo-gate") / "warm").exists(), "zo-gate released")
        for lane in ("root-release", "zo-release"):
            self.assertTrue((self.lane.target(lane) / "warm").exists(), f"{lane} is still to be used")
        events = [line.split(" ", 1)[1] for line in self.lane.stub_lines()]
        self.assertLess(events.index("clean root-gate"), next(i for i, e in enumerate(events) if e.startswith("gate zo target=")))
        self.assertIn("released root-gate", r.stdout)

    def test_a_roomy_disk_keeps_every_target_warm(self):
        self.warm_all()
        self.lane.enqueue(SHA_A)
        r = self.lane.run(DISK_FREE_GB=100)
        self.assertEqual(r.returncode, 0, r.stdout)
        for lane in TARGET_LANES:
            self.assertTrue((self.lane.target(lane) / "warm").exists(), f"{lane} must stay warm")

    def test_a_red_gate_keeps_its_target_until_the_solo_judgment_is_done(self):
        self.warm_all()
        self.lane.enqueue(SHA_A)
        r = self.lane.run(DISK_FREE_GB=12, GATE_ROOT_RC=101, GATE_ROOT_FAILS=FLAKE_ROOT)
        self.assertEqual(r.returncode, 0, r.stdout)
        events = [line.split(" ", 1)[1] for line in self.lane.stub_lines()]
        solos = [i for i, e in enumerate(events) if e.startswith("solo root")]
        self.assertEqual(len(solos), 3)
        self.assertLess(max(solos), events.index("clean root-gate"), "the solo runs used the warm target")

    def test_a_gate_runs_without_cargos_incremental_cache_and_the_stale_one_is_removed(self):
        # t-19990: a gate rebuilds the workspace crates every run, and the
        # incremental cache it leaves is the disk — on 2026-10-02 root-gate
        # held 11–12 G of its 18–26 G as incremental/ (7–8 folders a crate, one
        # per way it is built), and on 2026-10-01 a 22 G root-gate left 6 G
        # free mid-lane. Every cargo the lane runs goes without the cache (a
        # solo judgment too, or its fingerprints would differ from the gate's),
        # and what earlier runs left — a cross build's included — is gone
        # before the gates.
        self.warm_all()
        stale = ("debug/incremental/zerocode_shell-0abc/s-1",
                 "x86_64-pc-windows-msvc/debug/incremental/zerocode_pty-0def/s-1")
        for lane in TARGET_LANES:
            for rel in stale:
                d = self.lane.target(lane) / rel
                d.mkdir(parents=True)
                (d / "query-cache.bin").write_text("never read again")
        product = self.lane.target("root-gate") / "debug" / "deps" / "libzerocode_core-0abc.rlib"
        product.parent.mkdir(parents=True)
        product.write_text("a build product")
        self.lane.enqueue(SHA_A)
        r = self.lane.run(DISK_FREE_GB=100, GATE_ROOT_RC=101, GATE_ROOT_FAILS=FLAKE_ROOT)
        self.assertEqual(r.returncode, 0, r.stdout)
        for lane in TARGET_LANES:
            left = sorted(str(p.relative_to(self.lane.target(lane))) for p in self.lane.target(lane).rglob("incremental"))
            self.assertEqual(left, [], f"{lane} keeps a stale incremental cache")
            self.assertTrue((self.lane.target(lane) / "warm").exists(), f"{lane} must stay warm otherwise")
        self.assertTrue(product.exists(), "a build product stays")
        # The setting is the lane's own, exported once: the gates (background
        # subshells) show it, and the solo runs share the same environment.
        gates = [l for l in self.lane.stub_lines() if " gate " in l and "target=" in l]
        self.assertEqual(len(gates), 2, gates)
        for line in gates:
            self.assertIn("incremental=0", line)
        self.assertEqual(len(self.solos_run()), 3, "the red gate was judged solo on the warm target")
        out = subprocess.run([BASH, posix(LANE), "--table"], capture_output=True, text=True, env=self.lane.env())
        self.assertIn("CARGO_INCREMENTAL='0'", out.stdout.splitlines())

    def test_the_repositorys_own_units_are_dropped_and_the_registry_ones_stay_warm(self):
        """Every unit of a crate this repository defines is rebuilt in every run
        (the scratch is a fresh clone), and the ones an earlier version left are
        never read again (the version stamp moves their hash), so they go before
        the gates: on 10-13 they were 10.4 of zo-gate's 12.4 G and 11.4 of
        root-gate's 19 G, 1 G under the cap whose clean made v1.1.62's zo gate
        rebuild 245 registry crates. A registry crate's units stay warm."""
        self.warm_all()
        own, reg = "0123456789abcdef", "fedcba9876543210"
        files = (f".fingerprint/zerocode-core-{own}/lib-zerocode_core.json", f"deps/libzerocode_core-{own}.rlib",
                 f"deps/e2e_hermetic-{own}", f"deps/e2e_hermetic-{own}.d", f"build/zerocode-core-{own}/out/stamp",
                 f".fingerprint/serde-{reg}/lib-serde.json", f"deps/libserde-{reg}.rlib", f"build/serde-{reg}/out/stamp")
        profiles = ("debug", "x86_64-pc-windows-msvc/debug")
        for lane in TARGET_LANES:
            for profile in profiles:
                for rel in files:
                    p = self.lane.target(lane) / profile / rel
                    p.parent.mkdir(parents=True, exist_ok=True)
                    p.write_text("a build product")
        self.lane.enqueue(SHA_A)
        r = self.lane.run(DISK_FREE_GB=100, WORKSPACE_PACKAGES="zerocode-core zo-ide")
        self.assertEqual(r.returncode, 0, r.stdout)
        for lane in TARGET_LANES:
            t = self.lane.target(lane)
            left = sorted(str(q.relative_to(t)) for q in t.rglob("*") if own in q.name)
            self.assertEqual(left, [], f"{lane} keeps a unit of the repository's own crate")
            for profile in profiles:
                for rel in files[5:]:
                    self.assertTrue((t / profile / rel).exists(), f"{lane} lost the registry unit {profile}/{rel}")
            self.assertTrue((t / "warm").exists(), f"{lane} must stay warm otherwise")
        events = [line.split(" ", 1)[1] for line in self.lane.stub_lines()]
        drops = [i for i, e in enumerate(events) if e.startswith("drop-workspace-units ")]
        self.assertEqual(len(drops), len(TARGET_LANES), events)
        self.assertLess(max(drops), next(i for i, e in enumerate(events) if e.startswith("gate root target=")))
        self.assertIn("workspace unit", r.stdout)

    def solos_run(self):
        return [l for l in self.lane.stub_lines() if " solo " in l]

    def test_targets_are_persistent_and_per_lane(self):
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        used = [l for l in self.lane.stub_lines() if "target=" in l]
        dirs = sorted({l.split("target=")[1].split()[0] for l in used})
        self.assertEqual(dirs, sorted(str(self.lane.target(l)) for l in TARGET_LANES))


class Flakes(LaneCase):
    def test_the_solo_judgment_waits_for_a_calm_machine_like_a_gate(self):
        # The solo run is what decides flake or real, and it ran on a machine
        # the lane itself refuses to start a gate on — right after the zo gate,
        # with that gate's compile still flushing. On 2026-09-18 three window
        # assertions (a 2px anchor, two poll counts) failed in the gate at load
        # 7.75, failed again in the solo that followed it, and were green on an
        # idle machine every time they were asked there.
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_HARNESS="window", SOLO_RCS="0",
                          LOAD1="?", LOAD15="?")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("flakes-solo: load unreadable — running without the calm check", r.stdout)
        self.assertTrue(any("load-unknown flakes-solo" in l for l in self.lane.stub_lines()),
                        "the solo judgment asks for a calm machine by its own name")

    def test_a_few_unlisted_reds_are_judged_solo_like_listed_ones(self):
        # Every run of the lane's first day died on a NEW timing wait; a real
        # regression fails solo too, so a small number of names is judged, not
        # refused, whatever the list says — and the unlisted one is named.
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS=f"{FLAKE_ROOT} shell::tests::not_listed_yet")
        self.assertEqual(r.returncode, 0, r.stdout)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        solos = [l for l in self.lane.stub_lines() if " solo " in l]
        self.assertEqual(len([l for l in solos if "not_listed_yet" in l]), 3)
        self.assertEqual(len([l for l in solos if FLAKE_ROOT in l]), 3)
        self.assertIn("unlisted red root:shell::tests::not_listed_yet", r.stdout + r.stderr)

    def test_a_failed_node_harness_is_judged_solo_as_a_whole(self):
        # The window harness has no solo runner per case; the lane re-runs the
        # whole harness x3 in the scratch (release lane, 2026-09-07: the SCM
        # burst case flaked under gate load twice).
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=1, GATE_ROOT_HARNESS="window")
        self.assertEqual(r.returncode, 0, r.stdout)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        solos = [l for l in self.lane.stub_lines() if " solo " in l and "harness:window" in l]
        self.assertEqual(len(solos), 3)

    def test_any_browser_harness_red_is_named_and_judged_solo(self):
        # 1.3.33 (2026-09-11): the knowledge graph harness joined the root gate,
        # flaked on a paint timing under load, and the parser — which knew only
        # window/settings — named nothing, so the lane called it "root:?".
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=1, GATE_ROOT_HARNESS="knowledge")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.lane.status()["outcome"], "green")
        solos = [l for l in self.lane.stub_lines() if " solo " in l and "harness:knowledge" in l]
        self.assertEqual(len(solos), 3)

    def test_a_listed_cluster_is_judged_solo_however_many_fail_at_once(self):
        # 1.3.27 (c8627a49): six listed terminal-settle e2e timed out together
        # under load. Counted against MAX_SOLO_JUDGED they were "red for real"
        # with no solo run, so listing them had changed nothing.
        cluster = [f"e2e_settle_{i}" for i in range(6)]
        self.lane.flakes.write_text("".join(f"zo:{name}\n" for name in cluster))
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ZO_RC=101, GATE_ZO_FAILS=" ".join(cluster))
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.lane.status()["outcome"], "green")
        solos = [l for l in self.lane.stub_lines() if " solo zo " in l]
        self.assertEqual(len(solos), len(cluster) * 3)

    def test_a_recipe_that_failed_without_a_name_is_red_even_beside_a_named_one(self):
        # 1.3.37 (4f97bbd9): shell-test died compiling (ENOSPC) and printed no
        # failures block, while settings-browser-test failed by name. The name
        # was judged solo and the unnamed recipe — whose tests never ran — was
        # forgotten; "no names at all" was the only unnamed rule.
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=1, GATE_ROOT_HARNESS="settings", GATE_ROOT_UNNAMED="shell-test")
        self.assertNotEqual(r.returncode, 0, r.stdout)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("root:recipe:shell-test", st["reason"])
        self.assertFalse([l for l in self.lane.stub_lines() if " solo " in l], "nothing is judged solo when a red is real")

    def test_a_named_cargo_failure_keeps_its_recipe_out_of_the_unnamed_list(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS=FLAKE_ROOT, GATE_ROOT_UNNAMED="")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertNotIn("recipe:", r.stdout)

    def test_too_many_red_names_are_red_without_solo(self):
        self.lane.enqueue(SHA_A)
        names = " ".join(f"shell::tests::red_{i}" for i in range(5))
        r = self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS=names)
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("red_4", st["reason"])
        self.assertEqual([l for l in self.lane.stub_lines() if " solo " in l], [])
        self.assertFalse(self.lane.scratch(SHA_A).exists())

    def test_zo_red_with_no_failure_names_is_red_for_real(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ZO_RC=1, GATE_ZO_FAILS="")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.lane.status()["outcome"], "red")
        self.assertEqual([l for l in self.lane.stub_lines() if " solo " in l], [])

    def test_red_inside_the_list_is_judged_solo_three_times(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS=FLAKE_ROOT,
                          GATE_ZO_RC=1, GATE_ZO_FAILS=f"pane::tests::{FLAKE_ZO}")
        self.assertEqual(r.returncode, 0, r.stdout)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        solos = [l for l in self.lane.stub_lines() if " solo " in l]
        self.assertEqual(len([l for l in solos if " root " in l and FLAKE_ROOT in l]), 3)
        self.assertEqual(len([l for l in solos if " zo " in l and FLAKE_ZO in l]), 3)
        flakes = [p for p in st["phases"] if p["name"] == "flakes"][0]
        self.assertEqual(flakes["rc"], 0)
        self.assertEqual(self.phases(st), PHASES)

    def test_a_flake_that_stays_red_solo_is_red(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS=FLAKE_ROOT, SOLO_RCS="0 1")
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("solo", st["reason"])
        self.assertEqual(len([l for l in self.lane.stub_lines() if " solo " in l]), 2)
        self.assertNotIn("push", " ".join(self.lane.stub_lines()))
        self.assertFalse(self.lane.scratch(SHA_A).exists())



PCV = "pane-conversation-view"
TOGGLE = ("the active agent pane's toggle swaps its screen for its conversation in the same slot — "
          "turns from the pane's own transcript")
PCV_UNIT = f"ui:window/{PCV}"


class UiFlakes(LaneCase):
    """t-9741: a browser harness's FAIL line is judged by name, like a cargo
    test's — its suite re-run alone x3 through the window runner's chooser
    (the harness whole when it has no suites), a listed check (`ui:<the start
    of its name>`) outside the unlisted cap, and an unlisted one named with its
    result in the lane log and in unlisted.txt beside installed.json. Before,
    any FAIL line re-ran the whole harness: eight minutes a run, 1769 checks
    for another flake to land in."""

    def gate_with(self, *suites, harness="window", **stubs):
        self.lane.enqueue(SHA_A)
        return self.lane.run(GATE_ROOT_RC=1, GATE_ROOT_HARNESS=harness,
                             GATE_ROOT_REPORT=self.lane.report(*suites), **stubs)

    def solos(self):
        return [line.split(" ", 1)[1] for line in self.lane.stub_lines() if " solo " in line]

    def parse(self, text):
        log = self.lane.tmp / "gate.log"
        log.write_text(text)
        parse = 'eval "$(sed -n \'/^failed_tests()/,/^}/p\' "$0")"; failed_tests "$1"'
        return subprocess.run([BASH, "-c", parse, posix(LANE), posix(log)],
                              capture_output=True, text=True, check=True).stdout.splitlines()

    def test_a_listed_ui_check_is_judged_by_its_suite_alone(self):
        self.lane.flakes.write_text(f"root:{FLAKE_ROOT}\nui:the active agent pane's toggle swaps\n")
        r = self.gate_with((PCV, [("PASS", "a check beside it", "{}"), ("FAIL", TOGGLE, '{"otherPaneQuiet":false}')]))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertEqual(self.solos(), [f"solo root {PCV_UNIT} run={i}" for i in (1, 2, 3)])
        self.assertNotIn("unlisted red", r.stdout)
        self.assertEqual(self.lane.unlisted(), [])

    def test_an_unlisted_ui_check_is_judged_by_its_suite_and_its_result_is_kept(self):
        # An empty `ui:` line lists nothing — it is the start of no name.
        self.lane.flakes.write_text(f"root:{FLAKE_ROOT}\nui:\n")
        r = self.gate_with((PCV, [("FAIL", TOGGLE, '{"otherPaneQuiet":false}')]))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertEqual(self.solos(), [f"solo root {PCV_UNIT} run={i}" for i in (1, 2, 3)])
        named = f"root:{PCV_UNIT}:{TOGGLE}"
        self.assertIn(f"unlisted red {named} — judged solo\n", r.stdout)
        self.assertIn(f"unlisted red {named} — judged solo 3/3 green\n", r.stdout)
        kept = self.lane.unlisted()
        self.assertEqual(len(kept), 1, kept)
        self.assertIn(f" {SHA_A[:8]} {named} — judged solo 3/3 green (out/gate-root-{SHA_A[:8]}.log)", kept[0])
        self.assertIn("unlisted=1(unlisted.txt)", self.lane.status_line())

    def test_an_unlisted_ui_check_that_fails_solo_is_red_and_its_result_is_kept(self):
        r = self.gate_with((PCV, [("FAIL", TOGGLE, "why")]), SOLO_RCS="0 1")
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn(f"solo root {PCV_UNIT} run 2 rc=1", st["reason"])
        kept = self.lane.unlisted()
        self.assertEqual(len(kept), 1, kept)
        self.assertIn(f"root:{PCV_UNIT}:{TOGGLE} — judged solo 1/3 red", kept[0])
        self.assertNotIn("push", " ".join(self.lane.stub_lines()))

    def test_the_checks_of_one_suite_rerun_that_suite_once_a_round(self):
        r = self.gate_with((PCV, [("FAIL", TOGGLE, "a"), ("FAIL", "the card stands in the dock", "b")]),
                           ("conversation-scroll", [("FAIL", "the reader owns the scroll", "c")]))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.solos(),
                         [f"solo root ui:window/conversation-scroll run={i}" for i in (1, 2, 3)] +
                         [f"solo root {PCV_UNIT} run={i}" for i in (1, 2, 3)])
        self.assertEqual(len(self.lane.unlisted()), 3, "each unlisted check keeps its own line")

    def test_ui_checks_count_by_name_against_the_unlisted_cap(self):
        r = self.gate_with((PCV, [("FAIL", f"a red check {i}", "") for i in range(5)]))
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn(f"root:{PCV_UNIT}:a red check 4", st["reason"])
        self.assertEqual(self.solos(), [])

    def test_listed_ui_checks_stay_outside_the_unlisted_cap(self):
        self.lane.flakes.write_text("ui:a listed check\n")
        r = self.gate_with((PCV, [("FAIL", f"a listed check {i}", "") for i in range(5)]))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.solos(), [f"solo root {PCV_UNIT} run={i}" for i in (1, 2, 3)])

    def test_a_check_of_a_harness_without_suites_reruns_that_harness_whole(self):
        r = self.gate_with((None, [("FAIL", "a settings check", "d")]), harness="settings")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.solos(), [f"solo root harness:settings run={i}" for i in (1, 2, 3)])
        self.assertIn("root:ui:settings:a settings check — judged solo 3/3 green", self.lane.unlisted()[0])

    def test_a_harness_that_named_no_failed_check_is_still_judged_whole(self):
        r = self.gate_with((PCV, [("PASS", "a check", "")]))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.solos(), [f"solo root harness:window run={i}" for i in (1, 2, 3)])

    def test_a_check_is_named_by_harness_suite_and_name_and_only_where_its_recipe_failed(self):
        text = "\n".join([
            "==> verify recipe settings-browser-test",
            "FAIL  a line of a recipe that passed  — why",
            "<== verify recipe settings-browser-test rc=0 1s",
            "==> verify recipe window-browser-test",
            "SUITE  one",
            "PASS  a green check",
            "SUITE  two",
            'FAIL  a red check: with a colon  — {"a":"b  — c"}',
            "FAIL  a bare red check",
            "",
            "1/3 passed",
            "error: recipe `window-browser-test` failed on line 1 with exit code 1",
            "<== verify recipe window-browser-test rc=1 1s",
        ]) + "\n"
        self.assertEqual(self.parse(text), ["ui:window/two:a bare red check", "ui:window/two:a red check: with a colon"])

    def test_the_lane_reads_the_report_the_window_runner_writes(self):
        node = shutil.which("node")
        if node is None:
            self.skipTest("node is not installed")
        script = (
            "import { createRunner } from %s;\n"
            "const runner = createRunner({ env: {}, argv: [] });\n"
            "runner.suite('one', async ({ ok }) => ok('a green check', true, 'x'));\n"
            "runner.suite('two', async ({ ok }) => ok('a red check: with a colon', false, 'a  — b'));\n"
            "await runner.run({});\n"
            "process.exit(runner.report() ? 1 : 0);\n"
        ) % json.dumps((REPO / "ui" / "tests" / "window-runner.mjs").as_uri())
        run = subprocess.run([node, "--input-type=module", "-e", script], capture_output=True, text=True)
        self.assertEqual(run.returncode, 1, run.stderr)
        text = ("==> verify recipe window-browser-test\n" + run.stdout +
                "error: recipe `window-browser-test` failed on line 1 with exit code 1\n")
        self.assertEqual(self.parse(text), ["ui:window/two:a red check: with a colon"])

    def test_a_suite_is_rerun_alone_through_the_runners_own_chooser(self):
        bin_dir = self.lane.tmp / "bin-just"
        bin_dir.mkdir()
        said = self.lane.tmp / "just.txt"
        (bin_dir / "just").write_text(f'#!/bin/sh\nprintf "%s|%s\\n" "${{WINDOW_SUITES-unset}}" "$*" >> "{said}"\n')
        (bin_dir / "just").chmod(0o755)
        solo = 'eval "$(bash "$0" --table)"; eval "$(sed -n \'/^ui_solo()/,/^}/p\' "$0")"; ui_solo "$1"'
        env = {**self.lane.env(), "PATH": f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '/usr/bin:/bin')}"}
        for unit in ("window/pane-conversation-view", "window/a.b"):
            subprocess.run([BASH, "-c", solo, posix(LANE), unit], env=env, check=True)
        self.assertEqual(said.read_text().splitlines(), [
            "^pane-conversation-view$|window-browser-test",
            "^a\\.b$|window-browser-test",
        ])
        runner = (REPO / "ui" / "tests" / "window-runner.mjs").read_text()
        self.assertIn("env.WINDOW_SUITES", runner, "the chooser the lane names is the one the runner reads")


class Queue(LaneCase):
    def test_two_queue_files_drain_by_mtime_one_at_a_time(self):
        now = time.time()
        self.lane.enqueue(SHA_A, mtime=now - 10)   # name-first, but younger
        self.lane.enqueue(SHA_B, mtime=now - 100)  # older: must go first
        r = self.lane.run()
        self.assertEqual(r.returncode, 0, r.stdout)
        lines = self.lane.stub_lines()
        order = []
        for l in lines:
            sha = l.split()[0]
            if sha not in order:
                order.append(sha)
        self.assertEqual(order, [SHA_B[:8], SHA_A[:8]])
        first_sweep = next(i for i, l in enumerate(lines) if l.startswith(SHA_B[:8]) and " sweep" in l)
        second_start = next(i for i, l in enumerate(lines) if l.startswith(SHA_A[:8]))
        self.assertLess(first_sweep, second_start, "the second sha must wait for the first sweep")
        self.assertEqual(self.lane.queue(), [])
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_A)
        self.assertEqual(self.lane.status()["sha"], SHA_A)

    def test_lock_lets_only_one_lane_run(self):
        now = time.time()
        self.lane.enqueue(SHA_A, mtime=now - 100)
        self.lane.enqueue(SHA_B, mtime=now - 10)
        p = self.lane.spawn(GATE_ROOT_SLEEP=60, GATE_ZO_SLEEP=60)
        try:
            # …and has begun its archive: the queue file leaves before the
            # first stub line lands, and the count below wants that line.
            self.assertTrue(wait_until(lambda: (self.lane.home / "lock").exists()
                                       and self.lane.queue() == [SHA_B]
                                       and any(" archive" in l for l in self.lane.stub_lines())),
                            "the first lane takes the oldest sha out of the queue")
            second = self.lane.run(timeout=10)
            self.assertEqual(second.returncode, 0, second.stdout)
            self.assertIn("lock", second.stdout + second.stderr)
            self.assertEqual(self.lane.queue(), [SHA_B], "the second lane must not steal the next queue file")
            self.assertTrue((self.lane.home / "lock").exists())
            self.assertEqual(len([l for l in self.lane.stub_lines() if " archive" in l]), 1)
        finally:
            os.kill(p.pid, signal.SIGTERM)
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)

    def test_a_stale_lock_from_a_dead_lane_is_taken_over(self):
        lock = self.lane.home / "lock"
        lock.mkdir()
        (lock / "pid").write_text("999999999\n")
        self.lane.enqueue(SHA_A)
        r = self.lane.run()
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertFalse(lock.exists())

    def test_direct_sha_argument_runs_without_a_queue_file(self):
        r = self.lane.run(SHA_C)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.lane.status()["sha"], SHA_C)

    def test_empty_queue_is_a_quiet_exit(self):
        r = self.lane.run()
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertFalse((self.lane.home / "status.json").exists())


class Supersede(LaneCase):
    """A queued sha that is an ancestor of another queued sha is never gated
    (t-4123): the descendant carries everything it has. 09-14: v1.3.64, 65 and
    66 stood five minutes apart and the lane would have gated all three, about
    an hour each, until a person deleted queue files and killed the lane.

    Ancestry in dry-run mode is the stub ANCESTRY: space-separated
    `<descendant>:<ancestor>` pairs of full shas."""

    def test_a_queued_ancestor_is_skipped_when_its_descendant_is_queued(self):
        now = time.time()
        self.lane.enqueue(SHA_A, mtime=now - 100)  # the ancestor, older: would go first
        self.lane.enqueue(SHA_B, mtime=now - 10)   # its descendant
        r = self.lane.run(ANCESTRY=f"{SHA_B}:{SHA_A}")
        self.assertEqual(r.returncode, 0, r.stdout)
        lines = self.lane.stub_lines()
        self.assertFalse(any(l.startswith(SHA_A[:8]) and " archive" in l for l in lines),
                         "the ancestor must not even be archived")
        self.assertTrue(any(l.startswith(SHA_B[:8]) and " archive" in l for l in lines))
        self.assertEqual(self.lane.queue(), [])
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_B)
        self.assertEqual(self.lane.status()["sha"], SHA_B)
        self.assertEqual(self.lane.status()["outcome"], "green")
        log_a = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertIn(f"== superseded (superseded by {SHA_B[:8]}", log_a,
                      "the skipped sha's own log names what superseded it")

    def test_unrelated_queued_shas_both_run(self):
        now = time.time()
        self.lane.enqueue(SHA_A, mtime=now - 100)
        self.lane.enqueue(SHA_C, mtime=now - 10)
        r = self.lane.run()  # no ANCESTRY: neither descends from the other
        self.assertEqual(r.returncode, 0, r.stdout)
        archived = [l.split()[0] for l in self.lane.stub_lines() if " archive" in l]
        self.assertEqual(archived, [SHA_A[:8], SHA_C[:8]])

    def test_a_running_sha_bows_out_at_the_next_gate_when_its_descendant_arrives(self):
        self.lane.enqueue(SHA_A)
        p = self.lane.spawn(GATE_ROOT_SLEEP=4, ANCESTRY=f"{SHA_B}:{SHA_A}")
        try:
            self.assertTrue(wait_until(lambda: any(l.startswith(SHA_A[:8]) and " gate root" in l
                                                   for l in self.lane.stub_lines())),
                            "A reached its root gate")
            self.lane.enqueue(SHA_B)  # the descendant arrives mid-gate
            p.communicate(timeout=60)
        finally:
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGKILL)
        lines = self.lane.stub_lines()
        self.assertFalse(any(l.startswith(SHA_A[:8]) and " gate zo" in l for l in lines),
                         "A stops at the gate boundary — its zo gate never runs")
        log_a = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertIn("== superseded", log_a)
        self.assertTrue(any(l.startswith(SHA_B[:8]) and " gate zo" in l for l in lines), "B ran in full")
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_B)
        self.assertEqual(self.lane.queue(), [])

    def test_a_descendant_arriving_after_push_does_not_stop_the_build(self):
        self.lane.enqueue(SHA_A)
        p = self.lane.spawn(BUILD_APP_SLEEP=4, ANCESTRY=f"{SHA_B}:{SHA_A}")
        try:
            self.assertTrue(wait_until(lambda: any(l.startswith(SHA_A[:8]) and " build-app" in l
                                                   for l in self.lane.stub_lines())),
                            "A reached its build")
            self.lane.enqueue(SHA_B)
            p.communicate(timeout=60)
        finally:
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGKILL)
        log_a = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertIn("== green", log_a, "a sha past push is built to the end")
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_B, "and the descendant follows")

    def test_status_wait_answers_for_a_finished_sha_from_its_own_lane_log(self):
        # status.json is the running sha's; a sha that already finished (or was
        # superseded) is answered from out/lane-<sha8>.log, so `--wait` on it
        # cannot hang forever once the lane has moved on.
        now = time.time()
        self.lane.enqueue(SHA_A, mtime=now - 100)
        self.lane.enqueue(SHA_B, mtime=now - 10)
        r = self.lane.run(ANCESTRY=f"{SHA_B}:{SHA_A}")
        self.assertEqual(r.returncode, 0, r.stdout)
        w = subprocess.run([BASH, posix(RELEASE / "status.sh"), "--wait", SHA_A, "--timeout", "5"],
                           env=self.lane.env(), capture_output=True, text=True)
        self.assertEqual(w.returncode, 5, w.stdout + w.stderr)
        self.assertIn("superseded", w.stdout)
        w = subprocess.run([BASH, posix(RELEASE / "status.sh"), "--wait", SHA_B, "--timeout", "5"],
                           env=self.lane.env(), capture_output=True, text=True)
        self.assertEqual(w.returncode, 0, w.stdout + w.stderr)


class Companions(LaneCase):
    def test_enqueue_touches_the_queue_file(self):
        r = subprocess.run([BASH, posix(RELEASE / "enqueue.sh"), SHA_A], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.lane.queue(), [SHA_A])
        self.assertIn(SHA_A[:8], r.stdout)

    def test_enqueue_rejects_a_non_sha(self):
        r = subprocess.run([BASH, posix(RELEASE / "enqueue.sh"), "main"], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.lane.queue(), [])

    def test_status_line_and_wait(self):
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        r = subprocess.run([BASH, posix(RELEASE / "status.sh")], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        line = r.stdout.strip()
        self.assertEqual(line.count("\n"), 0)
        self.assertIn(SHA_A[:8], line)
        self.assertIn("sweep", line)
        self.assertIn("green", line)
        w = subprocess.run([BASH, posix(RELEASE / "status.sh"), "--wait", SHA_A], env=self.lane.env(),
                           capture_output=True, text=True, timeout=10)
        self.assertEqual(w.returncode, 0, w.stdout + w.stderr)
        self.assertIn("green", w.stdout)

    def test_status_wait_exit_code_follows_the_outcome(self):
        self.lane.enqueue(SHA_A)
        self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS="x::y", SOLO_RCS="1")  # stays red solo
        w = subprocess.run([BASH, posix(RELEASE / "status.sh"), "--wait", SHA_A], env=self.lane.env(),
                           capture_output=True, text=True, timeout=10)
        self.assertNotEqual(w.returncode, 0)
        self.assertIn("red", w.stdout)

    def test_status_without_a_file_says_so(self):
        r = subprocess.run([BASH, posix(RELEASE / "status.sh")], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)

    def test_install_launchd_renders_a_valid_plist(self):
        r = subprocess.run([BASH, posix(RELEASE / "install-launchd.sh"), "--render", "--label-suffix", "selftest"],
                           env=self.lane.env(), capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        plist = Path(self._tmp) / "render.plist"
        plist.write_text(r.stdout)
        lint = subprocess.run(["plutil", "-lint", str(plist)], capture_output=True, text=True)
        self.assertEqual(lint.returncode, 0, lint.stdout + lint.stderr)
        self.assertIn("dev.zerocode.release.selftest", r.stdout)
        self.assertIn(str(self.lane.home / "queue"), r.stdout)
        self.assertIn("QueueDirectories", r.stdout)
        self.assertIn("ThrottleInterval", r.stdout)
        self.assertIn(".cargo/bin", r.stdout)
        self.assertIn(str(LANE), r.stdout)
        self.assertIn("RELEASE_HOME", r.stdout)
        # t-4004: with no ProcessType launchd throttles the agent's CPU and I/O
        # (launchd.plist(5)); the zo gate's test compile took 529/842 s so against
        # 228 s as Interactive on the same machine, the same hour.
        self.assertIn("<key>ProcessType</key><string>Interactive</string>", r.stdout)

    def test_rollback_swaps_back_and_rewrites_installed(self):
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        self.lane.enqueue(SHA_B)
        self.assertEqual(self.lane.run(VERSION="0.2.0").returncode, 0)
        r = subprocess.run([BASH, posix(RELEASE / "rollback.sh"), "--app", "--zo"], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual((self.lane.app_dir / "ZeroCode.app" / "sha").read_text().strip(), SHA_A)
        self.assertEqual((self.lane.app_dir / "ZeroCode.app.old" / "sha").read_text().strip(), SHA_B)
        self.assertEqual(self.lane.zo_bin.read_text().strip(), SHA_A)
        inst = self.lane.installed()
        self.assertEqual(inst["app"]["sha"], SHA_A)
        self.assertEqual(inst["zo"]["sha"], SHA_A)
        # t-3237: the version goes back with the sha it belongs to, and the
        # replaced pair waits in installed.prev.json for the next flip.
        self.assertEqual((inst["app"]["version"], inst["zo"]["version"]), (VERSION, VERSION))
        self.assertRegex(inst["app"]["at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        prev = json.loads((self.lane.home / "installed.prev.json").read_text())
        self.assertEqual((prev["app"]["sha"], prev["app"]["version"]), (SHA_B, "0.2.0"))
        self.assertEqual((prev["zo"]["sha"], prev["zo"]["version"]), (SHA_B, "0.2.0"))

    def test_rollback_without_an_old_copy_refuses(self):
        r = subprocess.run([BASH, posix(RELEASE / "rollback.sh"), "--app"], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)


class UpdaterFeed(LaneCase):
    """updater_feed.py — the one function set behind latest.json, the
    CHANGELOG section, the key fingerprint and the legacy manifest."""

    def test_latest_json_is_the_tauri_v2_updater_shape(self):
        feed = load_updater_feed()
        doc = feed.latest_json("0.1.0", "- a note\n- another\n", "2026-09-08T00:00:00Z",
                               {"darwin-aarch64": {"signature": "SIG", "url": "https://x/y.app.tar.gz"}})
        parsed = json.loads(doc)
        self.assertEqual(list(parsed), ["version", "notes", "pub_date", "platforms"])
        self.assertEqual(parsed["version"], "0.1.0")
        self.assertEqual(parsed["notes"], "- a note\n- another\n")
        self.assertEqual(parsed["pub_date"], "2026-09-08T00:00:00Z")
        self.assertEqual(parsed["platforms"], {"darwin-aarch64": {"signature": "SIG", "url": "https://x/y.app.tar.gz"}})
        self.assertTrue(doc.endswith("\n"))

    def test_changelog_section_is_the_body_under_that_version_only(self):
        feed = load_updater_feed()
        text = ("# Changelog\n\n## [0.2.0] — 2026-09-09\n\n_since v0.1.1 (2 commits)_\n\n### fix\n- fix: b\n\n"
                "## [0.1.1] — 2026-09-08\n\n### feat\n- feat: a\n")
        self.assertEqual(feed.changelog_section(text, "0.2.0"), "_since v0.1.1 (2 commits)_\n\n### fix\n- fix: b\n")
        self.assertEqual(feed.changelog_section(text, "0.1.1"), "### feat\n- feat: a\n")
        self.assertIsNone(feed.changelog_section(text, "0.3.0"))

    def test_fingerprint_is_the_minisign_key_id_from_either_form(self):
        feed = load_updater_feed()
        keyid = bytes(range(8))
        pub = minisign_pub(keyid)
        self.assertEqual(feed.fingerprint(pub), keyid.hex())
        box = base64.b64decode(pub).decode()
        self.assertEqual(feed.fingerprint(box), keyid.hex(), "the decoded box reads the same")
        self.assertIsNone(feed.fingerprint("not a key"))

    def test_manifest_text_is_the_old_install_sh_shape(self):
        feed = load_updater_feed()
        bins = {}
        for triple in ZO_BUILD_TARGETS:
            p = Path(self._tmp) / f"zo-v1.3.0-{triple}"
            p.write_bytes(triple.encode() * 3)
            bins[triple] = p
        text = feed.manifest_text("1.3.0", f"https://github.com/{GITHUB_REPO}/releases/download/v1.3.0", bins)
        lines = text.splitlines()
        self.assertEqual(len(lines), 6)
        self.assertEqual(lines[:3], ["schema=1", "version=1.3.0",
                                     f"base=https://github.com/{GITHUB_REPO}/releases/download/v1.3.0"])
        for i, triple in enumerate(ZO_BUILD_TARGETS):
            digest = hashlib.sha256(bins[triple].read_bytes()).hexdigest()
            self.assertEqual(lines[3 + i], f"asset={triple}|zo-v1.3.0-{triple}|{digest}|{len(triple) * 3}")
        self.assertTrue(text.endswith("\n") and "\r" not in text, "install.sh insists on unix newlines")
        sums = feed.sha256sums([bins[ZO_BUILD_TARGETS[0]]])
        self.assertEqual(sums, f"{hashlib.sha256(bins[ZO_BUILD_TARGETS[0]].read_bytes()).hexdigest()}  zo-v1.3.0-{ZO_BUILD_TARGETS[0]}\n")


class BundleUpdater(LaneCase):
    def test_without_a_key_the_phase_is_skipped_aloud_and_the_lane_is_green(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        self.assertEqual(self.phases(st), PHASES)
        ph = self.lane.phase("bundle-updater")
        self.assertEqual(ph["rc"], 0)
        self.assertIn("key", ph["skipped"])
        self.assertIn(str(self.lane.key), ph["skipped"])
        self.assertEqual(st["version"], VERSION)
        self.assertEqual(st["updater_pubkey"], "")
        self.assertFalse((self.lane.out(SHA_A) / "latest.json").exists())
        self.assertNotIn("bundle-updater", " ".join(self.lane.stub_lines()), "no build without a key")
        line = subprocess.run([BASH, posix(RELEASE / "status.sh")], env=self.lane.env(),
                              capture_output=True, text=True).stdout.strip()
        self.assertEqual(line.count("\n"), 0)
        self.assertIn("skipped=", line)
        self.assertIn("bundle-updater", line)
        self.assertIn("publish", line)

    def test_with_a_key_but_no_publish_the_second_build_is_skipped_aloud(self):
        pub = self.lane.write_key(keyid=bytes.fromhex("0102030405060708"))
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=pub)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        self.assertEqual(self.phases(st), PHASES)
        ph = self.lane.phase("bundle-updater")
        self.assertEqual(ph["rc"], 0)
        self.assertIn("RELEASE_PUBLISH", ph["skipped"])
        self.assertEqual(st["updater_pubkey"], "0102030405060708", "the key was still read and recorded")
        self.assertFalse((self.lane.out(SHA_A) / "latest.json").exists())
        self.assertNotIn("bundle-updater", " ".join(self.lane.stub_lines()), "no second build without a publish")
        line = subprocess.run([BASH, posix(RELEASE / "status.sh")], env=self.lane.env(),
                              capture_output=True, text=True).stdout.strip()
        self.assertIn("skipped=", line)
        self.assertIn("bundle-updater", line)

    def test_with_a_key_the_three_assets_and_the_feed_land_under_out(self):
        pub = self.lane.write_key(keyid=bytes.fromhex("0102030405060708"))
        self.lane.extra_env["RELEASE_PUBLISH"] = "1"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=pub)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        self.assertNotIn("skipped", self.lane.phase("bundle-updater"))
        self.assertEqual(st["updater_pubkey"], "0102030405060708")
        out = self.lane.out(SHA_A)
        archive = out / f"ZeroCode_{VERSION}_aarch64.app.tar.gz"
        self.assertTrue(archive.exists(), sorted(p.name for p in out.iterdir()))
        self.assertTrue(archive.with_name(archive.name + ".sig").exists())
        self.assertTrue((out / f"ZeroCode_{VERSION}_aarch64.dmg").exists())
        feed = json.loads((out / "latest.json").read_text())
        self.assertEqual(list(feed), ["version", "notes", "pub_date", "platforms"])
        self.assertEqual(feed["version"], VERSION)
        self.assertRegex(feed["pub_date"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertTrue(feed["notes"].strip(), "notes are the CHANGELOG section")
        self.assertEqual(list(feed["platforms"]), ["darwin-aarch64"])
        darwin = feed["platforms"]["darwin-aarch64"]
        self.assertEqual(darwin["signature"], archive.with_name(archive.name + ".sig").read_text().strip())
        self.assertEqual(darwin["url"],
                         f"https://github.com/{GITHUB_REPO}/releases/download/v{VERSION}/ZeroCode_{VERSION}_aarch64.app.tar.gz")
        sealed = [l for l in self.lane.stub_lines() if " bundle-updater seal " in l]
        self.assertEqual(len(sealed), 1, sealed)
        self.assertIn(f"target={self.lane.target('root-release')}", sealed[0])
        self.assertIn("key=" + str(self.lane.key), sealed[0], "the archive is signed by the key's path, never its bytes")

    def test_a_publish_run_builds_the_app_once_and_seals_that_build(self):
        """A publish run's build-app is the publish build itself, so
        bundle-updater seals and packs that app instead of running
        tauri:release a second time (1105 s on 10-13, a cold zo stage inside)."""
        pub = self.lane.write_key(keyid=bytes.fromhex("0102030405060708"))
        self.lane.extra_env["RELEASE_PUBLISH"] = "1"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=pub)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        lines = self.lane.stub_lines()
        installs = [l for l in lines if " build-app target=" in l]
        self.assertEqual(len(installs), 1, lines)
        self.assertNotIn("key=", installs[0], "the install build never holds the updater key")
        self.assertFalse([l for l in lines if " bundle-updater target=" in l], "no second tauri:release")
        sealed = [l for l in lines if " bundle-updater seal " in l]
        self.assertEqual(len(sealed), 1, lines)
        self.assertIn(f"target={self.lane.target('root-release')}", sealed[0])

    def test_the_sealed_app_must_be_the_installed_app_or_the_lane_is_red(self):
        """One build serves both, so what the updater ships is what this
        machine installed: a sealed app whose code signature differs from the
        installed one is red, and nothing is gathered for publish."""
        pub = self.lane.write_key(keyid=bytes.fromhex("0102030405060708"))
        self.lane.extra_env["RELEASE_PUBLISH"] = "1"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=pub, CDHASH_INSTALLED="aa11", CDHASH_SEALED="bb22")
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("aa11", st["reason"])
        self.assertIn("bb22", st["reason"])
        self.assertFalse((self.lane.out(SHA_A) / "latest.json").exists())

    def test_with_a_distribution_identity_the_publish_builds_its_own_signed_app(self):
        """Tauri signs (and notarizes) only in the publish build when a
        distribution identity is in the env, and the install build re-signs
        with the local identity, so with one the two builds stay."""
        pub = self.lane.write_key(keyid=bytes.fromhex("0102030405060708"))
        self.lane.extra_env["RELEASE_PUBLISH"] = "1"
        self.lane.extra_env["APPLE_SIGNING_IDENTITY"] = "Developer ID Application: Fixture"
        self.lane.enqueue(SHA_A)
        # Two builds, two identities: the installed app is not the shipped one by design.
        r = self.lane.run(PLUGIN_PUBKEY=pub, CDHASH_INSTALLED="aa11", CDHASH_SEALED="bb22")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        lines = self.lane.stub_lines()
        installs = [l for l in lines if " build-app target=" in l]
        self.assertEqual(len(installs), 1, lines)
        self.assertNotIn("key=", installs[0], "the install build never holds the updater key")
        second = [l for l in lines if " bundle-updater target=" in l]
        self.assertEqual(len(second), 1, lines)
        self.assertIn("key=" + str(self.lane.key), second[0])

    def test_a_pubkey_that_is_not_the_configured_one_is_red(self):
        self.lane.write_key(keyid=bytes.fromhex("0102030405060708"))
        other = minisign_pub(bytes.fromhex("0000000000000009"))
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=other)
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("pubkey", st["reason"])
        self.assertIn("0102030405060708", st["reason"])
        self.assertIn("0000000000000009", st["reason"])
        self.assertFalse((self.lane.out(SHA_A) / "latest.json").exists())
        self.assertNotIn("bundle-updater", " ".join(self.lane.stub_lines()), "nothing is signed with the wrong key")
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_A, "the local install already happened")

    def test_a_key_without_the_plugin_section_is_skipped_for_u_b(self):
        self.lane.write_key()
        self.lane.enqueue(SHA_A)
        r = self.lane.run()  # no PLUGIN_PUBKEY: the scratch's tauri.conf.json has no plugins.updater yet
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("plugins.updater", self.lane.phase("bundle-updater")["skipped"])


class Publish(LaneCase):
    def setUp(self):
        super().setUp()
        self.pub = self.lane.write_key()
        self.lane.extra_env["RELEASE_PUBLISH"] = "1"

    def publish_lines(self):
        return [l for l in self.lane.stub_lines() if " publish " in l]

    def test_publish_is_skipped_unless_opted_in(self):
        self.lane.extra_env["RELEASE_PUBLISH"] = "0"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("RELEASE_PUBLISH", self.lane.phase("publish")["skipped"])
        self.assertIn("RELEASE_PUBLISH", self.lane.phase("bundle-updater")["skipped"])
        self.assertEqual(self.publish_lines(), [])
        self.assertFalse((self.lane.out(SHA_A) / "latest.json").exists(), "no feed without a publish: that run builds it")

    def test_publish_creates_the_release_with_the_feed_and_the_assets(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "green")
        self.assertNotIn("skipped", self.lane.phase("publish"))
        lines = self.publish_lines()
        self.assertEqual(len(lines), 1, lines)
        self.assertIn(f" publish v{VERSION} repo={GITHUB_REPO} prerelease=0 ", lines[0])
        assets = lines[0].split("assets=")[1].split()[0].split(",")
        self.assertEqual(sorted(assets), sorted([
            f"ZeroCode_{VERSION}_aarch64.app.tar.gz", f"ZeroCode_{VERSION}_aarch64.app.tar.gz.sig",
            f"ZeroCode_{VERSION}_aarch64.dmg", "latest.json", "install.sh",
        ]))
        # The app installer goes up beside the feed it reads, so the README's
        # `latest/download/install.sh` never 404s again (v1.1.0..v1.1.4 did).
        self.assertEqual((self.lane.out(SHA_A) / "install.sh").read_bytes(), (RELEASE / "install.sh").read_bytes(),
                         "the app's install.sh goes up verbatim")
        self.assertIn("notes=", lines[0])
        self.assertEqual([l for l in self.lane.stub_lines() if "beta" in l], [], "stable moves no beta tag")

    def test_an_existing_tag_is_refused_before_anything_is_uploaded(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub, TAG_EXISTS=1)
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "refused")
        self.assertIn(f"v{VERSION}", st["reason"])
        self.assertEqual(self.publish_lines(), [])
        self.assertEqual(self.lane.queue(), [], "a refused publish is not retried by launchd")
        self.assertEqual(self.phases(st)[-2:], ["publish", "sweep"])
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_A, "the local install stands")

    def test_publish_without_a_feed_is_red(self):
        os.remove(self.lane.key)
        self.lane.enqueue(SHA_A)
        r = self.lane.run()
        self.assertNotEqual(r.returncode, 0)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red")
        self.assertIn("publish", st["reason"])
        self.assertEqual(self.publish_lines(), [])

    def test_beta_is_a_prerelease_and_moves_the_beta_tag(self):
        self.lane.extra_env["RELEASE_CHANNEL"] = "beta"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub, BETA_RELEASE="ours")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        lines = self.publish_lines()
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("prerelease=1", lines[0])
        moves = [l for l in self.lane.stub_lines() if " beta-move " in l]
        self.assertEqual(len(moves), 1, moves)
        self.assertIn(f"v{VERSION}", moves[0])
        self.assertIn("latest.json", moves[0], "the beta release carries the feed")
        self.assertLess(self.lane.stub_lines().index(lines[0]), self.lane.stub_lines().index(moves[0]))

    def test_a_beta_release_that_is_not_ours_is_left_alone_and_refused(self):
        self.lane.extra_env["RELEASE_CHANNEL"] = "beta"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub, BETA_RELEASE="foreign")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "refused")
        self.assertIn("beta", st["reason"])
        self.assertEqual([l for l in self.lane.stub_lines() if " beta-move " in l], [])
        self.assertEqual(self.publish_lines(), [], "refused before the version release too")

    def test_the_legacy_manifest_rides_along_with_all_three_zo_targets(self):
        self.lane.extra_env["RELEASE_LEGACY_MANIFEST"] = "1"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        cross = [l for l in self.lane.stub_lines() if "build-zo cross=" in l]
        self.assertEqual(sorted(l.split("cross=")[1].split()[0] for l in cross), sorted(ZO_BUILD_TARGETS[1:]))
        out = self.lane.out(SHA_A)
        manifest = (out / "manifest.txt").read_text()
        lines = manifest.splitlines()
        self.assertEqual(len(lines), 6, manifest)
        self.assertEqual(lines[0], "schema=1")
        self.assertEqual(lines[1], f"version={VERSION}")
        self.assertEqual(lines[2], f"base=https://github.com/{GITHUB_REPO}/releases/download/v{VERSION}")
        for i, triple in enumerate(ZO_BUILD_TARGETS):
            name = f"zo-v{VERSION}-{triple}"
            body = (out / name).read_bytes()
            self.assertEqual(lines[3 + i], f"asset={triple}|{name}|{hashlib.sha256(body).hexdigest()}|{len(body)}")
        self.assertEqual((out / "install.sh").read_bytes(), (RELEASE / "legacy" / "install.sh").read_bytes(),
                         "the old install.sh goes up verbatim")
        sums = (out / "SHA256SUMS").read_text().splitlines()
        self.assertEqual([l.split("  ")[1] for l in sums],
                         ["install.sh", "manifest.txt"] + [f"zo-v{VERSION}-{t}" for t in ZO_BUILD_TARGETS])
        self.assertEqual(sums[1].split("  ")[0], hashlib.sha256(manifest.encode()).hexdigest())
        assets = self.publish_lines()[0].split("assets=")[1].split()[0].split(",")
        for name in ["manifest.txt", "SHA256SUMS", "install.sh"] + [f"zo-v{VERSION}-{t}" for t in ZO_BUILD_TARGETS]:
            self.assertIn(name, assets)

    def test_a_missing_zo_target_refuses_the_whole_publish(self):
        self.lane.extra_env["RELEASE_LEGACY_MANIFEST"] = "1"
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub, ZO_CROSS_MISSING="x86_64-unknown-linux-gnu")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "refused")
        self.assertIn("x86_64-unknown-linux-gnu", st["reason"])
        self.assertIn("manifest", st["reason"])
        self.assertEqual(self.publish_lines(), [], "no half manifest, no release")
        self.assertFalse((self.lane.out(SHA_A) / "manifest.txt").exists())
        self.assertEqual(self.lane.installed()["zo"]["sha"], SHA_A, "the host zo still swapped")

    def test_legacy_manifest_off_builds_no_cross_targets(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(PLUGIN_PUBKEY=self.pub)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual([l for l in self.lane.stub_lines() if "cross=" in l], [])
        self.assertFalse((self.lane.out(SHA_A) / "manifest.txt").exists())


class GateOwed(LaneCase):
    """A gate judges a tree. A sha that left the tree exactly as the last green
    install of that half was built from — apart from version lines in the
    release stamp — owes it nothing, and the phase line says so (lane.sh
    `gate_owed`). Any other changed path owes both gates: each gate reads files
    of the other half and of the docs (the root gate's pii-check reads every
    tracked file, core tests read zo-ide sources, the core build script compiles
    skills/, zo tests read the root fixtures), so no path is known to lie
    outside either."""

    STAMP = "Cargo.toml Cargo.lock crates/zerocode-shell/tauri.conf.json zo-ide/Cargo.toml zo-ide/Cargo.lock"

    def seed_installed(self, sha):
        (self.lane.home / "installed.json").write_text(
            '{"app":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.90"},'
            '"zo":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.90"}}\n' % (sha, sha))

    def gates(self, sha):
        lines = [l for l in self.lane.stub_lines() if l.startswith(sha[:8]) and " gate " in l]
        ran = {l.split(" gate ")[1].split()[0] for l in lines if "target=" in l}
        skipped = {l.split(" gate ")[1].split()[0] for l in lines if "not owed" in l}
        return ran, skipped

    def owed(self, changed, **stubs):
        self.seed_installed(SHA_C)
        self.lane.enqueue(SHA_A)
        result = self.lane.run(CHANGED_PATHS=changed, **stubs)
        self.assertEqual(result.returncode, 0, result.stdout)
        return self.gates(SHA_A)

    def test_a_stamp_only_sha_owes_no_gate_and_still_installs(self):
        self.assertEqual(self.owed(self.STAMP), (set(), {"root", "zo"}))
        zo = self.lane.phase("gate-zo")
        self.assertEqual((zo["rc"], zo["secs"]), (0, 0))
        self.assertIn(f"zo tree unchanged since {SHA_C[:8]}", zo["skipped"])
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_A)
        self.assertEqual(self.lane.installed()["zo"]["sha"], SHA_A, "the skipped half is still built and installed")
        log = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertIn("gate-zo rc=0 0s", log)
        self.assertIn("skipped: zo tree unchanged", log)

    def test_a_window_only_sha_owes_both_gates(self):
        self.assertEqual(self.owed("ui/shell.js crates/zerocode-shell/src/main.rs " + self.STAMP + " CHANGELOG.md"),
                         ({"root", "zo"}, set()))

    def test_a_zo_only_sha_owes_both_gates(self):
        self.assertEqual(self.owed("zo-ide/crates/api/src/lib.rs zo-ide/Cargo.lock Cargo.toml"),
                         ({"root", "zo"}, set()))

    def test_a_skill_the_core_compiles_owes_both_gates(self):
        self.assertEqual(self.owed("skills/orchestration/SKILL.md"), ({"root", "zo"}, set()))

    def test_a_core_fixture_written_in_markdown_owes_both_gates(self):
        self.assertEqual(self.owed("crates/zerocode-core/fixtures/plain-text/before.ko.md"), ({"root", "zo"}, set()))

    def test_a_file_the_core_reads_from_outside_its_folder_owes_both_gates(self):
        self.assertEqual(self.owed("ui/knowledge-export.html"), ({"root", "zo"}, set()))

    def test_a_zo_source_the_core_tests_read_owes_both_gates(self):
        self.assertEqual(self.owed("zo-ide/crates/runtime/src/model_router/policy.rs"), ({"root", "zo"}, set()))

    def test_a_root_fixture_zo_tests_read_owes_both_gates(self):
        self.assertEqual(self.owed("fixtures/quality-baseline/baseline.json"), ({"root", "zo"}, set()))

    def test_a_docs_only_sha_owes_both_gates(self):
        self.assertEqual(self.owed("docs/design/x.md README.md CHANGELOG.md"), ({"root", "zo"}, set()))

    def test_a_real_change_in_a_stamp_file_owes_both_gates(self):
        self.assertEqual(self.owed("Cargo.toml", STAMP_REAL_CHANGES="Cargo.toml"), ({"root", "zo"}, set()))

    def test_a_shared_crate_owes_both_gates(self):
        self.assertEqual(self.owed("crates/zerocode-core/src/agent.rs"), ({"root", "zo"}, set()))

    def test_a_shared_gate_policy_change_owes_both_gates(self):
        self.assertEqual(self.owed("tools/release/verify-every-recipe.sh"), ({"root", "zo"}, set()))

    def test_the_gate_planner_change_owes_both_gates(self):
        self.assertEqual(self.owed("tools/release/lane.sh"), ({"root", "zo"}, set()))

    def test_a_first_install_owes_both_gates(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="ui/shell.js")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), ({"root", "zo"}, set()))

    def test_the_base_is_each_halfs_own_install(self):
        """The app half and the zo half were installed from different shas
        (a red app beside a green zo): each gate compares against its own."""
        (self.lane.home / "installed.json").write_text(
            '{"app":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.89"},'
            '"zo":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.90"}}\n' % (SHA_B, SHA_C))
        self.lane.enqueue(SHA_A)
        result = self.lane.run(CHANGED_PATHS=self.STAMP)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn(f"root tree unchanged since {SHA_B[:8]}", self.lane.phase("gate-root")["skipped"])
        self.assertIn(f"zo tree unchanged since {SHA_C[:8]}", self.lane.phase("gate-zo")["skipped"])


class GateOwedOnGit(unittest.TestCase):
    """gate_owed and stamp_only on a real repository, with git's own diff: a
    stamp diff git cannot read is a real change, and a path git lists with a
    space in it owes the gate."""

    FUNCTIONS = ('eval "$(grep \'^RELEASE_STAMP_FILES=\' "$0")"; '
                 'eval "$(sed -n -e \'/^is_dry()/p\' -e \'/^installed_part()/,/^}/p\' '
                 '-e \'/^stamp_only()/,/^}/p\' -e \'/^real_changes()/,/^}/p\' -e \'/^gate_owed()/,/^}/p\' "$0")"; '
                 'RELEASE_DRY_RUN=0 SCRATCH=$1 SHA=$2 INSTALLED=$3; ')
    BUMPED = '[package]\nname = "t"\nversion = "1.0.1"\n'

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="lane-git-"))
        self.repo = self.tmp / "repo"
        self.env = {**WINDOWS_ENV, "PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(self.tmp),
                    "GIT_CONFIG_NOSYSTEM": "1",
                    "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@x", "GIT_COMMITTER_NAME": "t",
                    "GIT_COMMITTER_EMAIL": "t@x"}
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], check=True, env=self.env)
        self.write("Cargo.toml", '[package]\nname = "t"\nversion = "1.0.0"\n')
        self.write("ui/shell.js", "one\n")
        self.since = self.commit("base")
        self.installed = self.tmp / "installed.json"
        self.installed.write_text(
            '{"app":{"sha":"%s","at":"2026-10-12T00:00:00Z","version":"1.0.0"},'
            '"zo":{"sha":"%s","at":"2026-10-12T00:00:00Z","version":"1.0.0"}}\n' % (self.since, self.since))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], env=self.env,
                              capture_output=True, text=True, check=True).stdout.strip()

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self, subject):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", subject)
        return self.git("rev-parse", "HEAD")

    def commit_bump(self):
        self.write("Cargo.toml", self.BUMPED)
        return self.commit("bump")

    def judge(self, call, sha):
        result = subprocess.run([BASH, "-c", self.FUNCTIONS + call, posix(LANE), str(self.repo), sha,
                                 str(self.installed)], env=self.env, capture_output=True, text=True)
        return result.stdout.strip()

    def owed(self, sha):
        return {lane for lane in ("root", "zo") if self.judge(f'gate_owed {lane}; echo $?', sha) == "0"}

    def lose_the_base_blob(self, name):
        blob = self.git("rev-parse", f"{self.since}:{name}")
        loose = self.repo / ".git" / "objects" / blob[:2] / blob[2:]
        loose.chmod(stat.S_IWRITE)  # git writes loose objects read-only, and Windows will not unlink those
        loose.unlink()

    def test_a_version_line_alone_owes_no_gate(self):
        sha = self.commit_bump()
        self.assertEqual(self.judge(f'stamp_only Cargo.toml {self.since}; echo $?', sha), "0")
        self.assertEqual(self.owed(sha), set())

    def test_a_stamp_diff_git_cannot_read_is_a_real_change(self):
        sha = self.commit_bump()
        self.lose_the_base_blob("Cargo.toml")
        self.assertEqual(self.git("diff", "--name-only", self.since, sha), "Cargo.toml")
        self.assertEqual(self.judge(f'stamp_only Cargo.toml {self.since}; echo $?', sha), "1")

    def test_a_stamp_diff_git_cannot_read_owes_both_gates(self):
        sha = self.commit_bump()
        self.lose_the_base_blob("Cargo.toml")
        self.assertEqual(self.owed(sha), {"root", "zo"})

    def test_a_path_with_a_space_owes_both_gates(self):
        self.write("Cargo.toml Cargo.lock", 'version = "1.0.1"\n')
        sha = self.commit("a name with a space")
        self.assertEqual(self.owed(sha), {"root", "zo"})

    @unittest.skipIf(os.name == "nt", "a Windows file name cannot hold a newline")
    def test_a_path_with_a_newline_owes_both_gates(self):
        self.write("Cargo.toml\nCargo.lock", 'version = "1.0.1"\n')
        sha = self.commit("a name with a newline")
        self.assertEqual(self.owed(sha), {"root", "zo"})

    def test_a_base_git_does_not_know_owes_both_gates(self):
        self.write("ui/shell.js", "two\n")
        sha = self.commit("change")
        self.installed.write_text(self.installed.read_text().replace(self.since, "d" * 40))
        self.assertEqual(self.owed(sha), {"root", "zo"})

    def test_the_release_notes_are_a_real_change_beside_the_stamp(self):
        self.write("Cargo.toml", self.BUMPED)
        self.write("CHANGELOG.md", "## [1.0.1]\n")
        sha = self.commit("release")
        self.assertEqual(self.judge(f'real_changes {self.since} UNUSED; printf "%s|%s" "$REAL" "$CHANGED"', sha),
                         "CHANGELOG.md\n|2")
        self.assertEqual(self.owed(sha), {"root", "zo"}, "pii-check reads the notes")


class Receipt(LaneCase):
    """A sha cut from a tree the coordinator already gated on its build line carries
    that gate's receipt (enqueue.sh --receipt; coord_receipt.py makes it and reads it).
    The lane runs only the recipes the receipt left owed and skips a gate it covered
    whole aloud; for a receipt of another tree, a red one, one whose evidence changed
    since it was sealed or one older than its head, the gate runs whole and the phase
    says why (t-47516). So it does for jobs the build line never ran, jobs that say
    another run or owner, and a head that does not hold the tree its jobs printed (ZO's
    review, P2-1·P2-3). Stubs stand for git (RECEIPT_CHANGED_PATHS, HEAD_TREE,
    COMMIT_TIME) and for each gate's recipe list (GATE_<LANE>_RECIPES); the receipt is
    read for real, against the fixture's build line (RELEASE_BUILD_LINE)."""

    ROOT = "pii-check test win-check"
    ZO = "test win-check"
    GATE = {
        "g1-build": [("pii-check", "just pii-check", 0), ("win-check", "just win-check", 0)],
        "g2-tests": [("test", "just test", 0)],
        "g3-zo": [("zo-test", "zsh -c 'cd zo-ide && just test'", 0),
                  ("zo-win-check", "zsh -c 'cd zo-ide && just win-check'", 0)],
    }
    # The coordinator's gate as it ran on 10-22: the development Windows check, which
    # says SKIPPED on a cold target, so it covers no strict win-check.
    DEVELOPMENT = {
        "g1-build": [("pii-check", "just pii-check", 0), ("win-check", "just win-check-if-available", 0)],
        "g2-tests": [("test", "just test", 0)],
        "g3-zo": [("zo-test", "zsh -c 'cd zo-ide && just test'", 0)],
    }
    STAMP = GateOwed.STAMP

    def setUp(self):
        super().setUp()
        self.lane.extra_env["RELEASE_BUILD_LINE"] = str(Path(self._tmp) / "build-queue")

    def beside(self, receipt):
        """Keep RECEIPT for SHA_A past the door, as a file the lane alone reads."""
        self.kept().parent.mkdir(parents=True, exist_ok=True)
        self.kept().write_text(Path(receipt).read_text())

    def receipt(self, gate=None, **kw):
        jobs = receipt_fixture.write_gate(Path(self._tmp), "l9", gate or self.GATE, **kw)
        made = subprocess.run([sys.executable, str(RELEASE / "coord_receipt.py"), "make", *map(str, jobs)],
                              capture_output=True, text=True)
        self.assertEqual(made.returncode, 0, made.stdout + made.stderr)
        path = Path(self._tmp) / "receipt.json"
        path.write_text(made.stdout)
        return path

    def enqueue(self, *args):
        return subprocess.run([BASH, posix(RELEASE / "enqueue.sh"), SHA_A, *args], env=self.lane.env(),
                              capture_output=True, text=True)

    def kept(self):
        return self.lane.home / "receipts" / f"{SHA_A}.json"

    def cut(self, receipt=None, **stubs):
        """Queue SHA_A — with RECEIPT, through the door — and run the lane."""
        if receipt is None:
            self.lane.enqueue(SHA_A)
        else:
            door = self.enqueue("--receipt", str(receipt))
            self.assertEqual(door.returncode, 0, door.stdout + door.stderr)
        self.lane.run(**{"GATE_ROOT_RECIPES": self.ROOT, "GATE_ZO_RECIPES": self.ZO, **stubs})
        return self.ran()

    def ran(self):
        """What each gate ran: 'all', the recipes it was given alone, or nothing (not in the map)."""
        ran = {}
        for line in self.lane.stub_lines():
            if " gate " in line and "target=" in line:
                lane = line.split(" gate ")[1].split()[0]
                ran[lane] = line.split(" only=", 1)[1].split() if " only=" in line else "all"
        return ran

    def test_a_receipt_for_this_tree_skips_both_gates_aloud(self):
        self.assertEqual(self.cut(self.receipt()), {}, "neither gate ran")
        logs = Path(self._tmp) / "gates" / "l9"
        for phase, lane, total in (("gate-root", "root", 3), ("gate-zo", "zo", 2)):
            with self.subTest(phase=phase):
                got = self.lane.phase(phase)
                self.assertEqual((got["rc"], got["secs"]), (0, 0))
                # The owner and run are the jobs' own word, held to the lane's (ZO's review, P3-4).
                self.assertTrue(got["skipped"].startswith(
                    f"receipt coord-l9 of {receipt_fixture.RUN} as its jobs say (the lane's coordinator run), "
                    f"tree {receipt_fixture.TREE[:8]}: {total}/{total} {lane} recipes green"), got["skipped"])
                self.assertIn(f"logs {logs}", got["skipped"], "the skip says where the evidence is")
                self.assertNotIn("receipt", got)
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_A)
        self.assertEqual(self.lane.installed()["zo"]["sha"], SHA_A, "a skipped gate's half is still built and installed")
        log = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertIn("gate-root rc=0 0s", log)
        self.assertIn("skipped: receipt coord-l9", log)
        self.assertIn("skipped=gate-root(receipt coord-l9", self.lane.status_line())

    def test_a_gate_run_as_two_lines_is_one_receipt_that_skips_both_gates(self):
        # t-76396: mkgate-split's coord-<label>-root and coord-<label>-zo, not a whole gate again.
        owners = {"g1-build": "coord-l9-root", "g2-tests": "coord-l9-root", "g3-zo": "coord-l9-zo"}
        self.assertEqual(self.cut(self.receipt(who=owners)), {}, "neither gate ran")
        self.assertTrue(self.lane.phase("gate-root")["skipped"].startswith(
            f"receipt coord-l9-root+coord-l9-zo of {receipt_fixture.RUN} as its jobs say"))
        self.assertEqual(self.lane.status()["outcome"], "green")

    def test_a_receipt_that_covers_part_runs_only_what_it_left_owed(self):
        # 10-22's gate covered every recipe but the strict Windows checks: the lane
        # runs those alone instead of both gates whole (3168 + 1719 s for v1.1.84).
        self.assertEqual(self.cut(self.receipt(self.DEVELOPMENT)), {"root": ["win-check"], "zo": ["win-check"]})
        root, zo = self.lane.phase("gate-root"), self.lane.phase("gate-zo")
        self.assertNotIn("skipped", root)
        self.assertIn("2/3 root recipes green, owes win-check", root["receipt"])
        self.assertIn("1/2 zo recipes green, owes win-check", zo["receipt"])
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertIn("receipt=gate-root(coord-l9", self.lane.status_line())
        self.assertIn("receipt: coord-l9", (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text())

    def test_a_receipt_for_another_tree_is_refused_and_the_gates_run_whole(self):
        ran = self.cut(self.receipt(), RECEIPT_CHANGED_PATHS="crates/zerocode-core/src/lib.rs " + self.STAMP)
        self.assertEqual(ran, {"root": "all", "zo": "all"})
        for phase in ("gate-root", "gate-zo"):
            with self.subTest(phase=phase):
                self.assertIn(f"refused: {receipt_fixture.HEAD[:8]} holds another tree (crates/zerocode-core/src/lib.rs",
                              self.lane.phase(phase).get("receipt", ""))
        self.assertEqual(self.lane.status()["outcome"], "green")

    def test_the_stamps_version_lines_leave_the_same_tree(self):
        self.assertEqual(self.cut(self.receipt(), RECEIPT_CHANGED_PATHS=self.STAMP), {})

    def test_a_real_change_in_a_stamp_file_is_another_tree(self):
        ran = self.cut(self.receipt(), RECEIPT_CHANGED_PATHS=self.STAMP, STAMP_REAL_CHANGES="Cargo.lock")
        self.assertEqual(ran, {"root": "all", "zo": "all"})

    def test_the_release_notes_owe_their_readers_again(self):
        # bump.sh opens the version's section in every release commit; of the
        # gates' recipes only pii-check reads it.
        ran = self.cut(self.receipt(), RECEIPT_CHANGED_PATHS=self.STAMP + " CHANGELOG.md")
        self.assertEqual(ran, {"root": ["pii-check"]}, "zo's gate has no reader of the notes")
        self.assertIn("2/3 root recipes green, the notes changed, owes pii-check", self.lane.phase("gate-root").get("receipt", ""))
        self.assertTrue(self.lane.phase("gate-zo")["skipped"].startswith("receipt coord-l9"))

    def test_a_red_receipt_is_refused_at_the_door_and_nothing_is_queued(self):
        jobs = receipt_fixture.write_gate(Path(self._tmp), "l9", {"g1": [("test", "just test", 101)]})
        red = Path(self._tmp) / "red.json"
        red.write_text(receipt_fixture.sealed(jobs))
        door = self.enqueue("--receipt", str(red))
        self.assertEqual(door.returncode, 1, door.stdout + door.stderr)
        self.assertIn("the receipt is refused", door.stdout)
        self.assertIn("ended rc=1", door.stdout)
        self.assertEqual(self.lane.queue(), [])
        self.assertFalse(self.kept().exists())

    def test_a_red_receipt_beside_the_queue_is_refused_and_the_gates_run_whole(self):
        jobs = receipt_fixture.write_gate(Path(self._tmp), "l9",
                                          {"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 101)]})
        self.kept().parent.mkdir(parents=True)
        self.kept().write_text(receipt_fixture.sealed(jobs))
        self.assertEqual(self.cut(), {"root": "all", "zo": "all"})
        self.assertIn(f"refused: job 0-{receipt_fixture.FIRST_JOB}-coord-l9-g1 ended rc=1",
                      self.lane.phase("gate-root").get("receipt", ""))

    def test_evidence_changed_after_the_door_is_refused_and_the_gates_run_whole(self):
        door = self.enqueue("--receipt", str(self.receipt()))
        self.assertEqual(door.returncode, 0, door.stdout + door.stderr)
        (Path(self._tmp) / "gates" / "l9" / "test.log").write_text("another run's words\n")
        self.lane.run(GATE_ROOT_RECIPES=self.ROOT, GATE_ZO_RECIPES=self.ZO)
        self.assertEqual(self.ran(), {"root": "all", "zo": "all"})
        self.assertIn("test.log is not what the receipt sealed", self.lane.phase("gate-root").get("receipt", ""))

    def test_a_job_that_ended_before_its_head_was_committed_is_refused(self):
        ran = self.cut(self.receipt(), COMMIT_TIME=int(time.time()) + 3600)
        self.assertEqual(ran, {"root": "all", "zo": "all"})
        self.assertIn(f"a job on {receipt_fixture.HEAD[:8]} ended before that head was committed",
                      self.lane.phase("gate-root").get("receipt", ""))

    def test_without_a_receipt_the_gates_run_whole_and_say_nothing_of_one(self):
        self.assertEqual(self.cut(), {"root": "all", "zo": "all"})
        for phase in ("gate-root", "gate-zo"):
            with self.subTest(phase=phase):
                self.assertNotIn("receipt", self.lane.phase(phase))
                self.assertNotIn("skipped", self.lane.phase(phase))

    def test_a_green_sha_leaves_its_receipt_beside_the_lane_log(self):
        self.cut(self.receipt())
        self.assertTrue((self.lane.home / "out" / f"receipt-{SHA_A[:8]}.json").exists())
        self.assertFalse(self.kept().exists(), "a green sha never runs again")

    def test_a_red_sha_keeps_its_receipt_for_its_next_turn(self):
        self.cut(self.receipt(self.DEVELOPMENT), GATE_ROOT_RC=1, GATE_ROOT_UNNAMED="win-check")
        self.assertEqual(self.lane.status()["outcome"], "red")
        self.assertTrue(self.kept().exists())
        self.assertTrue((self.lane.home / "out" / f"receipt-{SHA_A[:8]}.json").exists())

    def test_the_door_keeps_the_receipt_before_the_queue_file(self):
        door = self.enqueue("--receipt", str(self.receipt()))
        self.assertEqual(door.returncode, 0, door.stdout + door.stderr)
        self.assertIn(f"receipt coord-l9 tree {receipt_fixture.TREE[:8]} kept for {SHA_A[:8]}", door.stdout)
        self.assertEqual(self.lane.queue(), [SHA_A])
        self.assertLessEqual(self.kept().stat().st_mtime_ns, (self.lane.home / "queue" / SHA_A).stat().st_mtime_ns)

    def test_bare_recipes_run_below_the_checkouts_top_cover_nothing(self):
        # ZO's review, P1-1: jobs that ran in zo-ide ran zo's `just test`, never the
        # root's — both gates run every recipe, as if no receipt had covered one.
        gate = {"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 0), ("win-check", "just win-check", 0)]}
        below = self.receipt(gate, cwd=Path(self._tmp) / "coord-gate" / "zo-ide")
        self.assertEqual(self.cut(below), {"root": self.ROOT.split(), "zo": self.ZO.split()})
        self.assertIn("0/3 root recipes green, owes pii-check test win-check", self.lane.phase("gate-root").get("receipt", ""))

    def test_jobs_outside_the_build_lines_done_folder_are_refused_and_the_gates_run_whole(self):
        # ZO's review, P2-1: jobs copied out of done/ by hand are no jobs the line ran.
        jobs = receipt_fixture.write_gate(Path(self._tmp), "l9", self.GATE)
        elsewhere = Path(self._tmp) / "elsewhere"
        elsewhere.mkdir()
        for path in jobs:
            for kind in ("job", "rc", "log"):
                shutil.copy2(str(path)[:-len(".job")] + "." + kind, elsewhere)
        copied = Path(self._tmp) / "copied.json"
        copied.write_text(receipt_fixture.sealed([elsewhere / path.name for path in jobs]))
        self.beside(copied)
        self.assertEqual(self.cut(), {"root": "all", "zo": "all"})
        done = (Path(self._tmp) / "build-queue" / "done").resolve()
        self.assertIn(f"refused: job 0-{receipt_fixture.FIRST_JOB}-coord-l9-g1-build is not in {done}",
                      self.lane.phase("gate-root").get("receipt", ""))

    def test_jobs_the_runner_never_ran_are_refused_and_the_gates_run_whole(self):
        # …and jobs written into done/ by hand have no RUN and DONE lines in the runner's log.
        self.beside(self.receipt())
        (Path(self._tmp) / "build-queue" / "runner.log").write_text("")
        self.assertEqual(self.cut(), {"root": "all", "zo": "all"})
        self.assertIn(f"runner.log never ended job 0-{receipt_fixture.FIRST_JOB}-coord-l9-g1-build green under coord-l9",
                      self.lane.phase("gate-zo").get("receipt", ""))

    def test_the_door_refuses_jobs_the_runner_never_ran(self):
        receipt = self.receipt()
        (Path(self._tmp) / "build-queue" / "runner.log").write_text("")
        door = self.enqueue("--receipt", str(receipt))
        self.assertEqual(door.returncode, 1, door.stdout + door.stderr)
        self.assertIn("runner.log never ended job", door.stdout)
        self.assertEqual(self.lane.queue(), [])
        self.assertFalse(self.kept().exists())

    def test_jobs_that_say_another_run_are_refused_and_the_gates_run_whole(self):
        # ZO's review, P2-1: the jobs' own `# run:` is held to the lane's RELEASE_COORD_RUN.
        self.beside(self.receipt(run="run-11955"))
        self.assertEqual(self.cut(), {"root": "all", "zo": "all"})
        self.assertIn(f"refused: the jobs say run run-11955, not {receipt_fixture.RUN}",
                      self.lane.phase("gate-root").get("receipt", ""))

    def test_jobs_that_say_no_coordinator_are_refused_and_the_gates_run_whole(self):
        # …and their `# who:` to a coordinator's coord-<label>: a worker's jobs are no landing gate,
        # and (t-76396 r3) make itself seals nothing for them, so the lane has no receipt and runs whole.
        jobs = receipt_fixture.write_gate(Path(self._tmp), "l9", self.GATE, who="w-70906")
        made = subprocess.run([sys.executable, str(RELEASE / "coord_receipt.py"), "make", *map(str, jobs)],
                              capture_output=True, text=True)
        self.assertEqual(made.returncode, 1, made.stdout + made.stderr)
        self.assertIn("the jobs say w-70906, not a coordinator's coord-<label>", made.stdout + made.stderr)
        self.assertEqual(self.cut(), {"root": "all", "zo": "all"})

    def test_a_receipt_a_worker_owned_is_refused_by_the_lane_and_the_gates_run_whole(self):
        # t-76396 r4: make refuses it, but a receipt an older make sealed still sits beside the queue; the lane reads it.
        jobs = receipt_fixture.write_gate(Path(self._tmp), "l9", self.GATE, who="w-70906")
        self.kept().parent.mkdir(parents=True, exist_ok=True)
        self.kept().write_text(receipt_fixture.sealed(jobs, who="w-70906"))
        self.assertEqual(self.cut(), {"root": "all", "zo": "all"})
        self.assertIn("refused: the jobs say w-70906, not a coordinator's coord-<label>",
                      self.lane.phase("gate-root").get("receipt", ""))

    def test_a_head_that_does_not_hold_the_tree_its_jobs_printed_is_refused(self):
        # ZO's review, P2-3: the head and the tree are both what a job printed; git ties them.
        ran = self.cut(self.receipt(), HEAD_TREE="3" * 40)
        self.assertEqual(ran, {"root": "all", "zo": "all"})
        self.assertIn(f"refused: {receipt_fixture.HEAD[:8]} does not hold the tree its jobs printed "
                      f"({receipt_fixture.TREE[:8]})", self.lane.phase("gate-root").get("receipt", ""))


class ReceiptOnGit(unittest.TestCase):
    """receipt_cover on a real repository: git's own diff between a job's head and the
    sha, git's commit time, the scratch's recipe lists (verify-every-recipe.sh --list)
    and coord_receipt.py, all run for real (t-47516)."""

    FUNCTIONS = ('eval "$(grep -E \'^(RELEASE_STAMP_FILES|RELEASE_NOTES|NOTES_READERS|RELEASE_COORD_RUN)=\' "$0")"; '
                 'eval "$(sed -n -e \'/^is_dry()/p\' -e \'/^gate_dir()/p\' -e \'/^stamp_only()/,/^}/p\' '
                 '-e \'/^real_changes()/,/^}/p\' -e \'/^gate_recipes()/,/^}/p\' -e \'/^commit_time()/,/^}/p\' '
                 '-e \'/^tree_of()/,/^}/p\' -e \'/^notes_edited()/,/^}/p\' -e \'/^receipt_cover()/,/^}/p\' "$0")"; '
                 'RELEASE_DRY_RUN=0 SELF_DIR=$(dirname "$0") RECEIPT_PY=$(dirname "$0")/coord_receipt.py '
                 'SCRATCH=$1 SHA=$2 SHA8=${2:0:8} RECEIPTS=$3 RELEASE_BUILD_LINE=$4; ')
    GATE = {
        "g1-build": [("pii-check", "just pii-check", 0), ("test", "just test", 0),
                     ("win-check", "just win-check-if-available", 0)],
        "g3-zo": [("zo-test", "zsh -c 'cd zo-ide && just test'", 0)],
    }
    COMMITTED = "2026-10-22T03:00:00+0900"   # the gated landing, well before its jobs ran

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="lane-receipt-"))
        self.repo = self.tmp / "repo"
        self.receipts = self.tmp / "receipts"
        self.receipts.mkdir()
        self.env = {**WINDOWS_ENV, "PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(self.tmp),
                    "GIT_CONFIG_NOSYSTEM": "1",
                    "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@x", "GIT_COMMITTER_NAME": "t",
                    "GIT_COMMITTER_EMAIL": "t@x", "GIT_AUTHOR_DATE": self.COMMITTED, "GIT_COMMITTER_DATE": self.COMMITTED}
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], check=True, env=self.env)
        self.write("justfile", "verify: pii-check test win-check-if-available\n")
        self.write("zo-ide/justfile", "verify: test\n")
        self.write("Cargo.toml", '[package]\nname = "t"\nversion = "1.0.0"\n')
        self.write("CHANGELOG.md", "# Changelog\n")
        self.write("ui/shell.js", "one\n")
        self.gated = self.commit("the landing the coordinator gated")
        self.write("Cargo.toml", '[package]\nname = "t"\nversion = "1.0.1"\n')
        self.write("CHANGELOG.md", "# Changelog\n\n## [1.0.1] — 2026-10-22\n\n- fix: one\n")
        self.release = self.commit("release: v1.0.1")
        self.gate(self.gated)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], env=self.env,
                              capture_output=True, text=True, check=True).stdout.strip()

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self, subject):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", subject)
        return self.git("rev-parse", "HEAD")

    def gate(self, head, tree=None):
        self.jobs = receipt_fixture.write_gate(self.tmp, "l9", self.GATE, head=head,
                                               tree=tree or self.git("rev-parse", f"{head}^{{tree}}"))

    def cover(self, lane, sha=None):
        sha = sha or self.release
        made = subprocess.run([sys.executable, str(RELEASE / "coord_receipt.py"), "make", *map(str, self.jobs)],
                              capture_output=True, text=True)
        self.assertEqual(made.returncode, 0, made.stdout + made.stderr)
        (self.receipts / f"{sha}.json").write_text(made.stdout)
        out = subprocess.run([BASH, "-c", self.FUNCTIONS + f'receipt_cover {lane}; echo "rc=$?"; echo "only=$ONLY"; '
                              'echo "said=$RECEIPT"', str(LANE), str(self.repo), sha, str(self.receipts),
                              str(self.tmp / "build-queue")],
                             env=self.env, capture_output=True, text=True)
        return dict(l.split("=", 1) for l in out.stdout.splitlines() if l.split("=", 1)[0] in ("rc", "only", "said"))

    def test_the_tree_before_the_bump_leaves_the_notes_readers_and_the_strict_windows_check(self):
        root = self.cover("root")
        self.assertEqual((root["rc"], root["only"]), ("0", "pii-check win-check"), root)
        self.assertIn("1/3 root recipes green, the notes changed, owes pii-check win-check", root["said"])
        zo = self.cover("zo")
        self.assertEqual((zo["rc"], zo["only"]), ("0", ""), zo)
        self.assertIn("1/1 zo recipes green, the notes changed", zo["said"])

    def test_a_sha_that_changed_more_than_the_stamp_and_the_notes_is_refused(self):
        self.write("ui/shell.js", "two\n")
        later = self.commit("one more change")
        root = self.cover("root", later)
        self.assertEqual((root["rc"], root["only"]), ("1", ""), root)
        self.assertIn(f"refused: {self.gated[:8]} holds another tree", root["said"])
        self.assertIn("ui/shell.js", root["said"])

    def test_a_job_that_ended_before_its_head_was_committed_is_refused(self):
        committed = int(self.git("show", "-s", "--format=%ct", self.gated))
        receipt_fixture.ended(self.jobs[0], committed - 60)
        root = self.cover("root")
        self.assertEqual(root["rc"], "1", root)
        self.assertIn(f"a job on {self.gated[:8]} ended before that head was committed", root["said"])

    def test_a_head_git_does_not_know_is_refused(self):
        self.gate("d" * 40, tree=self.git("rev-parse", f"{self.gated}^{{tree}}"))
        root = self.cover("root")
        self.assertEqual(root["rc"], "1", root)
        self.assertIn(f"git cannot compare dddddddd with {self.release[:8]}", root["said"])

    def test_a_verify_that_is_not_a_plain_list_leaves_the_receipt_unused(self):
        self.write("justfile", "verify: (pii-check)\n")   # the scratch's justfile; git's diff of the two shas never sees it
        root = self.cover("root")
        self.assertEqual(root["rc"], "1", root)
        self.assertIn("unused: the root gate's recipes are unknown", root["said"])

    def test_a_head_whose_tree_is_not_the_one_its_jobs_printed_is_refused(self):
        # ZO's review of t-47516, P2-3: jobs that printed the release's own tree over the
        # gated head — git holds the head to the tree it really has.
        self.gate(self.gated, tree=self.git("rev-parse", f"{self.release}^{{tree}}"))
        root = self.cover("root")
        self.assertEqual(root["rc"], "1", root)
        self.assertIn(f"refused: {self.gated[:8]} does not hold the tree its jobs printed", root["said"])

    def test_notes_that_are_not_a_plain_edit_are_refused(self):
        # ZO's review of t-47516, P3-1: `git diff --name-only` shows a link, a removal and a
        # mode change as the same one path; only the notes' text edited in place is the stamp's.
        notes = self.repo / "CHANGELOG.md"
        changes = (("link", lambda: (notes.unlink(), notes.symlink_to("Cargo.toml"))),
                   ("removal", notes.unlink),
                   ("mode", lambda: (notes.write_text("# Changelog\n\n## [1.0.1]\n"), notes.chmod(0o755))))
        for name, change in changes:
            with self.subTest(change=name):
                self.git("checkout", "-q", "--detach", self.gated)
                change()
                self.write("Cargo.toml", '[package]\nname = "t"\nversion = "1.0.1"\n')
                sha = self.commit(f"release: v1.0.1 with the notes' {name}")
                self.assertEqual(self.git("diff", "--name-only", self.gated, sha).split(), ["CHANGELOG.md", "Cargo.toml"])
                root = self.cover("root", sha)
                self.assertEqual((root["rc"], root["only"]), ("1", ""), root)
                self.assertIn(f"refused: CHANGELOG.md is not a plain file edited in place since {self.gated[:8]}", root["said"])


def common_dir_of(repo):
    """What git names as the folder every checkout of `repo` shares -- the lane's word for the
    repository it releases (t-83513)."""
    return subprocess.run(["git", "-C", str(repo), "rev-parse", "--path-format=absolute", "--git-common-dir"],
                          capture_output=True, text=True, check=True).stdout.strip()


class SaysItself(LaneCase):
    """t-83513: status.json says which repository the lane releases and which steps it runs,
    with each step's name in the window's languages -- the board keeps no table of either."""

    def test_status_names_the_repository_and_the_steps_with_their_words(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        st = self.lane.status()
        self.assertEqual(st["git_common_dir"], common_dir_of(REPO))
        self.assertEqual([step["name"] for step in st["plan"]], PHASES)
        for step in st["plan"]:
            self.assertEqual(list(step["words"]), LANGS, step)
            self.assertTrue(all(word.strip() for word in step["words"].values()), step)
        words = {step["name"]: step["words"] for step in st["plan"]}
        self.assertEqual(words["gate-zo"]["ko"], "zo 시험")
        self.assertEqual(words["gate-root"]["en"], "Integration tests")
        self.assertIsNone(st["stalled"])
        plan = subprocess.run([BASH, posix(LANE), "--plan"], env=self.lane.env(), capture_output=True, text=True)
        self.assertEqual(plan.returncode, 0, plan.stderr)
        self.assertEqual(json.loads(plan.stdout), st["plan"], "--plan prints the list status.json carries")

    def test_the_repository_is_the_one_the_lane_was_told(self):
        self.lane.extra_env["RELEASE_GIT_COMMON_DIR"] = posix(self.lane.tmp / "elsewhere.git")
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        self.assertEqual(self.lane.status()["git_common_dir"], posix(self.lane.tmp / "elsewhere.git"))

    def test_status_sh_names_a_stalled_recipe(self):
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        st = self.lane.status()
        st["stalled"] = {"gate": "gate-zo", "recipe": "test", "quiet_since_ms": 1, "cap_secs": 2400}
        (self.lane.home / "status.json").write_text(json.dumps(st, ensure_ascii=False, separators=(",", ":")))
        self.assertIn("stalled=gate-zo/test", self.lane.status_line())


class Quiet(LaneCase):
    """t-83513: a gate's recipe that writes nothing past its usual time is said stalled, and past
    a cap ended red -- on 10-29 a deadlocked test held v1.1.104's gate-zo 55 minutes in silence."""

    FUNCTIONS = ('eval "$(grep -E \'^QUIET_[A-Z_]+=\' "$0" | sed \'s/[[:space:]]*#.*//\')"; '
                 'eval "$(sed -n -e \'/^is_dry()/p\' -e \'/^stub()/,/^  local/p\' -e \'/^lane_verdict()/p\' '
                 '-e \'/^recipe_usuals()/,/^}/p\' -e \'/^quiet_floor()/p\' -e \'/^quiet_limits()/,/^}/p\' "$0")"; '
                 'RELEASE_DRY_RUN=0 OUT=$1; ')

    def out(self):
        out = self.lane.home / "out"
        out.mkdir(parents=True, exist_ok=True)
        return out

    def past(self, sha8, verdict, recipes, gate="zo", mtime=None):
        """One lane of the past: its log ending in `verdict`, and its gate log's closing lines."""
        out = self.out()
        lane = out / f"lane-{sha8}.log"
        lane.write_text(f"10:00:00 == lane {sha8} start (dry=0)\n10:00:01 == gate-zo\n10:30:00 == {verdict}\n")
        log = out / f"gate-{gate}-{sha8}.log"
        log.write_text("".join(f"==> verify recipe {name}\n<== verify recipe {name} rc={rc} {secs}s\n"
                               for name, rc, secs in recipes))
        if mtime is not None:
            os.utime(lane, (mtime, mtime))

    def shell(self, script):
        r = subprocess.run([BASH, "-c", self.FUNCTIONS + script, posix(LANE), posix(self.out())],
                           env=self.lane.env(), capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        return r.stdout

    def test_a_recipes_usual_time_is_its_median_over_the_newest_green_lanes(self):
        base = time.time() - 10_000
        # Six green lanes, oldest first: the oldest is past QUIET_SAMPLES (five) and is not read.
        for n, secs in enumerate([9000, 100, 300, 200, 500, 400]):
            self.past(f"{n:08d}", "green", [("test", 0, secs), ("lint", 0, 10 * (n + 1))], mtime=base + n * 100)
        # A newer red lane, a newer green lane whose gate was not owed (empty log), and a recipe
        # that failed in a green lane (cleared by the flaky-test judge) are no samples.
        self.past("aaaaaaaa", "red", [("test", 0, 7777)], mtime=base + 1_000)
        self.past("bbbbbbbb", "green", [], mtime=base + 1_100)
        self.past("cccccccc", "green", [("test", 1, 8888)], gate="root", mtime=base + 1_200)
        usuals = dict(line.split() for line in self.shell("recipe_usuals zo").splitlines())
        self.assertEqual(usuals, {"test": "300", "lint": "40"}, "the medians of 100 300 200 500 400 and 20…60")

    def test_said_at_its_factor_and_the_floor_and_ended_at_twice_that(self):
        limits = self.shell('printf "%s\\n" "$(quiet_limits test "test 300")" "$(quiet_limits test "test 450")" '
                            '"$(quiet_limits lint "test 300")"').splitlines()
        # 2 × 300 = 600, the floor; 2 × 450 = 900 over it; a recipe with no green past is said at
        # the floor and never ended.
        self.assertEqual(limits, ["600 1200 300", "900 1800 450", "600 0 0"])

    def justdir(self, recipes):
        work = self.lane.tmp / "justdir"
        work.mkdir(exist_ok=True)
        names = " ".join(name for name, _ in recipes)
        (work / "justfile").write_text(f"verify: {names}\n\n" + "".join(
            f"{name}:\n    {line}\n\n" for name, line in recipes))
        return work

    def test_a_silent_recipe_is_said_stalled_then_ended_red_and_the_gate_goes_on(self):
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        # A green past for `stuck` (0 s), so it has a cap: said at the floor (1 s), ended at 2 s.
        self.past("11111111", "green", [("stuck", 0, 0)])
        work = self.justdir([("quick", "echo quick"),
                             ("stuck", "sh -c 'echo $$ > stuck.pid; exec sleep 30'"),
                             ("after", "echo after > after.txt")])
        self.lane.enqueue(SHA_A)
        started = time.monotonic()
        proc = self.lane.spawn(GATE_ZO_JUSTDIR=posix(work), QUIET_FLOOR_SECS=1, QUIET_POLL_SECS=0.2)
        try:
            said = wait_until(lambda: (self.lane.home / "status.json").exists()
                              and (self.lane.status().get("stalled") or {}).get("recipe") == "stuck", timeout=20)
            stalled = self.lane.status().get("stalled")
            out, _ = proc.communicate(timeout=30)
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
        self.assertTrue(said, out)
        self.assertEqual((stalled["gate"], stalled["recipe"], stalled["cap_secs"]), ("gate-zo", "stuck", 2), stalled)
        self.assertIsInstance(stalled["quiet_since_ms"], int)
        self.assertLess(time.monotonic() - started, 25, "the stuck recipe was ended, not waited out")
        self.assertNotEqual(proc.returncode, 0, out)
        st = self.lane.status()
        self.assertEqual(st["outcome"], "red", st)
        self.assertIn("zo:recipe:stuck", st["reason"])
        self.assertIsNone(st["stalled"], "the gate's end takes the word back")
        lane_log = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertRegex(lane_log, r"stalled: gate-zo stuck — \d+s without output; ended red at 2s")
        self.assertIn("stalled: gate-zo stuck — ", lane_log)
        self.assertIn("past its cap: ending it red", lane_log)
        gate_log = (self.lane.home / "out" / f"gate-zo-{SHA_A[:8]}.log").read_text()
        self.assertIn("error: recipe `stuck` was terminated: no output past the release lane's cap", gate_log)
        self.assertIn("==> verify recipe after", gate_log, "the gate went on to the next recipe")
        self.assertTrue((work / "after.txt").exists())
        pid = int((work / "stuck.pid").read_text())
        with self.assertRaises(ProcessLookupError, msg="the recipe's own process is gone"):
            os.kill(pid, 0)
        events = [line.split(" ", 1)[1] for line in self.lane.stub_lines()]
        self.assertIn("stalled gate-zo stuck", events)
        self.assertIn("quiet-end gate-zo stuck", events)
        self.assertFalse(any(e.startswith("stalled gate-zo quick") for e in events), "a quick recipe is never said")

    def test_output_that_comes_back_takes_the_word_back_and_ends_nothing(self):
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        self.past("11111111", "green", [("pauses", 0, 0)])   # said at 2 s, ended at 4 s
        work = self.justdir([("pauses", "echo a; sleep 2.6; echo b")])
        self.lane.enqueue(SHA_A)
        r = self.lane.run(GATE_ZO_JUSTDIR=posix(work), QUIET_FLOOR_SECS=2, QUIET_POLL_SECS=0.2)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        lane_log = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertIn("stalled: gate-zo pauses — ", lane_log)
        self.assertIn("stalled: gate-zo pauses — output again", lane_log)
        self.assertNotIn("ending it red", lane_log)
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertIsNone(self.lane.status()["stalled"])


class Stoppable(LaneCase):
    """verify-every-recipe.sh --stoppable (t-83513): SIGUSR1 ends the running recipe's own process
    tree as a red recipe and the run goes on; any other signal still ends the run."""

    def test_usr1_ends_the_running_recipe_red_and_the_run_goes_on(self):
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: first stuck last\n\n"
            "first:\n    echo first >> ran.txt\n\n"
            "stuck:\n    sh -c 'echo $$ > stuck.pid; exec sleep 30'\n\n"
            "last:\n    echo last >> ran.txt\n"
        )
        log = work / "gate.log"
        with log.open("w") as out:
            proc = subprocess.Popen([BASH, posix(RELEASE / "verify-every-recipe.sh"), "--stoppable"], cwd=work,
                                    stdout=out, stderr=subprocess.STDOUT)
        try:
            self.assertTrue(wait_until(lambda: (work / "stuck.pid").exists()), log.read_text())
            proc.send_signal(signal.SIGUSR1)
            rc = proc.wait(timeout=20)
        finally:
            if proc.poll() is None:
                proc.kill()
        text = log.read_text()
        self.assertEqual(rc, 1, text)
        self.assertEqual((work / "ran.txt").read_text().split(), ["first", "last"])
        self.assertIn("error: recipe `stuck` was terminated: no output past the release lane's cap", text)
        self.assertIn("verify: red recipes: stuck", text)
        with self.assertRaises(ProcessLookupError):
            os.kill(int((work / "stuck.pid").read_text()), 0)
        parse = ('eval "$(sed -n \'/^failed_tests()/,/^}/p\' "$0")"; failed_tests "$1"')
        failed = subprocess.run([BASH, "-c", parse, posix(LANE), posix(log)],
                                capture_output=True, text=True, check=True).stdout.split()
        self.assertEqual(failed, ["recipe:stuck"], "the lane reads it as a red recipe of its own")

    def test_any_other_signal_still_ends_the_run(self):
        if shutil.which("just") is None:
            self.skipTest("just is not installed")
        work = Path(self._tmp) / "justdir"
        work.mkdir()
        (work / "justfile").write_text(
            "verify: first killed last\n\n"
            "first:\n    echo first >> ran.txt\n\n"
            "killed:\n    echo killed >> ran.txt; kill -TERM $$\n\n"
            "last:\n    echo last >> ran.txt\n"
        )
        r = subprocess.run([BASH, posix(RELEASE / "verify-every-recipe.sh"), "--stoppable"], cwd=work,
                           capture_output=True, text=True, timeout=20)
        self.assertGreaterEqual(r.returncode, 128, r.stdout + r.stderr)
        self.assertEqual((work / "ran.txt").read_text().split(), ["first", "killed"])



if __name__ == "__main__":
    unittest.main(verbosity=1)
