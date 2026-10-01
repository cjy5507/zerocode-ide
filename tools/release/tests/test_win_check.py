#!/usr/bin/env python3
"""Exercise both real Windows gate recipes with isolated tool installations."""

import os
import shlex
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[3]
JUST = shutil.which("just")
LLVM_LIB = "/opt/homebrew/opt/llvm/bin/llvm-lib"
XWIN_TARGET_ARGS = ["--workspace", "--all-targets", "--target", "x86_64-pc-windows-msvc"]
# The root recipe lints with the CI leg's `-D warnings`; zo-ide's still only checks.
XWIN_ARGS = {
    "justfile": ["xwin", "clippy", *XWIN_TARGET_ARGS, "--", "-D", "warnings"],
    "zo-ide/justfile": ["xwin", "check", *XWIN_TARGET_ARGS],
}


@unittest.skipUnless(os.name == "posix" and JUST, "the local cross-check uses just and a Unix shell")
class WindowsRecipeTests(unittest.TestCase):
    def run_recipe(self, source, *, lane=True, warm=False, external=False, cargo=True, llvm=True, exit_code=0):
        with tempfile.TemporaryDirectory(prefix="win-check-recipe-") as name:
            root = Path(name)
            project = root / "project"
            project.mkdir()
            tools = root / "bin"
            tools.mkdir()
            llvm_lib = tools / "llvm-lib"
            if llvm:
                llvm_lib.touch()
                llvm_lib.chmod(0o755)
            if cargo:
                executable = tools / "cargo-xwin"
                executable.write_text("#!/bin/sh\nexit 0\n")
                executable.chmod(0o755)
            receipt = root / "receipt"
            (tools / "just").symlink_to(JUST)
            child = tools / "cargo"
            child.write_text(
                '#!/bin/sh\nprintf "%s\\n" "$@" > "$WIN_CHECK_TEST_RECEIPT"\n'
                'exit "$WIN_CHECK_TEST_EXIT"\n'
            )
            child.chmod(0o755)
            # Only the installation path changes. The actual recipe and its
            # recursive call execute as shipped, without changing host tools.
            text = source.read_text()
            self.assertIn(LLVM_LIB, text)
            justfile = project / "justfile"
            lines = text.replace(LLVM_LIB, shlex.quote(str(llvm_lib))).splitlines()
            lines = ["verify: gate-probe-before win-check-if-available gate-probe-after"
                     if line.startswith("verify:") else line for line in lines]
            justfile.write_text("\n".join(lines) + "\n\ngate-probe-before:\n    echo before\n"
                               "\ngate-probe-after:\n    echo after\n")
            env = os.environ.copy()
            env.pop("CARGO_TARGET_DIR", None)
            env.update(
                PATH=f"{tools}:/usr/bin:/bin",
                WIN_CHECK_TEST_RECEIPT=str(receipt),
                WIN_CHECK_TEST_EXIT=str(exit_code if cargo and llvm else 127),
            )
            target = project / "target"
            if external:
                target = root / "external-target"
                env["CARGO_TARGET_DIR"] = str(target)
            if warm:
                (target / "x86_64-pc-windows-msvc").mkdir(parents=True)
            command = (["bash", str(REPO / "tools/release/verify-every-recipe.sh")] if lane else
                       [JUST, "--justfile", str(justfile), "win-check-if-available"])
            result = subprocess.run(command, cwd=project, env=env, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            called = receipt.read_text().splitlines() if receipt.exists() else []
            return result, called

    def check_recipes(self, *, skipped=False, failed=False, **options):
        for recipe, expected in XWIN_ARGS.items():
            with self.subTest(recipe=recipe, **options):
                result, called = self.run_recipe(REPO / recipe, **options)
                if failed:
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                else:
                    self.assertEqual(result.returncode, 0, result.stdout)
                self.assertEqual(called, [] if skipped else expected)
                if options.get("lane", True):
                    self.assertIn("<== verify recipe win-check rc=", result.stdout)
                    self.assertIn("after", result.stdout)
                if skipped:
                    self.assertIn("SKIPPED", result.stdout)
                elif not failed:
                    self.assertNotIn("SKIPPED", result.stdout)

    def test_available_tools_run_with_a_cold_target(self):
        self.check_recipes()

    def test_available_tools_run_with_a_warm_target(self):
        self.check_recipes(warm=True)

    def test_external_target_does_not_need_to_be_warm(self):
        self.check_recipes(external=True)

    def test_a_failed_cross_check_is_not_a_success(self):
        self.check_recipes(exit_code=23, failed=True)

    def test_development_missing_cargo_xwin_skips_aloud(self):
        self.check_recipes(lane=False, cargo=False, skipped=True)

    def test_development_missing_llvm_skips_aloud(self):
        self.check_recipes(lane=False, llvm=False, skipped=True)

    def test_development_cold_target_still_skips_aloud(self):
        self.check_recipes(lane=False, skipped=True)

    def test_release_requires_cargo_xwin(self):
        self.check_recipes(cargo=False, failed=True)

    def test_release_requires_llvm(self):
        self.check_recipes(llvm=False, failed=True)

    def test_the_lane_names_a_failed_cross_check_as_a_recipe_failure(self):
        for recipe, expected in XWIN_ARGS.items():
            with self.subTest(recipe=recipe):
                result, called = self.run_recipe(REPO / recipe, exit_code=23)
                self.assertEqual(called, expected)
                with tempfile.TemporaryDirectory(prefix="win-check-log-") as name:
                    log = Path(name) / "gate.log"
                    log.write_text(result.stdout)
                    command = ('eval "$(sed -n \'/^failed_tests()/,/^}/p\' "$0")"; '
                               'failed_tests "$1"')
                    parsed = subprocess.run(
                        ["bash", "-c", command, str(REPO / "tools/release/lane.sh"), str(log)],
                        text=True, capture_output=True, check=True,
                    )
                self.assertEqual(parsed.stdout.split(), ["recipe:win-check"])


if __name__ == "__main__":
    unittest.main()
