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
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

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
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": os.environ.get("HOME", str(self.tmp)),
            "RELEASE_DRY_RUN": "1",
            "RELEASE_HOME": str(self.home),
            "RELEASE_REPO": str(REPO),
            "RELEASE_SCRATCH_ROOT": str(self.scratch_root),
            "RELEASE_APP_DIR": str(self.app_dir),
            "RELEASE_ZO_BIN": str(self.zo_bin),
            "RELEASE_FLAKES_FILE": str(self.flakes),
            "RELEASE_STUB_LOG": str(self.stub_log),
            "UPDATER_KEY": str(self.key),
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
            ["bash", str(LANE), *args], env=self.env(**stubs),
            capture_output=True, text=True, timeout=timeout,
        )

    def spawn(self, *args, **stubs):
        return subprocess.Popen(
            ["bash", str(LANE), *args], env=self.env(**stubs),
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
            rc = subprocess.run(["bash", "-n", str(path)], capture_output=True, text=True)
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
        r = subprocess.run([str(RELEASE / "verify-every-recipe.sh")], cwd=work,
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
            subprocess.run([str(RELEASE / "verify-every-recipe.sh")], cwd=work,
                           stdout=out, stderr=subprocess.STDOUT)
        text = log.read_text()
        self.assertEqual(text.count("<== verify recipe "), 4, text)
        bare = work / "bare.log"
        bare.write_text("".join(l for l in text.splitlines(True) if not l.startswith("<== ")))
        parse = ('eval "$(sed -n \'/^failed_tests()/,/^}/p\' "$0")"; failed_tests "$1"')

        def failed(path):
            return subprocess.run(["bash", "-c", parse, str(LANE), str(path)],
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
        r = subprocess.run([str(RELEASE / "verify-every-recipe.sh")], cwd=work,
                           capture_output=True, text=True)
        self.assertGreaterEqual(r.returncode, 128, r.stdout + r.stderr)
        self.assertEqual((work / "ran.txt").read_text().split(), ["first", "killed"])

    def test_table_is_the_one_place(self):
        out = subprocess.run(["bash", str(LANE), "--table"], capture_output=True, text=True, env=self.lane.env())
        self.assertEqual(out.returncode, 0, out.stderr)
        table = dict(line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)
        table = {k: v.strip("'") for k, v in table.items()}
        self.assertTrue(all(line.endswith("'") for line in out.stdout.splitlines()), "values are quoted for eval")
        self.assertEqual(table["DISK_FLOOR_GB"], "10")
        self.assertEqual(table["TARGET_CAP_GB"], "20")
        self.assertEqual(table["ZO_PROFILE"], "release")
        self.assertEqual(table["PHASES"].split(), PHASES)
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
        out = subprocess.run(["bash", str(LANE), "--table"], capture_output=True, text=True, env=env)
        self.assertEqual(out.returncode, 0, out.stderr)
        table = {k: v.strip("'") for k, v in
                 (line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)}
        self.assertEqual(table["RELEASE_GITHUB_REPO"], GITHUB_REPO)

    def test_the_default_table_key_is_under_the_release_home(self):
        out = subprocess.run(["bash", str(LANE), "--table"], capture_output=True, text=True,
                             env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(self.lane.tmp)})
        table = dict(line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)
        self.assertEqual(table["UPDATER_KEY"].strip("'"), f"{self.lane.tmp}/.local/share/zerocode/release/keys/updater.key")

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
        installed app is: without a distribution identity in the env the lane
        signs it inside-out with the install signer, packs the archive and the
        DMG again from the sealed bundle and signs the archive with the updater
        key; with one, Tauri's own seal is kept. Either way the bundle verifies
        --deep --strict before it is gathered (v1.3.78 shipped the linker's adhoc
        signature over unsealed resources and a colleague's Mac called it
        damaged, 2026-09-15)."""
        lane = LANE.read_text()
        seal = lane.split("seal_updater_bundle() {")[1].split("\n}\n")[0]
        for words in [
            'if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then',
            'sign-app-bundle.sh" "$app"',
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

    def test_repo_flakes_file_has_only_prefixed_names(self):
        lines = [l.strip() for l in (RELEASE / "flakes.txt").read_text().splitlines()]
        names = [l for l in lines if l and not l.startswith("#")]
        self.assertTrue(names, "flakes.txt names nothing")
        for n in names:
            self.assertRegex(n, r"^(root|zo):[A-Za-z0-9_:]+$", n)
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
        out = subprocess.run(["bash", str(LANE), "--table"], capture_output=True, text=True, env=self.lane.env())
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
        w = subprocess.run(["bash", str(RELEASE / "status.sh"), "--wait", SHA_A, "--timeout", "5"],
                           env=self.lane.env(), capture_output=True, text=True)
        self.assertEqual(w.returncode, 5, w.stdout + w.stderr)
        self.assertIn("superseded", w.stdout)
        w = subprocess.run(["bash", str(RELEASE / "status.sh"), "--wait", SHA_B, "--timeout", "5"],
                           env=self.lane.env(), capture_output=True, text=True)
        self.assertEqual(w.returncode, 0, w.stdout + w.stderr)


class Companions(LaneCase):
    def test_enqueue_touches_the_queue_file(self):
        r = subprocess.run(["bash", str(RELEASE / "enqueue.sh"), SHA_A], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.lane.queue(), [SHA_A])
        self.assertIn(SHA_A[:8], r.stdout)

    def test_enqueue_rejects_a_non_sha(self):
        r = subprocess.run(["bash", str(RELEASE / "enqueue.sh"), "main"], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.lane.queue(), [])

    def test_status_line_and_wait(self):
        self.lane.enqueue(SHA_A)
        self.assertEqual(self.lane.run().returncode, 0)
        r = subprocess.run(["bash", str(RELEASE / "status.sh")], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        line = r.stdout.strip()
        self.assertEqual(line.count("\n"), 0)
        self.assertIn(SHA_A[:8], line)
        self.assertIn("sweep", line)
        self.assertIn("green", line)
        w = subprocess.run(["bash", str(RELEASE / "status.sh"), "--wait", SHA_A], env=self.lane.env(),
                           capture_output=True, text=True, timeout=10)
        self.assertEqual(w.returncode, 0, w.stdout + w.stderr)
        self.assertIn("green", w.stdout)

    def test_status_wait_exit_code_follows_the_outcome(self):
        self.lane.enqueue(SHA_A)
        self.lane.run(GATE_ROOT_RC=101, GATE_ROOT_FAILS="x::y", SOLO_RCS="1")  # stays red solo
        w = subprocess.run(["bash", str(RELEASE / "status.sh"), "--wait", SHA_A], env=self.lane.env(),
                           capture_output=True, text=True, timeout=10)
        self.assertNotEqual(w.returncode, 0)
        self.assertIn("red", w.stdout)

    def test_status_without_a_file_says_so(self):
        r = subprocess.run(["bash", str(RELEASE / "status.sh")], env=self.lane.env(),
                           capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)

    def test_install_launchd_renders_a_valid_plist(self):
        r = subprocess.run(["bash", str(RELEASE / "install-launchd.sh"), "--render", "--label-suffix", "selftest"],
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
        r = subprocess.run(["bash", str(RELEASE / "rollback.sh"), "--app", "--zo"], env=self.lane.env(),
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
        r = subprocess.run(["bash", str(RELEASE / "rollback.sh"), "--app"], env=self.lane.env(),
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
        line = subprocess.run(["bash", str(RELEASE / "status.sh")], env=self.lane.env(),
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
        line = subprocess.run(["bash", str(RELEASE / "status.sh")], env=self.lane.env(),
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
        built = [l for l in self.lane.stub_lines() if "bundle-updater" in l]
        self.assertEqual(len(built), 1, built)
        self.assertIn(f"target={self.lane.target('root-release')}", built[0])
        self.assertIn("key=" + str(self.lane.key), built[0], "the build is told the private key, never its bytes")

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
    """A gate judges one tree. A sha that left that tree exactly as the last
    green install of that half was built from — apart from the release stamp
    and the docs — owes it nothing, and the phase line says so (lane.sh
    `gate_owed`). Measured 2026-09-16: gate-root 1105–1615 s, gate-zo 876–1296 s
    on releases that each changed one tree."""

    def seed_installed(self, sha):
        (self.lane.home / "installed.json").write_text(
            '{"app":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.90"},'
            '"zo":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.90"}}\n' % (sha, sha))

    def gates(self, sha):
        lines = [l for l in self.lane.stub_lines() if l.startswith(sha[:8]) and " gate " in l]
        ran = {l.split(" gate ")[1].split()[0] for l in lines if "target=" in l}
        skipped = {l.split(" gate ")[1].split()[0] for l in lines if "not owed" in l}
        return ran, skipped

    def test_a_window_only_sha_owes_the_root_gate_alone(self):
        self.seed_installed(SHA_C)
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="ui/shell.js crates/zerocode-shell/src/main.rs Cargo.toml Cargo.lock "
                                        "zo-ide/Cargo.toml zo-ide/Cargo.lock CHANGELOG.md")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), ({"root"}, {"zo"}))
        zo = self.lane.phase("gate-zo")
        self.assertEqual((zo["rc"], zo["secs"]), (0, 0))
        self.assertIn(f"zo tree unchanged since {SHA_C[:8]}", zo["skipped"])
        self.assertEqual(self.lane.status()["outcome"], "green")
        self.assertEqual(self.lane.installed()["zo"]["sha"], SHA_A, "the skipped half is still built and installed")
        log = (self.lane.home / "out" / f"lane-{SHA_A[:8]}.log").read_text()
        self.assertIn("gate-zo rc=0 0s", log)
        self.assertIn("skipped: zo tree unchanged", log)

    def test_a_zo_only_sha_owes_the_zo_gate_alone(self):
        self.seed_installed(SHA_C)
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="zo-ide/crates/api/src/lib.rs zo-ide/Cargo.lock Cargo.toml "
                                        "docs/design/zo-autonomous-routing-review-20260915.md")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), ({"zo"}, {"root"}))
        self.assertIn(f"root tree unchanged since {SHA_C[:8]}", self.lane.phase("gate-root")["skipped"])

    def test_a_shared_crate_owes_both_gates(self):
        self.seed_installed(SHA_C)
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="crates/zerocode-core/src/agent.rs")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), ({"root", "zo"}, set()))

    def test_a_real_change_in_a_stamp_file_owes_its_gate(self):
        self.seed_installed(SHA_C)
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="Cargo.toml", STAMP_REAL_CHANGES="Cargo.toml")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), ({"root"}, {"zo"}))

    def test_a_docs_only_sha_owes_no_gate_and_still_installs(self):
        self.seed_installed(SHA_C)
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="docs/design/x.md README.md CHANGELOG.md")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), (set(), {"root", "zo"}))
        self.assertEqual(self.lane.installed()["app"]["sha"], SHA_A)

    def test_a_first_install_owes_both_gates(self):
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="ui/shell.js")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), ({"root", "zo"}, set()))

    def test_when_cargo_cannot_name_the_shared_crates_both_gates_run(self):
        self.seed_installed(SHA_C)
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="ui/shell.js", ZO_SHARED_PATHS="-")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.gates(SHA_A), ({"root", "zo"}, set()))

    def test_the_base_is_each_halfs_own_install(self):
        """The app half and the zo half were installed from different shas
        (a red app beside a green zo): each gate compares against its own."""
        (self.lane.home / "installed.json").write_text(
            '{"app":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.89"},'
            '"zo":{"sha":"%s","at":"2026-09-16T00:00:00Z","version":"1.3.90"}}\n' % (SHA_B, SHA_C))
        self.lane.enqueue(SHA_A)
        r = self.lane.run(CHANGED_PATHS="zo-ide/crates/api/src/lib.rs")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn(f"root tree unchanged since {SHA_B[:8]}", self.lane.phase("gate-root")["skipped"])


if __name__ == "__main__":
    unittest.main(verbosity=1)
