#!/usr/bin/env python3
"""What `just pre-handin` should run for the paths a branch changed.

    changed_crates.py [--base REV]

prints one fact a line, tab-separated, for the recipe to read:

    crate <workspace dir> <package> <cargo test arguments>
    ui <changed path under ui/>
    suite <window suite that looks at a changed ui file: named like it, or run by a test module that spells its name>

The paths are the working tree against the merge base with main (else
origin/main), plus untracked files — what the branch would hand in. A crate is
a package of either Cargo workspace (the root one and zo-ide's) that owns a
changed path, in one of two ways, and no list of crates or folders is kept here:

  - the path is inside the package's directory (the longest manifest directory
    from `cargo metadata` that prefixes it);
  - the package reads the path from outside its directory: its sources name it
    in `include_str!`/`include_bytes!`/`include!`, in
    `concat!(env!("CARGO_MANIFEST_DIR"), "/…")` or in a build script's
    `cargo:rerun-if-changed=`. Those are the files read at compile time, so a
    test that lists a folder at run time is not seen.

Its unit tests are `--lib` when it has a library and `--bins` when it has
binaries, both when it has both.
"""

import argparse
import json
import posixpath
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
WORKSPACES = (".", "zo-ide")     # the two Cargo workspaces; zo-ide is not a member of the root's
WINDOW = "ui/tests/window.mjs"   # where the window suites are registered, `suite("<name>", …)`
SUITE = re.compile(r'^\s*suite\(\s*"([^"]+)"', re.M)
IMPORT = re.compile(r'^import\s*(?:\{([^}]*)\}\s*from\s*)?"\./([^"]+)\.mjs"', re.M)
# What a crate's sources read from disk, each a quoted path (no `{}` format holes):
MANIFEST = r'env!\(\s*"CARGO_MANIFEST_DIR"\s*\)'
FILE_READS = re.compile(r'\binclude(?:_str|_bytes)?!\s*\(\s*"([^"{}]+)"')          # beside the source file
MANIFEST_READS = re.compile(r'concat!\(\s*' + MANIFEST + r'\s*,\s*"([^"{}]+)"')    # under the package dir
RERUN_READS = re.compile(r'rerun-if-changed=([^"\\\s{}]+)')                          # a build script's inputs


def git(*args):
    """git's output split on NUL: `-z` keeps a path with non-ASCII letters as spelled, not quoted."""
    done = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"changed_crates: git {' '.join(args)}: {done.stderr.strip()}")
    return [item for item in done.stdout.split("\0") if item]


def merge_base(base=None):
    """The commit the branch left main at: `--base`, else main, else origin/main.

    Local main first: between a landing and its push it is ahead of origin/main,
    and a branch cut then must not count the landing as its own.
    """
    for rev in ([base] if base else ["main", "origin/main"]):
        done = subprocess.run(["git", "merge-base", rev, "HEAD"], cwd=REPO, capture_output=True, text=True)
        if done.returncode == 0:
            return done.stdout.strip()
    raise SystemExit(f"changed_crates: no merge base with {base or 'main or origin/main'}")


def changed_paths(base):
    tracked = git("diff", "--name-only", "-z", base)
    untracked = git("ls-files", "-z", "--others", "--exclude-standard")
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
    kinds = {kind for kinds, _ in package["targets"] for kind in kinds}
    arguments = [flag for flag, kind in (("--lib", "lib"), ("--bins", "bin")) if kind in kinds]
    return " ".join(arguments) or None


def inside(path, directory):
    return path == directory or path.startswith(directory + "/")


def own(path, packages_by_workspace):
    """(workspace dir, package) whose directory holds `path`: the longest one that prefixes it."""
    best = None
    for workspace, found in packages_by_workspace.items():
        for package in found:
            if inside(path, package["dir"]) and (best is None or len(package["dir"]) > len(best[1]["dir"])):
                best = (workspace, package)
    return best


def reads_of(text, directory, package_dir):
    """The repo-relative paths one source file names: its `directory` for the file-relative macros, `package_dir` for the rest."""
    code = "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("//"))
    for pattern, base in ((FILE_READS, directory), (MANIFEST_READS, package_dir), (RERUN_READS, package_dir)):
        for name in pattern.findall(code):
            yield posixpath.normpath(posixpath.join(base, name.lstrip("/")))


def outside_reads(found, own_dir):
    """What a package reads from outside its own directory (and inside the repository), as sorted paths."""
    return sorted({path for path in found if path not in (".", "") and not path.startswith("..") and not inside(path, own_dir)})


def attach_reads(packages_by_workspace):
    """Give each package its "reads": the paths outside its directory that its Rust sources name."""
    sources = [path for path in git("ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "*.rs")]
    found = {}
    for source in sources:
        hit = own(source, packages_by_workspace)
        if hit is None or not (REPO / source).is_file():
            continue
        text = (REPO / source).read_text(encoding="utf-8", errors="replace")
        found.setdefault((hit[0], hit[1]["name"]), set()).update(
            reads_of(text, posixpath.dirname(source), hit[1]["dir"]))

    for workspace, packages in packages_by_workspace.items():
        for package in packages:
            package["reads"] = outside_reads(found.get((workspace, package["name"]), ()), package["dir"])


def owners(path, packages_by_workspace):
    """(workspace dir, package) pairs a change to `path` can break: its directory's package, and each package that reads it."""
    hit = own(path, packages_by_workspace)
    found = [hit] if hit else []
    for workspace, packages in packages_by_workspace.items():
        for package in packages:
            if (workspace, package) not in found and any(inside(path, read) for read in package.get("reads", ())):
                found.append((workspace, package))
    return found


def crates(paths, packages_by_workspace):
    """The (workspace dir, package name, cargo test arguments) of every crate the paths touch, once."""
    chosen = {}
    for path in paths:
        for workspace, package in owners(path, packages_by_workspace):
            arguments = test_arguments(package)
            if arguments is not None:
                chosen[(workspace, package["name"])] = arguments
    return [(workspace, name, arguments) for (workspace, name), arguments in sorted(chosen.items())]


def suites_for(paths, window_text, tests):
    """Window suites that look at a changed ui file — a hint, never the whole answer.

    `tests` maps a ui/tests module's stem to its text. A module looks at a
    changed file when it is that file or spells its name (`shell.css`); a suite
    is the one named like the module, or the one registered in window.mjs with a
    function the module exports (named in window.mjs's import of it).
    """
    changed = [path for path in paths if path.startswith("ui/")]
    names = {Path(path).name for path in changed}
    modules = {stem for stem, text in tests.items()
               if f"ui/tests/{stem}.mjs" in changed or any(name in text for name in names)}
    registered = [(match.group(1), match.start()) for match in SUITE.finditer(window_text)]
    bodies = {name: window_text[at:(registered[i + 1][1] if i + 1 < len(registered) else len(window_text))]
              for i, (name, at) in enumerate(registered)}
    picked = {name for name in bodies if name in modules}
    for imported, stem in IMPORT.findall(window_text):
        if stem not in modules:
            continue
        functions = [part.split(" as ")[-1].strip() for part in imported.split(",") if part.strip()]
        picked.update(name for name, body in bodies.items()
                      if any(re.search(rf"\b{re.escape(function)}\b", body) for function in functions))
    return sorted(picked)


def ui_paths(paths):
    return [path for path in paths if path.startswith("ui/")]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="the revision to measure from (default: merge base with main, else origin/main)")
    args = parser.parse_args(argv)
    paths = changed_paths(merge_base(args.base))
    by_workspace = {workspace: packages(workspace) for workspace in WORKSPACES if (REPO / workspace / "Cargo.toml").is_file()}
    attach_reads(by_workspace)
    for workspace, name, arguments in crates(paths, by_workspace):
        print("crate", workspace, name, arguments, sep="\t")
    ui = ui_paths(paths)
    for path in ui:
        print("ui", path, sep="\t")
    window = REPO / WINDOW
    if ui and window.is_file():
        tests = {path.stem: path.read_text(encoding="utf-8", errors="replace")
                 for path in window.parent.glob("*.mjs") if path != window}
        for name in suites_for(paths, window.read_text(encoding="utf-8"), tests):
            print("suite", name, sep="\t")


if __name__ == "__main__":
    sys.exit(main())
