#!/usr/bin/env python3
"""The quick check before a batch is gated (`tools/release/prebatch.py`, t-73910 W3-5).

Each red of 10-25's batch gate that a text read finds is one case here, in a throwaway
repository shaped like this one: a bundled skill's text with no release line (l78d), a
tools seed behind the Rust source it copies (l78d), a launch road more than the shell's
literal count allows (l78g), a load-time call the settings test's fake backend has no case
for (l78h). A path no check reads is said unchecked, never passed, and nothing runs cargo.

Run: python3 tools/release/tests/test_prebatch.py   (stdlib only)
"""

import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "prebatch.py"

BOOK = '''\
const BACKEND_PARTS: &[(&str, &str)] = &[
    ("main.rs", include_str!("main.rs")),
    (
        "cmd/launch.rs",
        include_str!("cmd/launch.rs"),
    ),
];

fn shipped_backend() -> &'static str {
    static JOINED: OnceLock<String> = OnceLock::new();
    JOINED
        .get_or_init(|| {
            BACKEND_PARTS
                .iter()
                .map(|(_, text)| {
                    text.split_once("#[cfg(test)]")
                        .map_or(*text, |(before, _)| before)
                })
                .collect::<Vec<_>>()
                .join("\\n")
                .replace("pub(super) ", "")
        })
        .as_str()
}

#[test]
fn a_launch_deadline_is_the_agents_own_on_every_launch_road() {
    let backend = shipped_backend();
    let (shipped, _) = backend.split_once("#[cfg(test)]").unwrap_or((backend, ""));
    assert_eq!(
        shipped.matches("PromptDelivery::with_deadlines(").count(),
        ROADS,
        "a launch road stopped paying the agent's own quiet window and deadline"
    );
}

#[test]
fn a_count_over_some_other_text_is_not_the_backends() {
    let window = window_source();
    assert_eq!(
        window.matches("PromptDelivery::with_deadlines(").count(),
        7,
        "not a count of the shipped backend"
    );
}
'''

SEED_TEST = '''\
import re, sys
from pathlib import Path
REPO = Path(__file__).resolve().parents[2]
RUST = (REPO / "crates" / "zerocode-core" / "src" / "orchestration.rs").read_text()
SEED = (REPO / "tools/kinds-seed/seed.py").read_text()
kinds = sorted(re.findall(r"Kind::(\\w+)", RUST))
seeded = sorted(re.findall(r'"(\\w+)"', SEED))
sys.exit(0 if kinds == seeded else 1)
'''


def git(repo, *args, env):
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True, check=True, env=env).stdout.strip()


class Fixture:
    """A repository holding one instance of every rule the quick check reads."""

    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.repo = tmp / "repo"
        self.bin = tmp / "bin"
        self.bin.mkdir()
        (self.bin / "python3").symlink_to(sys.executable)
        self.cargo_called = tmp / "cargo-was-called"
        cargo = self.bin / "cargo"
        cargo.write_text(f"#!/bin/sh\necho \"$@\" >> '{self.cargo_called}'\nexit 0\n")
        cargo.chmod(0o755)
        self.env = {
            "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@x", "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@x", "HOME": str(tmp),
            "PATH": f"{self.bin}:/usr/bin:/bin",
        }
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], check=True, env=self.env)
        self.write("skills/plain/SKILL.md", "# plain\n")
        self.write("crates/zerocode-core/src/skill_releases.tsv", "")
        self.record_skill("plain")
        self.write("crates/zerocode-shell/src/main_unit_tests.rs", BOOK.replace("ROADS", "2"))
        self.write("crates/zerocode-shell/src/main.rs",
                   "fn a() { PromptDelivery::with_deadlines(1); }\n#[cfg(test)]\nPromptDelivery::with_deadlines(9);\n")
        self.write("crates/zerocode-shell/src/cmd/launch.rs", "pub(super) fn b() { PromptDelivery::with_deadlines(2); }\n")
        self.write("ui/index.html", '<script defer src="./vendor/cm6.js"></script>\n<script defer src="./shell-a.js"></script>\n')
        self.write("ui/shell-a.js", 'void invoke("known_one").catch(() => {});\n')
        self.write("ui/tests/settings.mjs", 'switch (command) {\n  case "known_one": return null;\n}\n')
        self.write("crates/zerocode-core/src/orchestration.rs", "Kind::Alpha\nKind::Beta\n")
        self.write("tools/kinds-seed/seed.py", 'KINDS = ["Alpha", "Beta"]\n')
        self.write("tools/tests/test_kinds_seed.py", SEED_TEST)
        self.write("justfile", textwrap.dedent("""\
            pii-check:
                python3 tools/release/pii-scan.py

            tools-test:
                python3 tools/tests/test_kinds_seed.py
                # a comment line is no test

            test:
                cargo test
            """))
        self.write("docs/notes.md", "notes\n")
        self.base = self.commit("the first commit")

    def write(self, path, text):
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def append(self, path, text):
        target = self.repo / path
        target.write_text(target.read_text() + text)

    def record_skill(self, name):
        blob = git(self.repo, "hash-object", f"skills/{name}/SKILL.md", env=self.env)
        self.append("crates/zerocode-core/src/skill_releases.tsv", f"{name}\t{blob}\n")

    def commit(self, message):
        git(self.repo, "add", "-A", env=self.env)
        git(self.repo, "commit", "-q", "--allow-empty", "-m", message, env=self.env)
        return git(self.repo, "rev-parse", "HEAD", env=self.env)

    def check(self, base=None, head="HEAD", *more):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--repo", str(self.repo), "--base", base or self.base, "--head", head, *more],
            capture_output=True, text=True, env=self.env, timeout=120,
        )


class PrebatchTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.fx = Fixture(Path(self.tmp.name))

    def tearDown(self):
        self.tmp.cleanup()

    def assertRed(self, said, check):
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn(f"RC {check}=1", said.stdout)
        self.assertIn("RC prebatch=1", said.stdout)

    def assertGreen(self, said, check):
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        self.assertIn(f"RC {check}=0", said.stdout)
        self.assertIn("RC prebatch=0", said.stdout)

    def test_a_changed_skill_without_its_release_line_is_red(self):
        self.fx.write("skills/plain/SKILL.md", "# plain, said again\n")
        self.fx.commit("a skill changed")
        said = self.fx.check()
        self.assertRed(said, "skill-release")
        self.assertIn("skills/plain/SKILL.md", said.stdout)
        self.fx.record_skill("plain")
        self.fx.commit("its release line")
        self.assertGreen(self.fx.check(), "skill-release")

    def test_a_new_ledger_kind_without_the_seed_row_is_red(self):
        self.fx.append("crates/zerocode-core/src/orchestration.rs", "Kind::Gamma\n")
        self.fx.commit("a kind the seed does not copy")
        said = self.fx.check()
        self.assertRed(said, "tools-test:tools/tests/test_kinds_seed.py")
        self.fx.write("tools/kinds-seed/seed.py", 'KINDS = ["Alpha", "Beta", "Gamma"]\n')
        self.fx.commit("the seed copies it")
        self.assertGreen(self.fx.check(), "tools-test:tools/tests/test_kinds_seed.py")

    def test_a_tools_test_runs_only_when_a_path_it_reads_changed(self):
        self.fx.append("ui/shell-a.js", "// a comment\n")
        self.fx.commit("a window change")
        said = self.fx.check()
        self.assertNotIn("tools-test:", said.stdout)

    def test_the_recipe_that_runs_every_script_is_read_as_its_scripts(self):
        # t-74384's tools-test: shell helpers `t` and `macos` called one line each, every
        # line ending `; \`. Each call is the script it runs; the helpers' own lines are none.
        self.fx.write("justfile", textwrap.dedent("""\
            tools-test:
                @rc=0; red=; skipped=; \\
                t() { echo "python3 $*" >&2; python3 "$@" || { rc=1; red="$red $*"; }; }; \\
                macos() { why=$1; shift; if [ "$(uname -s)" = Darwin ]; then t "$@"; else echo "SKIPPED python3 $* — macOS-only: $why"; fi; }; \\
                t tools/tests/test_kinds_seed.py; \\
                macos "drives a macOS-only tool" tools/tests/test_mac_only.py; \\
                [ -z "$red" ] || { echo "tools-test: red scripts:$red" >&2; exit 1; }; exit $rc
            """))
        self.fx.write("tools/tests/test_mac_only.py", "raise SystemExit(0)\n")
        self.fx.commit("the recipe runs every script")
        self.fx.append("crates/zerocode-core/src/orchestration.rs", "Kind::Gamma\n")
        self.fx.commit("a kind the seed does not copy")
        said = self.fx.check()
        self.assertRed(said, "tools-test:tools/tests/test_kinds_seed.py")
        self.assertNotIn("Traceback", said.stderr)
        self.fx.write("tools/kinds-seed/seed.py", 'KINDS = ["Alpha", "Beta", "Gamma"]\n')
        self.fx.commit("the seed copies it")
        self.assertGreen(self.fx.check(), "tools-test:tools/tests/test_kinds_seed.py")
        self.fx.append("tools/tests/test_mac_only.py", "# touched\n")
        self.fx.commit("the macOS-only script changed")
        said = self.fx.check()
        if sys.platform == "darwin":
            self.assertGreen(said, "tools-test:tools/tests/test_mac_only.py")
        else:
            self.assertNotIn("RC tools-test:tools/tests/test_mac_only.py", said.stdout)

    def test_a_tools_test_line_whose_file_head_lacks_is_red(self):
        self.fx.write("justfile", self.fx.repo.joinpath("justfile").read_text().replace(
            "    # a comment line is no test\n",
            "    # a comment line is no test\n    python3 -I tools/tests/test_missing.py\n"))
        self.fx.write("tools/tests/helper.py", "X = 1\n")
        self.fx.commit("a test line with no file")
        said = self.fx.check()
        self.assertRed(said, "tools-test:tools/tests/test_missing.py")
        self.assertIn("RC tools-test:tools/tests/test_kinds_seed.py=0", said.stdout)

    def test_a_launch_road_past_the_tests_own_count_is_red(self):
        self.fx.append("crates/zerocode-shell/src/cmd/launch.rs", "fn c() { PromptDelivery::with_deadlines(3); }\n")
        self.fx.commit("a third launch road")
        said = self.fx.check()
        self.assertRed(said, "backend-count")
        self.assertIn("crates/zerocode-shell/src/main_unit_tests.rs:", said.stdout)
        self.assertIn("counts 3", said.stdout)
        self.assertIn("the test says 2", said.stdout)
        self.assertNotIn("the test says 7", said.stdout, "a count over another text is not the backend's")
        self.fx.write("crates/zerocode-shell/src/main_unit_tests.rs", BOOK.replace("ROADS", "3"))
        self.fx.commit("the count says three")
        self.assertGreen(self.fx.check(), "backend-count")

    def test_a_load_time_call_the_settings_backend_does_not_know_is_red(self):
        self.fx.append("ui/shell-a.js", 'el("b").addEventListener("click", () => void invoke("on_press").catch(() => {}));\n')
        self.fx.commit("a call made on a press")
        self.assertGreen(self.fx.check(), "settings-ipc")
        self.fx.append("ui/shell-a.js", 'void invoke("share_state").then(take).catch(() => {});\n')
        self.fx.commit("a call made on load")
        said = self.fx.check()
        self.assertRed(said, "settings-ipc")
        self.assertIn("share_state", said.stdout)
        self.assertNotIn("on_press", said.stdout)
        self.fx.append("ui/tests/settings.mjs", 'switch (command) {\n  case "share_state": return null;\n}\n')
        self.fx.commit("the fake backend answers it")
        self.assertGreen(self.fx.check(), "settings-ipc")

    def test_a_slow_tools_test_is_unchecked_unless_slow_is_asked(self):
        self.fx.write("tools/release/tests/test_lane.py", SEED_TEST.replace("parents[2]", "parents[3]"))
        self.fx.write("justfile", self.fx.repo.joinpath("justfile").read_text().replace(
            "    # a comment line is no test\n",
            "    # a comment line is no test\n    python3 tools/release/tests/test_lane.py\n"))
        self.fx.commit("the lane's test")
        self.fx.append("crates/zerocode-core/src/orchestration.rs", "// touched\n")
        self.fx.commit("a path both tests read")
        said = self.fx.check(None, "HEAD")
        self.assertGreen(said, "tools-test:tools/tests/test_kinds_seed.py")
        self.assertIn("unchecked check tools-test:tools/release/tests/test_lane.py — slow", said.stdout)
        self.assertNotIn("RC tools-test:tools/release/tests/test_lane.py", said.stdout)
        self.assertGreen(self.fx.check(None, "HEAD", "--slow"), "tools-test:tools/release/tests/test_lane.py")

    def test_a_path_no_rule_covers_is_reported_unchecked_not_passed(self):
        self.fx.append("docs/notes.md", "more\n")
        self.fx.commit("a note")
        said = self.fx.check()
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        self.assertIn("unchecked docs/notes.md", said.stdout)
        self.assertNotIn("RC skill-release", said.stdout)
        self.assertIn("RC prebatch=0", said.stdout)

    def test_it_never_runs_cargo(self):
        self.fx.write("skills/plain/SKILL.md", "# plain, said again\n")
        self.fx.append("crates/zerocode-shell/src/main.rs", "// touched\n")
        self.fx.append("ui/shell-a.js", "// touched\n")
        self.fx.append("crates/zerocode-core/src/orchestration.rs", "// touched\n")
        self.fx.commit("every check's path")
        said = self.fx.check()
        for check in ("skill-release", "backend-count", "settings-ipc", "tools-test:tools/tests/test_kinds_seed.py"):
            self.assertIn(f"RC {check}=", said.stdout, said.stdout + said.stderr)
        self.assertFalse(self.fx.cargo_called.exists(), "the quick check ran cargo")

    def test_an_unreadable_revision_is_its_own_exit_not_a_pass(self):
        said = self.fx.check(head="no-such-revision")
        self.assertEqual(said.returncode, 2, said.stdout + said.stderr)
        self.assertNotIn("RC prebatch=0", said.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=1)
