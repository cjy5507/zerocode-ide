#!/usr/bin/env python3
"""workspace_units.py: before its gates the release lane drops the build units
of the crates this repository defines, which a fresh clone rebuilds anyway.

Run with: python3 tools/release/tests/test_workspace_units.py
"""

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "workspace_units.py"


def load():
    spec = importlib.util.spec_from_file_location("workspace_units", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


class Names(unittest.TestCase):
    def setUp(self):
        self.repo = Path(tempfile.mkdtemp(prefix="workspace-units-names-"))
        self.addCleanup(shutil.rmtree, self.repo, ignore_errors=True)

    def test_every_package_the_repository_defines_is_its_own(self):
        write(self.repo / "Cargo.toml", '[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "1.2.3"\n')
        write(self.repo / "crates/core/Cargo.toml",
              '[package]\nname = "zerocode-core"\nversion.workspace = true\n\n[lib]\nname = "zerocode_core_lib"\n')
        write(self.repo / "zo-ide/crates/tools/Cargo.toml", '[package]\nname = "tools"\nversion = "0.1.0"\n')
        write(self.repo / "target/debug/build/copy/Cargo.toml", '[package]\nname = "a-built-copy"\n')
        write(self.repo / "node_modules/pkg/Cargo.toml", '[package]\nname = "from-npm"\n')
        self.assertEqual(load().package_names(self.repo), {"zerocode-core", "tools"})

    def test_a_name_a_registry_crate_also_has_is_never_taken_for_the_repositorys(self):
        write(self.repo / "a/Cargo.toml", '[package]\nname = "tools"\nversion = "0.1.0"\n')
        write(self.repo / "b/Cargo.toml", '[package]\nname = "app"\nversion = "0.1.0"\n')
        write(self.repo / "Cargo.lock", 'version = 4\n\n[[package]]\nname = "app"\nversion = "0.1.0"\n\n'
              '[[package]]\nname = "tools"\nversion = "0.3.0"\n'
              'source = "registry+https://github.com/rust-lang/crates.io-index"\n')
        self.assertEqual(load().package_names(self.repo), {"app"})


class Foreign(unittest.TestCase):
    """A build-script run that ran in another target (its `root-output` is not its own `out`) goes, with
    its fingerprint; a run of this target, in any profile, stays."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="workspace-units-foreign-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.target = self.tmp / "root-gate"

    def run_unit(self, profile, unit, output, ran_in=None):
        folder = self.target / profile / "build" / unit
        write(folder / "output", output)
        write(folder / "out" / "made", "a build product")
        write(folder / "root-output", str(ran_in or folder) + "/out")
        write(self.target / profile / ".fingerprint" / unit / "run-build-script-build-script-build.json", "{}")

    def test_a_run_copied_from_another_target_goes_and_one_of_this_target_stays(self):
        gone = self.tmp / "worktree" / "target" / "debug"
        own = self.target / "debug"
        cross = self.target / "x86_64-pc-windows-msvc" / "debug"
        self.run_unit("debug", "tauri-71514b9904e12dce",
                      f"cargo:rustc-cfg=desktop\ncargo:PERMISSION_FILES_PATH={gone}/build/tauri-71514b9904e12dce/out/permissions/a.toml\n",
                      ran_in=gone / "build" / "tauri-71514b9904e12dce")
        self.run_unit("debug", "aws-lc-sys-a07a165b539ceaf6", "cargo:rustc-link-lib=static=crypto\n",
                      ran_in=gone / "build" / "aws-lc-sys-a07a165b539ceaf6")
        self.run_unit("debug", "ring-976e7dd2a29890b7", f"cargo:rustc-link-search=native={own}/build/ring-976e7dd2a29890b7/out\n")
        self.run_unit("x86_64-pc-windows-msvc/debug", "windows-0123456789abcdef",
                      f"cargo:rustc-link-search=native={cross}/build/windows-0123456789abcdef/out\n"
                      f"cargo:rustc-env=HOST={own}/build/ring-976e7dd2a29890b7/out\n")
        self.run_unit("debug", "serde-fedcba9876543210", "cargo:rerun-if-changed=build.rs\ncargo:rustc-cfg=no_core\n")
        write(self.target / "debug" / "build" / "serde-0000000000000000" / "build-script-build", "a compiled script")

        units, freed = load().drop_foreign(self.target)

        self.assertEqual(units, 2)
        self.assertGreater(freed, 0)
        for unit in ("tauri-71514b9904e12dce", "aws-lc-sys-a07a165b539ceaf6"):
            self.assertFalse((own / "build" / unit).exists(), unit)
            self.assertFalse((own / ".fingerprint" / unit).exists(), unit)
        for profile, unit in (("debug", "ring-976e7dd2a29890b7"), ("debug", "serde-fedcba9876543210"),
                              ("x86_64-pc-windows-msvc/debug", "windows-0123456789abcdef")):
            self.assertTrue((self.target / profile / "build" / unit / "out" / "made").exists(), unit)
            self.assertTrue((self.target / profile / ".fingerprint" / unit).exists(), unit)
        self.assertTrue((own / "build" / "serde-0000000000000000" / "build-script-build").exists())

    def test_the_command_says_what_it_removed_and_a_missing_target_is_nothing(self):
        self.run_unit("debug", "tauri-71514b9904e12dce", "cargo:X=1\n",
                      ran_in=self.tmp / "elsewhere" / "debug" / "build" / "tauri-71514b9904e12dce")
        r = subprocess.run([sys.executable, str(SCRIPT), "foreign", "--target", str(self.target)],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.split()[0], "1")
        r = subprocess.run([sys.executable, str(SCRIPT), "foreign", "--target", str(self.tmp / "none")],
                           capture_output=True, text=True)
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "0 0"))


@unittest.skipUnless(shutil.which("cargo"), "cargo is not on PATH")
class OnARealBuild(unittest.TestCase):
    """A real, tiny, offline cargo build: the repository's units go, a crate
    from outside it stays, and cargo rebuilds exactly what went."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="workspace-units-build-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.repo, self.target = self.tmp / "repo", self.tmp / "target"
        # `dep` stands in for a registry crate: it lives outside the repository.
        write(self.tmp / "outside/dep/Cargo.toml", '[package]\nname = "dep"\nversion = "0.1.0"\nedition = "2021"\n')
        write(self.tmp / "outside/dep/src/lib.rs", "pub fn one() -> u32 { 1 }\n")
        write(self.repo / "Cargo.toml", '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
              '[dependencies]\ndep = { path = "../outside/dep" }\n')
        write(self.repo / "src/lib.rs", "pub fn two() -> u32 { dep::one() + 1 }\n")
        write(self.repo / "tests/adds.rs", "#[test]\nfn adds() { assert_eq!(app::two(), 2); }\n")

    def cargo(self):
        # The log is read for cargo's words, so they are asked for without colour: CI sets CARGO_TERM_COLOR=always.
        env = dict(os.environ, CARGO_TARGET_DIR=str(self.target), CARGO_INCREMENTAL="0", CARGO_TERM_COLOR="never")
        r = subprocess.run(["cargo", "test", "--no-run", "--offline", "-v", "--manifest-path", str(self.repo / "Cargo.toml")],
                           capture_output=True, text=True, env=env, timeout=300)
        self.assertEqual(r.returncode, 0, r.stderr)
        return r.stderr

    def test_the_repositorys_units_go_and_cargo_rebuilds_only_them(self):
        self.cargo()
        dep = next((self.target / "debug" / "deps").glob("libdep-*.rlib"))
        built = dep.stat().st_mtime_ns
        r = subprocess.run([sys.executable, str(SCRIPT), "drop", "--target", str(self.target), "--scratch", str(self.repo)],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        units, size = (int(v) for v in r.stdout.split())
        self.assertGreaterEqual(units, 2, "the lib and the test binaries")
        self.assertGreater(size, 0)
        left = sorted(p.name for p in (self.target / "debug").rglob("*") if p.name.startswith(("app-", "libapp-", "adds-")))
        self.assertEqual(left, [], "a unit of the repository's own crate stayed")
        self.assertTrue(dep.exists(), "the outside crate's unit went")
        log = self.cargo()
        self.assertIn("Fresh dep v0.1.0", log)
        self.assertIn("Compiling app v0.1.0", log)
        self.assertEqual(dep.stat().st_mtime_ns, built, "the outside crate was rebuilt")


if __name__ == "__main__":
    unittest.main()
