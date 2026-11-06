#!/usr/bin/env python3
"""zo-ide-touched.py: whether a change reaches what zo-ide's CI job builds and
tests, read from a made-up repository's manifests and sources — and `true`
whenever the script is in doubt.

Run with: python3 tools/release/tests/test_zo_ide_touched.py
"""

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "zo-ide-touched.py"

FILES = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/core", "crates/other"]\n',
    "crates/core/Cargo.toml": '[package]\nname = "core"\n\n[dependencies]\nleaf = { path = "../leaf" }\n',
    "crates/core/src/lib.rs": "pub fn core() {}\n",
    "crates/leaf/Cargo.toml": '[package]\nname = "leaf"\n',
    "crates/leaf/src/lib.rs": "pub fn leaf() {}\n",
    "crates/other/Cargo.toml": '[package]\nname = "other"\n',
    "crates/other/src/lib.rs": "pub fn other() {}\n",
    "zo-ide/Cargo.toml": (
        '[workspace]\nmembers = ["crates/zo"]\n\n[workspace.dependencies]\ncore = { path = "../crates/core" }\n'
    ),
    "zo-ide/crates/zo/Cargo.toml": '[package]\nname = "zo"\n\n[dependencies]\ncore = { workspace = true }\n',
    "zo-ide/crates/zo/build.rs": 'fn main() {\n    println!("cargo:rerun-if-changed=../../../ui/feed.json");\n}\n',
    "zo-ide/crates/zo/src/lib.rs": (
        'pub const SKILL: &str = include_str!("../../../../skills/a/SKILL.md");\n'
        "pub fn data() -> std::path::PathBuf {\n"
        '    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/data")\n'
        "}\n"
    ),
    "skills/a/SKILL.md": "a\n",
    "skills/b/SKILL.md": "b\n",
    "fixtures/data/x.json": "{}\n",
    "fixtures/other/y.json": "{}\n",
    "ui/feed.json": "[]\n",
    "ui/app.js": "app();\n",
    "docs/readme.md": "readme\n",
}


class ZoIdeTouched(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name) / "repo"
        for name, text in FILES.items():
            self.write(name, text)
        config = Path(self._tmp.name) / "gitconfig"
        config.write_text("")
        self.env = dict(
            os.environ,
            GIT_CONFIG_GLOBAL=str(config),
            GIT_CONFIG_NOSYSTEM="1",
            GIT_AUTHOR_NAME="test",
            GIT_AUTHOR_EMAIL="test@example.invalid",
            GIT_COMMITTER_NAME="test",
            GIT_COMMITTER_EMAIL="test@example.invalid",
        )
        self.git("init", "-q")
        self.commit("the made-up repository")

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.repo, env=self.env, check=True, capture_output=True, text=True
        ).stdout.strip()

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "--no-verify", "-m", message)
        return self.git("rev-parse", "HEAD")

    def answer(self, base):
        said = subprocess.run(
            [sys.executable, str(SCRIPT), base, "HEAD"], cwd=self.repo, env=self.env, capture_output=True, text=True
        )
        self.assertEqual(said.returncode, 0, said.stderr)
        return said.stdout.strip()

    def test_a_change_reaches_zo_ide_only_through_what_it_reads(self):
        for name, read in (
            ("zo-ide/crates/zo/src/lib.rs", True),  # zo-ide itself
            ("crates/core/src/lib.rs", True),  # a crate it names by path, through its workspace
            ("crates/leaf/src/lib.rs", True),  # and what that crate names
            ("skills/a/SKILL.md", True),  # include_str!
            ("fixtures/data/x.json", True),  # a `..` path in a source that finds its crate
            ("ui/feed.json", True),  # a build script's rerun-if-changed
            ("Cargo.toml", True),  # the root manifest the root crates inherit from
            ("crates/other/src/lib.rs", False),
            ("skills/b/SKILL.md", False),
            ("fixtures/other/y.json", False),
            ("ui/app.js", False),
            ("docs/readme.md", False),
        ):
            with self.subTest(changed=name):
                before = self.git("rev-parse", "HEAD")
                self.write(name, (self.repo / name).read_text() + "\n")
                self.commit(f"{name} changed")
                self.assertEqual(self.answer(before), "true" if read else "false")

    def test_a_doubt_runs_zo_ide(self):
        self.write("crates/leaf/Cargo.toml", "[package\nname =\n")
        broken = self.commit("a manifest that does not parse")
        self.write("ui/app.js", "app(); // again\n")
        self.commit("a window change alone")
        self.assertEqual(self.answer(broken), "true", "a manifest that does not parse")

        self.write("crates/leaf/Cargo.toml", FILES["crates/leaf/Cargo.toml"])
        self.write(
            "zo-ide/crates/zo/src/lib.rs",
            FILES["zo-ide/crates/zo/src/lib.rs"] + 'static UI: Dir = include_dir!("../../../../ui");\n',
        )
        unknown = self.commit("a macro whose name says it includes")
        self.write("ui/app.js", "app(); // and again\n")
        self.commit("another window change alone")
        self.assertEqual(self.answer(unknown), "true", "an include-like macro the script does not know")

        self.assertEqual(self.answer("0" * 40), "true", "a base the history lacks")


if __name__ == "__main__":
    unittest.main(verbosity=2)
