#!/usr/bin/env python3
"""changed_crates.py: `just pre-handin` runs the unit tests of the crates a
branch changed, chosen from Cargo's package list and the diff — no list kept.

Run with: python3 tools/release/tests/test_changed_crates.py
"""

import importlib.util
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "changed_crates.py"


def load():
    spec = importlib.util.spec_from_file_location("changed_crates", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def package(name, directory, *targets):
    return {"name": name, "dir": directory, "targets": [(kinds, target) for kinds, target in targets]}


WORKSPACES = {
    ".": [
        package("zerocode-core", "crates/zerocode-core", (["lib"], "zerocode_core")),
        package("zerocode-shell", "crates/zerocode-shell", (["bin"], "zerocode-shell"), (["bin"], "zerocode-pick")),
        package("zerocode-shell-state", "crates/zerocode-shell-state", (["lib"], "zerocode_shell_state")),
        package("tools-only", "tools/many", (["bin"], "first"), (["bin"], "second")),
        package("bare", "crates/bare"),
    ],
    "zo-ide": [
        package("runtime", "zo-ide/crates/runtime", (["lib"], "runtime")),
    ],
}


class Pick(unittest.TestCase):
    def setUp(self):
        self.mod = load()

    def test_a_library_is_tested_with_lib_and_a_bin_only_crate_with_its_own_bin(self):
        picked = self.mod.crates(["crates/zerocode-core/src/a.rs", "crates/zerocode-shell/src/main.rs"], WORKSPACES)
        self.assertEqual(picked, [(".", "zerocode-core", "--lib"), (".", "zerocode-shell", "--bin zerocode-shell")])

    def test_a_crate_whose_name_prefixes_a_sibling_does_not_own_the_siblings_files(self):
        picked = self.mod.crates(["crates/zerocode-shell-state/src/lib.rs"], WORKSPACES)
        self.assertEqual(picked, [(".", "zerocode-shell-state", "--lib")])

    def test_zo_ide_crates_belong_to_their_own_workspace(self):
        picked = self.mod.crates(["zo-ide/crates/runtime/src/lib.rs"], WORKSPACES)
        self.assertEqual(picked, [("zo-ide", "runtime", "--lib")])

    def test_a_crate_is_named_once_however_many_of_its_files_changed(self):
        picked = self.mod.crates(["crates/zerocode-core/a.rs", "crates/zerocode-core/b.rs"], WORKSPACES)
        self.assertEqual(len(picked), 1)

    def test_a_crate_with_several_bins_and_none_like_it_runs_all_of_them_and_one_with_no_target_none(self):
        picked = self.mod.crates(["tools/many/a.rs", "crates/bare/Cargo.toml"], WORKSPACES)
        self.assertEqual(picked, [(".", "tools-only", "--bins")])

    def test_paths_outside_every_crate_pick_nothing(self):
        self.assertEqual(self.mod.crates(["justfile", "ui/shell.css", "docs/a.md", "Cargo.lock"], WORKSPACES), [])

    def test_a_suite_is_hinted_only_when_a_changed_ui_test_file_is_named_like_a_registered_one(self):
        paths = ["ui/tests/workers.mjs", "ui/tests/helper.mjs", "ui/shell.css", "crates/x/workers.mjs"]
        self.assertEqual(self.mod.suites_named_like(paths, ["workers", "inbox"]), ["workers"])

    def test_the_registry_regex_reads_the_names_window_mjs_registers(self):
        text = 'suite("window-harness", async () => {});\n  suite( "inbox", () => 1);\n// suite("not-a-call"\n'
        self.assertEqual(self.mod.SUITE.findall(text), ["window-harness", "inbox"])


@unittest.skipUnless(shutil.which("cargo") and shutil.which("git"), "needs cargo and git")
class WholeRun(unittest.TestCase):
    """The script copied into a throwaway repository: real `cargo metadata`, real `git diff`."""

    def setUp(self):
        self.repo = Path(tempfile.mkdtemp(prefix="changed-crates-"))
        self.addCleanup(shutil.rmtree, self.repo, ignore_errors=True)
        self.write("Cargo.toml", '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n')
        self.write("crates/lib-crate/Cargo.toml", '[package]\nname = "lib-crate"\nversion = "0.1.0"\nedition = "2021"\n')
        self.write("crates/lib-crate/src/lib.rs", "")
        self.write("crates/bin-crate/Cargo.toml", '[package]\nname = "bin-crate"\nversion = "0.1.0"\nedition = "2021"\n')
        self.write("crates/bin-crate/src/main.rs", "fn main() {}\n")
        self.write("ui/tests/window.mjs", 'suite("inbox", () => 1);\nsuite("workers", () => 1);\n')
        self.write("ui/tests/inbox.mjs", "")
        self.git("init", "-q", "-b", "main")
        self.git("add", ".")
        self.git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", "base")
        copy = self.repo / "tools/release/changed_crates.py"
        copy.parent.mkdir(parents=True)
        shutil.copy(SCRIPT, copy)

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.repo, check=True, capture_output=True)

    def run_script(self):
        done = subprocess.run([sys.executable, "-I", str(self.repo / "tools/release/changed_crates.py")],
                              cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.splitlines()

    def test_a_tracked_edit_an_untracked_file_and_a_ui_edit_each_come_out_once(self):
        self.write("crates/lib-crate/src/lib.rs", "pub fn a() {}\n")
        self.write("crates/bin-crate/src/new.rs", "")
        self.write("ui/tests/inbox.mjs", "// changed\n")
        self.assertEqual(self.run_script(), [
            "crate\t.\tbin-crate\t--bin bin-crate",
            "crate\t.\tlib-crate\t--lib",
            "ui\tui/tests/inbox.mjs",
            "suite\tinbox",
        ])

    def test_an_unchanged_branch_picks_nothing(self):
        self.assertEqual(self.run_script(), [])


if __name__ == "__main__":
    unittest.main()
