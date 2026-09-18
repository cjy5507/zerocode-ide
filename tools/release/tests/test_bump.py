#!/usr/bin/env python3
"""Contract for `tools/release/bump.sh` (docs/design/versioned-auto-update.md §2.1).

One hand raises the version: the three files that carry it — the root
`Cargo.toml` `[workspace.package]`, `crates/zerocode-shell/tauri.conf.json`
and `zo-ide/Cargo.toml` — move together to the same semver, and
`CHANGELOG.md` opens a `## [x.y.z] — YYYY-MM-DD` section at the top that
lists the commit titles since the last `vX.Y.Z` tag by prefix. Every case
runs the real script against a throwaway git repository built here, so the
contract is pinned without touching this checkout.

Run: python3 tools/release/tests/test_bump.py   (stdlib only)
"""

import datetime
import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
RELEASE = REPO / "tools" / "release"
BUMP = RELEASE / "bump.sh"
# The repository the fixture's own feed points at: bump.sh asks lane.sh's table
# for the release repo and the table reads it off the shipped updater endpoint,
# so a fixture whose config carries none asks `repos//git/ref/tags/...`, which
# answers 404 for every tag there is and reads back as "the tag is free".
FIXTURE_REPO = "zerocode-tests/bump-fixture"

ROOT_CARGO = """[workspace]
members = []
resolver = "3"

[workspace.package]
version = "{v}"
edition = "2024"
rust-version = "1.94"
license = "MIT"

[workspace.dependencies]
serde = {{ version = "1.0.219", features = ["derive"] }}

[profile.release]
lto = "fat"
"""

ZO_CARGO = """# zo-ide — its own workspace.
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
version = "{v}"
edition = "2021"
license = "MIT"

[workspace.dependencies]
anyhow = {{ version = "1.0.98" }}
"""

TAURI_CONF = {
    "$schema": "https://schema.tauri.app/config/2",
    "productName": "ZeroCode",
    "version": None,
    "identifier": "dev.zerocode.app",
    "bundle": {"active": True, "targets": ["app", "dmg"]},
    "plugins": {"updater": {"endpoints": [
        f"https://github.com/{FIXTURE_REPO}/releases/latest/download/latest.json"]}},
}

SECTION = re.compile(r"^## \[(\d+\.\d+\.\d+)\] — (\d{4}-\d\d-\d\d)$", re.M)


def git(repo, *args, **kw):
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True, check=True, **kw)


class Fixture:
    """A throwaway repository shaped like this one where the version lives."""

    def __init__(self, tmp: Path, version="0.1.0"):
        self.repo = tmp / "repo"
        (self.repo / "crates" / "zerocode-shell").mkdir(parents=True)
        (self.repo / "zo-ide").mkdir(parents=True)
        git_env = {
            "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@x", "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@x",
            # The system dirs only: git, awk and sed live there, gh (homebrew)
            # does not, so the default fixture never reaches the network; the
            # probe cases put a fake gh in front.
            "PATH": "/usr/bin:/bin",
            "HOME": str(tmp),
        }
        self.env = git_env
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], check=True, env=git_env)
        self.write_versions(version, version, version)
        self.commit("chore: the first commit")

    # -- files -------------------------------------------------------------
    @property
    def root_cargo(self):
        return self.repo / "Cargo.toml"

    @property
    def tauri_conf(self):
        return self.repo / "crates" / "zerocode-shell" / "tauri.conf.json"

    @property
    def zo_cargo(self):
        return self.repo / "zo-ide" / "Cargo.toml"

    @property
    def changelog(self):
        return self.repo / "CHANGELOG.md"

    def write_versions(self, root, tauri, zo):
        self.root_cargo.write_text(ROOT_CARGO.format(v=root))
        conf = dict(TAURI_CONF)
        conf["version"] = tauri
        self.tauri_conf.write_text(json.dumps(conf, indent=2) + "\n")
        self.zo_cargo.write_text(ZO_CARGO.format(v=zo))

    def versions(self):
        root = re.search(r"^\[workspace\.package\]\nversion = \"([^\"]+)\"", self.root_cargo.read_text(), re.M)
        zo = re.search(r"^\[workspace\.package\]\nversion = \"([^\"]+)\"", self.zo_cargo.read_text(), re.M)
        return root.group(1), json.loads(self.tauri_conf.read_text())["version"], zo.group(1)

    # -- history -----------------------------------------------------------
    def commit(self, subject):
        marker = self.repo / "touched"
        marker.write_text(subject + "\n")
        subprocess.run(["git", "-C", str(self.repo), "add", "-A"], check=True, env=self.env)
        subprocess.run(["git", "-C", str(self.repo), "commit", "-q", "-m", subject], check=True, env=self.env)

    def tag(self, name):
        subprocess.run(["git", "-C", str(self.repo), "tag", name], check=True, env=self.env)

    # -- the script --------------------------------------------------------
    def bump(self, *args):
        return subprocess.run(
            ["bash", str(BUMP), *args, "--repo", str(self.repo)],
            capture_output=True, text=True, env=self.env, timeout=30,
        )

    def sections(self):
        return SECTION.findall(self.changelog.read_text())

    def section_body(self, version):
        text = self.changelog.read_text()
        start = text.index(f"## [{version}]")
        rest = text[start + 1:]
        end = rest.find("\n## [")
        return text[start:] if end < 0 else text[start:start + 1 + end]


class BumpCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.mkdtemp(prefix="bump-")
        self.fx = Fixture(Path(self._tmp))

    def tearDown(self):
        shutil.rmtree(self._tmp, ignore_errors=True)


class Table(BumpCase):
    def test_script_exists_parses_and_has_a_table(self):
        self.assertTrue(BUMP.exists(), "tools/release/bump.sh missing")
        self.assertTrue(os.access(BUMP, os.X_OK), "bump.sh not executable")
        rc = subprocess.run(["bash", "-n", str(BUMP)], capture_output=True, text=True)
        self.assertEqual(rc.returncode, 0, rc.stderr)
        out = subprocess.run(["bash", str(BUMP), "--table"], capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stderr)
        table = dict(line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)
        table = {k: v.strip("'") for k, v in table.items()}
        self.assertEqual(table["ROOT_CARGO"], "Cargo.toml")
        self.assertEqual(table["TAURI_CONF"], "crates/zerocode-shell/tauri.conf.json")
        self.assertEqual(table["ZO_CARGO"], "zo-ide/Cargo.toml")
        self.assertEqual(table["CHANGELOG"], "CHANGELOG.md")
        self.assertEqual(table["TAG_GLOB"], "v[0-9]*")
        self.assertTrue(int(table["MAX_PER_PREFIX"]) > 0)
        self.assertEqual(table["PREFIX_ORDER"].split()[:2], ["feat", "fix"])
        self.assertIn("other", table["PREFIX_ORDER"].split())

    def test_a_bad_part_is_usage(self):
        r = self.fx.bump("bigger")
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.1.0", "0.1.0", "0.1.0"), "nothing moves on usage")
        self.assertFalse(self.fx.changelog.exists())
        r = self.fx.bump()
        self.assertEqual(r.returncode, 2)


class Lockfiles(BumpCase):
    def fake_cargo(self, fail=False):
        fake = Path(self._tmp) / "cargo-bin"
        fake.mkdir()
        log = Path(self._tmp) / "cargo.log"
        cargo = fake / "cargo"
        cargo.write_text(
            "#!/bin/sh\n"
            f"printf '%s|%s\\n' \"$PWD\" \"$*\" >> '{log}'\n"
            + ("exit 17\n" if fail else
               "awk '/^version =/{print; exit}' Cargo.toml > Cargo.lock\n")
        )
        cargo.chmod(0o755)
        self.fx.env["PATH"] = f"{fake}:{self.fx.env['PATH']}"
        return log

    def test_both_lockfiles_follow_the_new_workspace_version_offline(self):
        log = self.fake_cargo()
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(log.exists(), "bump never ran cargo for the lockfiles")
        self.assertEqual([f"{Path(folder).resolve()}|{args}"
                          for folder, args in (line.split("|", 1) for line in log.read_text().splitlines())], [
            f"{folder.resolve()}|update --workspace --offline"
            for folder in (self.fx.repo, self.fx.repo / "zo-ide")
        ])
        for folder in (self.fx.repo, self.fx.repo / "zo-ide"):
            self.assertEqual((folder / "Cargo.lock").read_text(), 'version = "0.1.1"\n')

    def test_no_cargo_loudly_skips_lockfile_refresh(self):
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("cargo not found", r.stdout + r.stderr)
        self.assertIn("lockfiles", r.stdout + r.stderr)

    def test_cargo_failure_is_not_reported_as_a_successful_bump(self):
        self.fake_cargo(fail=True)
        r = self.fx.bump("patch")
        self.assertNotEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("lockfile", r.stdout + r.stderr)


class ThreeFiles(BumpCase):
    def test_patch_minor_major_move_the_three_files_together(self):
        for part, want in (("patch", "0.1.1"), ("minor", "0.2.0"), ("major", "1.0.0")):
            self.fx.write_versions("0.1.0", "0.1.0", "0.1.0")
            self.fx.commit(f"chore: reset before {part}")
            r = self.fx.bump(part)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertEqual(self.fx.versions(), (want, want, want), part)
            self.assertIn(want, r.stdout, "the new version is the script's answer")

    def test_only_the_version_line_under_workspace_package_moves(self):
        before_root = self.fx.root_cargo.read_text()
        before_zo = self.fx.zo_cargo.read_text()
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        after_root = self.fx.root_cargo.read_text()
        after_zo = self.fx.zo_cargo.read_text()
        self.assertIn('serde = { version = "1.0.219"', after_root, "a dependency's version is not the workspace's")
        self.assertIn('anyhow = { version = "1.0.98" }', after_zo)
        self.assertEqual(before_root.replace('version = "0.1.0"', 'version = "0.1.1"', 1), after_root)
        self.assertEqual(before_zo.replace('version = "0.1.0"', 'version = "0.1.1"', 1), after_zo)
        conf = json.loads(self.fx.tauri_conf.read_text())
        self.assertEqual(conf["version"], "0.1.1")
        self.assertEqual(conf["productName"], "ZeroCode", "the rest of tauri.conf.json is untouched")
        self.assertEqual(conf["bundle"]["targets"], ["app", "dmg"])

    def test_an_existing_local_tag_refuses(self):
        # cjy5507/zerocode already carries the old CLI's v1.2.3–v1.2.7 tags
        # (design §2.2 coexistence rule): a version whose tag exists is refused
        # before any file moves, locally and — through `gh api` — on the repo.
        self.fx.commit("feat: something")
        self.fx.tag("v0.1.1")
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("v0.1.1", r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.1.0", "0.1.0", "0.1.0"))
        self.assertFalse(self.fx.changelog.exists())

    def test_an_existing_remote_tag_is_asked_of_gh_api_and_refuses(self):
        fake = Path(self._tmp) / "bin"
        fake.mkdir()
        log = Path(self._tmp) / "gh.log"
        (fake / "gh").write_text(
            "#!/bin/sh\n"
            f"printf '%s\\n' \"$*\" >> {log}\n"
            "case \"$*\" in *api*git/ref/tags/v0.1.1*) exit 0;; esac\n"
            "echo 'gh: Not Found (HTTP 404)' >&2; exit 1\n"
        )
        (fake / "gh").chmod(0o755)
        self.fx.env["PATH"] = f"{fake}:{self.fx.env['PATH']}"
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("v0.1.1", r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.1.0", "0.1.0", "0.1.0"))
        asked = log.read_text()
        self.assertIn(f"repos/{FIXTURE_REPO}/git/ref/tags/v0.1.1", asked,
                      "the probe names the checkout's own repo and the tag")
        r = self.fx.bump("minor")
        self.assertEqual(r.returncode, 0, "a tag the repo does not have goes through: " + r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.2.0", "0.2.0", "0.2.0"))

    def test_a_checkout_that_names_no_repository_refuses(self):
        # `repos//git/ref/tags/v0.1.1` is a 404 for every tag there is, so a
        # probe against a nameless repository reads as "free" and lets a
        # duplicate tag through. The bump says so instead of asking.
        conf = json.loads(self.fx.tauri_conf.read_text())
        del conf["plugins"]
        self.fx.tauri_conf.write_text(json.dumps(conf, indent=2) + "\n")
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("origin", r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.1.0", "0.1.0", "0.1.0"))

    def test_a_probe_that_fails_for_another_reason_refuses(self):
        # A network error is not "the tag is free": the person retries.
        fake = Path(self._tmp) / "bin"
        fake.mkdir()
        (fake / "gh").write_text("#!/bin/sh\necho 'error connecting to api.github.com' >&2; exit 1\n")
        (fake / "gh").chmod(0o755)
        self.fx.env["PATH"] = f"{fake}:{self.fx.env['PATH']}"
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("error connecting", r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.1.0", "0.1.0", "0.1.0"))

    def test_without_gh_the_remote_probe_is_skipped_aloud(self):
        # bash and git still resolve from the system dirs; gh (homebrew) does not.
        self.fx.env["PATH"] = "/usr/bin:/bin"
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("gh", r.stdout + r.stderr, "the skipped remote probe is said, not silent")

    def test_disagreeing_files_refuse_before_touching_anything(self):
        self.fx.write_versions("0.1.0", "0.1.0", "0.0.9")
        self.fx.commit("chore: zo drifted")
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("0.0.9", r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.1.0", "0.1.0", "0.0.9"), "a refusal moves nothing")
        self.assertFalse(self.fx.changelog.exists())


class Changelog(BumpCase):
    def test_the_first_section_without_a_tag_lists_every_commit_by_prefix(self):
        self.fx.commit("feat(window): a first feature")
        self.fx.commit("fix: a fix without a scope")
        self.fx.commit("docs(product): the product doc")
        self.fx.commit("merge: t-1 — a merge")
        self.fx.commit("test(shell)+docs(product): a compound prefix")
        self.fx.commit("A subject with no prefix at all")
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        text = self.fx.changelog.read_text()
        self.assertTrue(text.startswith("# Changelog\n"), text[:40])
        today = datetime.date.today().isoformat()
        self.assertEqual(self.fx.sections(), [("0.1.1", today)])
        body = self.fx.section_body("0.1.1")
        self.assertRegex(body, r"(?m)^_.*(no tag|first).*_$", "the first section says there was no earlier tag")
        headings = re.findall(r"(?m)^### (\S+)$", body)
        self.assertEqual(headings, ["feat", "fix", "docs", "test", "chore", "merge", "other"], body)
        self.assertIn("- feat(window): a first feature", body)
        self.assertIn("- fix: a fix without a scope", body)
        self.assertIn("- test(shell)+docs(product): a compound prefix", body)
        self.assertIn("- A subject with no prefix at all", body)
        self.assertIn("- chore: the first commit", body, "with no tag every commit so far is listed")
        self.assertLess(body.index("### feat"), body.index("### fix"))
        self.assertLess(body.index("### docs"), body.index("### merge"))
        self.assertLess(body.index("### merge"), body.index("### other"))

    def test_a_second_bump_lists_only_the_commits_after_the_last_tag(self):
        self.fx.commit("feat: before the tag")
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.fx.commit("release: v0.1.1")
        self.fx.tag("v0.1.1")
        self.fx.commit("fix(lane): after the tag")
        self.fx.commit("perf(pty): also after the tag")
        r = self.fx.bump("minor")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.fx.versions(), ("0.2.0", "0.2.0", "0.2.0"))
        versions = [v for v, _ in self.fx.sections()]
        self.assertEqual(versions, ["0.2.0", "0.1.1"], "the new section opens on top; the old one stays")
        body = self.fx.section_body("0.2.0")
        self.assertIn("v0.1.1", body, "the section names the tag it starts after")
        self.assertIn("- fix(lane): after the tag", body)
        self.assertIn("- perf(pty): also after the tag", body)
        self.assertNotIn("before the tag", body)
        self.assertNotIn("the first commit", body)
        old = self.fx.section_body("0.1.1")
        self.assertIn("- feat: before the tag", old, "the earlier section is not rewritten")

    def test_a_tag_that_is_not_a_version_does_not_count(self):
        self.fx.commit("feat: early")
        self.fx.tag("beta")
        self.fx.tag("archive/old-branch")
        self.fx.commit("fix: late")
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        body = self.fx.section_body("0.1.1")
        self.assertIn("- feat: early", body, "only vX.Y.Z tags bound a section")
        self.assertIn("- fix: late", body)

    def test_a_long_prefix_is_capped_by_the_table(self):
        out = subprocess.run(["bash", str(BUMP), "--table"], capture_output=True, text=True)
        cap = int(dict(l.split("=", 1) for l in out.stdout.splitlines())["MAX_PER_PREFIX"].strip("'"))
        for i in range(cap + 5):
            self.fx.commit(f"fix(many): number {i}")
        r = self.fx.bump("patch")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        body = self.fx.section_body("0.1.1")
        listed = re.findall(r"(?m)^- fix\(many\): number \d+$", body)
        self.assertEqual(len(listed), cap)
        self.assertIn(f"number {cap + 4}", listed[0], "the newest commits are the ones listed")
        self.assertRegex(body, r"(?m)^- … 5 more fix", "the rest is counted, not dropped")


if __name__ == "__main__":
    unittest.main(verbosity=1)
