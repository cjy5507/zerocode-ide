#!/usr/bin/env python3
"""Whether a change reaches what zo-ide's CI job builds and tests.

usage: zo-ide-touched.py <base> [<head>]   (from the repository's root)

Prints `true` when a file that changed between <base> and <head> (HEAD when
not given) is one zo-ide's build or tests read, and `false` only when every
one of them was read and none changed. What zo-ide reads is found anew each
run, from the manifests and the sources, so there is no list here to keep up:
- zo-ide/ itself, the workflow, the recipe runner and this script;
- the root manifest and `.cargo/`, which the root crates below inherit from;
- a crate outside zo-ide/ that a manifest names by `path`, and what that
  crate's manifest names in turn;
- what a source of those crates (or of zo-ide) names: a file in
  `include_str!`, `include_bytes!` or `include!`, from the source's folder; a
  `..` path in a source that finds its crate by `CARGO_MANIFEST_DIR`, and the
  folders its `.parent()` and `.ancestors().nth(n)` climb to, from the crate;
  a build script's `rerun-if-changed`, from the crate.
A path that names the repository's root as a whole says nothing about which
file is read, so it is not counted: the two such reads today are replays run
only by hand (`#[ignore]`). Nor is a rerun path a build script formats at run
time: zo-ide's name git's own files, and zerocode-core's names the `skills`
folder that script also spells as a literal.
Any doubt prints `true`: a commit the history lacks, a manifest that does not
parse, a macro whose name says it includes and is none of the three above.
"""
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ALWAYS = (
    "zo-ide",
    ".gitattributes",
    ".github/workflows/verify.yml",
    "tools/release/verify-every-recipe.sh",
    "tools/release/zo-ide-touched.py",
    "Cargo.toml",
    ".cargo",
)
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")
INCLUDES = {"include", "include_str", "include_bytes"}
INCLUDE_LIKE = re.compile(r"\b(\w*include\w*)!\s*[(\[{]")
INCLUDED = re.compile(r'include(?:_str|_bytes)?!\(\s*"([^"]+)"\s*\)')
UPWARD = re.compile(r'"(/?\.\.(?:/[^"]*)?)"')
CLIMB = re.compile(r'CARGO_MANIFEST_DIR"[^;{}]*')
ANCESTOR = re.compile(r"\.ancestors\(\)\s*\.nth\((\d+)\)")
RERUN = re.compile(r'rerun-if-changed=([^"{}\s]+)"')


class Doubt(Exception):
    """Something zo-ide may read that this script cannot tell."""


def normal(path):
    return os.path.normpath(str(path))


def within(path, folder):
    return path == folder or path.startswith(folder + os.sep)


def specs(manifest):
    """Every dependency a manifest names, with the table it was named in."""
    tables = [manifest.get(name, {}) for name in DEPENDENCY_TABLES]
    for target in manifest.get("target", {}).values():
        tables += [target.get(name, {}) for name in DEPENDENCY_TABLES]
    tables.append(manifest.get("workspace", {}).get("dependencies", {}))
    tables += manifest.get("patch", {}).values()
    for table in tables:
        yield from table.items()


def crate_of(source, root):
    """The folder of the nearest manifest above a source: its crate."""
    for folder in Path(source).parents:
        if (folder / "Cargo.toml").is_file():
            return folder
        if normal(folder) == root:
            break
    return None


def named_by(source, root):
    """The paths a source names, as the module's docstring says."""
    text = Path(source).read_text(errors="replace")
    unknown = sorted(set(INCLUDE_LIKE.findall(text)) - INCLUDES)
    if unknown:
        raise Doubt(f"{os.path.relpath(source, root)} calls {unknown[0]}!, which may read a file")
    found = [Path(source).parent / name for name in INCLUDED.findall(text)]
    crate = crate_of(source, root)
    if crate is not None:
        found += [crate / name for name in RERUN.findall(text)]
        if "CARGO_MANIFEST_DIR" in text:
            found += [crate / name.lstrip("/") for name in UPWARD.findall(text)]
            for tail in CLIMB.findall(text):
                climbed = tail.count(".parent()") + sum(map(int, ANCESTOR.findall(tail)))
                found.append(crate.joinpath(*[".."] * climbed))
    return [path for path in map(normal, found) if within(path, root) and path != root]


def reads(root):
    """The files and folders, under the repository, that zo-ide reads."""
    zo = normal(Path(root) / "zo-ide")
    read = {normal(Path(root) / name) for name in ALWAYS}
    workspace_of = {
        zo: tomllib.loads((Path(zo) / "Cargo.toml").read_text())["workspace"].get("dependencies", {}),
        root: tomllib.loads((Path(root) / "Cargo.toml").read_text()).get("workspace", {}).get("dependencies", {}),
    }
    waiting = [normal(path) for path in Path(zo).rglob("Cargo.toml") if "target" not in path.relative_to(zo).parts]
    crates = set()
    while waiting:
        manifest = waiting.pop()
        folder = normal(Path(manifest).parent)
        if folder in crates:
            continue
        crates.add(folder)
        home = zo if within(folder, zo) else root
        for name, spec in specs(tomllib.loads(Path(manifest).read_text())):
            if not isinstance(spec, dict):
                continue
            if "path" in spec:
                named = normal(Path(folder) / spec["path"])
            elif spec.get("workspace") and "path" in workspace_of[home].get(name, {}):
                named = normal(Path(home) / workspace_of[home][name]["path"])
            else:
                continue
            read.add(named)
            if (Path(named) / "Cargo.toml").is_file():
                waiting.append(normal(Path(named) / "Cargo.toml"))
    for crate in sorted(crates):
        for source in Path(crate).rglob("*.rs"):
            if "target" not in source.relative_to(crate).parts:
                read.update(named_by(normal(source), root))
    return read


def changed(root, base, head):
    """Every path a commit range touched; a rename counts at both ends."""
    out = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", "-z", base, head],
        cwd=root,
        check=True,
        capture_output=True,
    ).stdout
    return [normal(Path(root) / name) for name in out.decode().split("\0") if name]


def main(argv):
    if len(argv) not in (2, 3):
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    root = normal(Path.cwd())
    try:
        read = reads(root)
        for path in changed(root, argv[1], argv[2] if len(argv) == 3 else "HEAD"):
            through = next((folder for folder in sorted(read) if within(path, folder)), None)
            if through is not None:
                where = os.path.relpath(path, root)
                print(f"zo-ide-touched: {where} (through {os.path.relpath(through, root)})", file=sys.stderr)
                print("true")
                return 0
    except Exception as error:  # any doubt runs zo-ide
        print(f"zo-ide-touched: {type(error).__name__}: {error}, so zo-ide runs", file=sys.stderr)
        print("true")
        return 0
    print("false")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
