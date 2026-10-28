#!/usr/bin/env python3
"""What `just pre-handin` should run for the paths a branch changed.

    changed_crates.py [--base REV]

prints one fact a line, tab-separated, for the recipe to read:

    crate <workspace dir> <package> <cargo test arguments>
    ui <changed path under ui/>
    suite <window suite named like a changed ui/tests/<suite>.mjs>

The paths are the working tree against the merge base with origin/main (or
main), plus untracked files — what the branch would hand in. A crate is a
package of either Cargo workspace (the root one and zo-ide's) that owns a
changed path: the owner is found from `cargo metadata`, the longest manifest
directory that prefixes the path, so no list of crates is kept here. Its unit
tests are `--lib` when it has a library, else the binary named like the package
(`zerocode-shell`), else `--bins`.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
WORKSPACES = (".", "zo-ide")     # the two Cargo workspaces; zo-ide is not a member of the root's
WINDOW = "ui/tests/window.mjs"   # where the window suites are registered, `suite("<name>", …)`
SUITE = re.compile(r'^\s*suite\(\s*"([^"]+)"', re.M)


def git(*args):
    done = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"changed_crates: git {' '.join(args)}: {done.stderr.strip()}")
    return done.stdout


def merge_base(base=None):
    """The commit the branch left main at: `--base`, else origin/main, else main."""
    for rev in ([base] if base else ["origin/main", "main"]):
        done = subprocess.run(["git", "merge-base", rev, "HEAD"], cwd=REPO, capture_output=True, text=True)
        if done.returncode == 0:
            return done.stdout.strip()
    raise SystemExit(f"changed_crates: no merge base with {base or 'origin/main or main'}")


def changed_paths(base):
    tracked = git("diff", "--name-only", base).splitlines()
    untracked = git("ls-files", "--others", "--exclude-standard").splitlines()
    return sorted(set(tracked) | set(untracked))


def packages(workspace):
    """Each package of one workspace: its name, its directory from the repo root, its targets."""
    done = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=REPO / workspace, capture_output=True, text=True,
    )
    if done.returncode != 0:
        raise SystemExit(f"changed_crates: cargo metadata in {workspace}: {done.stderr.strip()}")
    found = []
    for package in json.loads(done.stdout)["packages"]:
        directory = Path(package["manifest_path"]).parent.resolve().relative_to(REPO.resolve())
        targets = [(target["kind"], target["name"]) for target in package["targets"]]
        found.append({"name": package["name"], "dir": directory.as_posix(), "targets": targets})
    return found


def test_arguments(package):
    """The `cargo test` arguments for a package's unit tests, or None when it has no tests to run."""
    kinds = [kind for kinds, _ in package["targets"] for kind in kinds]
    if "lib" in kinds:
        return "--lib"
    bins = [name for kinds, name in package["targets"] if "bin" in kinds]
    if package["name"] in bins:
        return f"--bin {package['name']}"
    return "--bins" if bins else None


def owner(path, packages_by_workspace):
    """(workspace dir, package) owning `path`: the longest package directory that prefixes it."""
    best = None
    for workspace, found in packages_by_workspace.items():
        for package in found:
            inside = path == package["dir"] or path.startswith(package["dir"] + "/")
            if inside and (best is None or len(package["dir"]) > len(best[1]["dir"])):
                best = (workspace, package)
    return best


def crates(paths, packages_by_workspace):
    """The (workspace dir, package name, cargo test arguments) of every crate the paths touch, once."""
    chosen = {}
    for path in paths:
        hit = owner(path, packages_by_workspace)
        if hit is None:
            continue
        workspace, package = hit
        arguments = test_arguments(package)
        if arguments is not None:
            chosen[(workspace, package["name"])] = arguments
    return [(workspace, name, arguments) for (workspace, name), arguments in sorted(chosen.items())]


def suites_named_like(paths, registered):
    """Window suites whose name is a changed `ui/tests/<name>.mjs` — a hint, never the whole answer."""
    names = {Path(path).stem for path in paths if path.startswith("ui/tests/") and path.endswith(".mjs")}
    return sorted(names & set(registered))


def ui_paths(paths):
    return [path for path in paths if path.startswith("ui/")]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="the revision to measure from (default: merge base with origin/main, else main)")
    args = parser.parse_args(argv)
    paths = changed_paths(merge_base(args.base))
    by_workspace = {workspace: packages(workspace) for workspace in WORKSPACES if (REPO / workspace / "Cargo.toml").is_file()}
    for workspace, name, arguments in crates(paths, by_workspace):
        print("crate", workspace, name, arguments, sep="\t")
    for path in ui_paths(paths):
        print("ui", path, sep="\t")
    window = REPO / WINDOW
    registered = SUITE.findall(window.read_text(encoding="utf-8")) if window.is_file() else []
    for name in suites_named_like(paths, registered):
        print("suite", name, sep="\t")


if __name__ == "__main__":
    sys.exit(main())
