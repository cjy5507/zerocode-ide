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


def package(name, directory, *targets, reads=()):
    return {"name": name, "dir": directory, "targets": [(kinds, target) for kinds, target in targets], "reads": dict(reads)}


WORKSPACES = {
    ".": [
        package("zerocode-core", "crates/zerocode-core", (["lib"], "zerocode_core")),
        package("zerocode-shell", "crates/zerocode-shell", (["bin"], "zerocode-shell"), (["bin"], "zerocode-pick"),
                reads={"ui/shell.css": {"--bins"}, "crates/zerocode-core/src/account.rs": {"--bins"}}),
        package("both", "crates/both", (["lib"], "both"), (["bin"], "both-cli")),
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

    def test_a_library_is_tested_with_lib_a_bin_only_crate_with_bins_and_a_crate_with_both_with_both(self):
        picked = self.mod.crates(
            ["crates/zerocode-core/src/a.rs", "crates/zerocode-shell/src/main.rs", "crates/both/src/lib.rs"], WORKSPACES)
        self.assertEqual(picked, [
            (".", "both", "--lib --bins"), (".", "zerocode-core", "--lib"), (".", "zerocode-shell", "--bins")])

    def test_a_crate_whose_name_prefixes_a_sibling_does_not_own_the_siblings_files(self):
        picked = self.mod.crates(["crates/zerocode-shell-state/src/lib.rs"], WORKSPACES)
        self.assertEqual(picked, [(".", "zerocode-shell-state", "--lib")])

    def test_zo_ide_crates_belong_to_their_own_workspace(self):
        picked = self.mod.crates(["zo-ide/crates/runtime/src/lib.rs"], WORKSPACES)
        self.assertEqual(picked, [("zo-ide", "runtime", "--lib")])

    def test_a_crate_is_named_once_however_many_of_its_files_changed(self):
        picked = self.mod.crates(["crates/zerocode-core/a.rs", "crates/zerocode-core/b.rs"], WORKSPACES)
        self.assertEqual(len(picked), 1)

    def test_a_crate_with_several_bins_runs_all_of_them_and_one_with_no_target_none(self):
        picked = self.mod.crates(["tools/many/a.rs", "crates/bare/Cargo.toml"], WORKSPACES)
        self.assertEqual(picked, [(".", "tools-only", "--bins")])

    def test_paths_no_crate_holds_or_reads_pick_nothing(self):
        self.assertEqual(self.mod.crates(["justfile", "ui/other.css", "docs/a.md", "Cargo.lock"], WORKSPACES), [])

    def test_a_file_outside_a_crate_picks_the_crate_that_reads_it(self):
        # ui/shell.css is no package's file, but the shell's unit tests `include_str!` it
        self.assertEqual(self.mod.crates(["ui/shell.css"], WORKSPACES), [(".", "zerocode-shell", "--bins")])

    def test_a_file_in_one_crate_that_another_reads_picks_both(self):
        picked = self.mod.crates(["crates/zerocode-core/src/account.rs"], WORKSPACES)
        self.assertEqual(picked, [(".", "zerocode-core", "--lib"), (".", "zerocode-shell", "--bins")])

    def test_a_test_target_has_no_unit_tests_and_a_bin_source_has_only_the_bins(self):
        # tests/ is an integration target (stage 1 runs the shell's), src/main.rs and src/bin/ are the bins',
        # and src/lib.rs is what the bins link, so it picks both
        self.assertEqual(self.mod.crates(["crates/both/tests/contract.rs", "crates/both/benches/b.rs"], WORKSPACES), [])
        self.assertEqual(self.mod.crates(["crates/both/src/main.rs"], WORKSPACES), [(".", "both", "--bins")])
        self.assertEqual(self.mod.crates(["crates/both/src/bin/other.rs"], WORKSPACES), [(".", "both", "--bins")])
        self.assertEqual(self.mod.crates(["crates/both/src/lib.rs"], WORKSPACES), [(".", "both", "--lib --bins")])

    def test_a_file_a_bin_reads_picks_the_bins_and_one_a_lib_reads_picks_the_lib_and_the_bins_that_link_it(self):
        reading = {
            "both": package("both", "crates/both", (["lib"], "both"), (["bin"], "both-cli"),
                            reads={"data/bin.json": {"--bins"}, "data/lib.json": {"--lib", "--bins"}}),
        }
        found = {".": [reading["both"]]}
        self.assertEqual(self.mod.crates(["data/bin.json"], found), [(".", "both", "--bins")])
        self.assertEqual(self.mod.crates(["data/lib.json"], found), [(".", "both", "--lib --bins")])

    def test_a_build_script_reads_what_the_package_formats_at_run_time_so_none_of_it_is_seen(self):
        text = 'println!("cargo:rerun-if-changed=../../tools/y.json");\n'
        self.assertEqual(list(self.mod.reads_of(text, "crates/shell", "crates/shell")), [])

    def test_the_reads_of_a_source_are_the_paths_its_macros_name_relative_to_the_right_base(self):
        text = (
            '// include_str!("../../../ui/commented.css")\n'
            'const A: &str = include_str!("../../../../ui/shell.css");\n'
            'const B: &[u8] = include_bytes!("fixtures/own.bin");\n'
            'const C: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/x.md"));\n'
            'const D: &str = include_str!(concat!(env!("OUT_DIR"), "/generated.rs"));\n'
        )
        reads = set(self.mod.reads_of(text, "crates/shell/src/a", "crates/shell"))
        self.assertEqual(reads, {"ui/shell.css", "crates/shell/src/a/fixtures/own.bin", "skills/x.md"})

    def test_reads_leave_out_what_is_inside_the_package_the_repository_or_nothing(self):
        found = {"crates/shell/src/a.rs", "ui/shell.css", "../outside", ".", "crates/shell"}
        self.assertEqual(self.mod.outside_reads(found, "crates/shell"), ["ui/shell.css"])

    def test_a_suite_is_hinted_when_it_is_named_like_a_changed_test_or_runs_a_module_that_spells_a_changed_file(self):
        window = (
            'import { testWorkers } from "./workers.mjs";\n'
            'import { testPaint, testTokens } from "./paint.mjs";\n'
            'import "./side.mjs";\n'
            'suite("workers", () => testWorkers());\n'
            'suite("paint-tokens", () => testTokens());\n'
            'suite("paint-other", () => testPaint());\n'
            'suite("unrelated", () => 1);\n'
        )
        tests = {"workers": "", "paint": 'read("../shell.css")', "side": "", "window": window}
        self.assertEqual(self.mod.suites_for(["ui/tests/workers.mjs"], window, tests), ["workers"])
        self.assertEqual(self.mod.suites_for(["ui/shell.css"], window, tests), ["paint-other", "paint-tokens"])
        self.assertEqual(self.mod.suites_for(["ui/shell-new.css"], window, tests), [])

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
        self.write("crates/bin-crate/src/main.rs", 'const STYLE: &str = include_str!("../../../ui/style.css");\nfn main() {}\n')
        self.write("crates/mixed/Cargo.toml", '[package]\nname = "mixed"\nversion = "0.1.0"\nedition = "2021"\n')
        self.write("crates/mixed/src/lib.rs", 'const A: &str = include_str!("../../../data/lib.json");\n')
        self.write("crates/mixed/src/main.rs", 'const B: &str = include_str!("../../../data/bin.json");\nfn main() {}\n')
        self.write("crates/mixed/tests/contract.rs", 'const C: &str = include_str!("../../../AGENTS.md");\n')
        for name in ("data/lib.json", "data/bin.json", "AGENTS.md"):
            self.write(name, "")
        self.write("ui/style.css", "")
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

    def commit(self, message):
        self.git("add", ".")
        self.git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", message)

    def run_script(self, *arguments):
        done = subprocess.run([sys.executable, "-I", str(self.repo / "tools/release/changed_crates.py"), *arguments],
                              cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.splitlines()

    def test_a_tracked_edit_an_untracked_file_and_a_ui_edit_each_come_out_once(self):
        self.write("crates/lib-crate/src/lib.rs", "pub fn a() {}\n")
        self.write("crates/bin-crate/src/new.rs", "")
        self.write("ui/tests/inbox.mjs", "// changed\n")
        self.assertEqual(self.run_script(), [
            "crate\t.\tbin-crate\t--bins",
            "crate\t.\tlib-crate\t--lib",
            "ui\tui/tests/inbox.mjs",
            "suite\tinbox",
            "suites\t1\t2",
        ])

    def test_an_unchanged_branch_picks_nothing(self):
        self.assertEqual(self.run_script(), [])

    def test_a_ui_file_a_crate_includes_picks_that_crates_unit_tests_and_one_it_does_not_picks_none(self):
        self.write("ui/style.css", "a { color: red }\n")
        self.write("ui/unread.css", "")
        self.assertEqual(self.run_script(), [
            "crate\t.\tbin-crate\t--bins", "ui\tui/style.css", "ui\tui/unread.css", "suites\t0\t2"])

    def test_a_path_with_korean_letters_is_read_as_spelled_not_as_git_quotes_it(self):
        self.write("ui/한글.css", "")
        self.assertEqual(self.run_script(), ["ui\tui/한글.css", "suites\t0\t2"])

    def test_a_branch_cut_from_local_main_ahead_of_origin_main_does_not_count_the_landing(self):
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.write("crates/lib-crate/src/lib.rs", "pub fn landed() {}\n")
        self.commit("landed on main, not pushed")
        self.git("checkout", "-q", "-b", "work")
        self.write("crates/bin-crate/src/new.rs", "")
        self.assertEqual(self.run_script(), ["crate\t.\tbin-crate\t--bins"])

    def test_a_branch_cut_from_a_stack_tip_counts_from_the_base_it_is_given(self):
        self.git("checkout", "-q", "-b", "stack")
        self.write("crates/lib-crate/src/lib.rs", "pub fn stacked() {}\n")
        self.commit("the stack's change")
        self.write("crates/bin-crate/src/new.rs", "")
        self.assertEqual(self.run_script(), ["crate\t.\tbin-crate\t--bins", "crate\t.\tlib-crate\t--lib"])
        self.assertEqual(self.run_script("--base", "stack"), ["crate\t.\tbin-crate\t--bins"])

    def test_a_file_only_a_test_target_reads_picks_no_unit_tests(self):
        # tests/contract.rs reads AGENTS.md; the contracts are an integration target, not the bins' unit tests
        self.write("AGENTS.md", "a line\n")
        self.assertEqual(self.run_script(), [])

    def test_the_kind_of_source_that_reads_a_file_picks_the_unit_tests_of_that_kind(self):
        self.write("data/bin.json", "{}\n")
        self.assertEqual(self.run_script(), ["crate\t.\tmixed\t--bins"])
        self.write("data/bin.json", "")
        self.write("data/lib.json", "{}\n")
        self.assertEqual(self.run_script(), ["crate\t.\tmixed\t--lib --bins"])

    def test_a_changed_integration_test_picks_no_unit_tests(self):
        self.write("crates/mixed/tests/contract.rs", "// edited\n")
        self.assertEqual(self.run_script(), [])

    def test_the_other_cargo_workspaces_are_named_by_the_script_alone(self):
        self.assertEqual(self.run_script("--workspaces"), [])
        self.write("zo-ide/Cargo.toml", "[workspace]\n")
        self.assertEqual(self.run_script("--workspaces"), ["zo-ide"])


if __name__ == "__main__":
    unittest.main()
