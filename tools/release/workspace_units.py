#!/usr/bin/env python3
"""Drop the build units of the crates a repository defines from a cargo target.

The release lane builds every run from a fresh clone, so each unit of a crate
the repository defines is rebuilt in that run anyway, and the ones an earlier
version left behind (the version stamp moves their hash) are never read again:
on 2026-10-13 they were 10.4 of zo-gate's 12.4 GiB and 11.4 of root-gate's
19 GiB. Units of registry and git crates stay, so the gates keep them warm.

    workspace_units.py drop --target DIR (--scratch DIR | --names "a b ...")

prints "<units> <bytes>" for what it removed.

    workspace_units.py foreign --target DIR

drops, from any crate, each build-script run that ran in another folder — its
`root-output`, the OUT_DIR cargo ran it with, is not its own `out` — with its
fingerprint, so cargo runs that build script again here; it prints the same
"<units> <bytes>". Cargo keys a registry crate's units by nothing that holds the
target's own path, and when a target is copied it rewrites only the OUT_DIR in
a run's recorded `output`, never the paths a script wrote into files under it:
on 2026-11-10 a worktree's target copied into the lane's root-gate kept tauri's
permission lists naming that worktree, which was then removed, and v1.1.140's
root gate stopped on them.
"""

import argparse
import os
import re
import shutil
from pathlib import Path

UNIT = re.compile(r"(.+)-([0-9a-f]{16})$")     # a unit's folder: <package>-<cargo's 16-hex hash>
OUTPUT = re.compile(r"-([0-9a-f]{16})(?:\.|$)")  # one of its outputs: <crate>-<hash>[.<ext>]
OUTPUT_DIRS = ("deps", "build", "examples")     # where a unit's outputs sit beside .fingerprint
NOT_SOURCE = {".git", "target", "node_modules"}  # never a crate the repository defines


def package_name(manifest):
    """The [package] name of a Cargo.toml, read line by line (no TOML parser on every Python here)."""
    section = None
    for line in manifest.read_text(encoding="utf-8", errors="replace").splitlines():
        s = line.strip()
        if s.startswith("["):
            section = s
        elif section == "[package]":
            m = re.match(r'name\s*=\s*"([^"]+)"', s)
            if m:
                return m.group(1)
    return None


def sourced_names(lock):
    """Names a Cargo.lock gives a registry or git source: never the repository's own."""
    names, name = set(), None
    for line in lock.read_text(encoding="utf-8", errors="replace").splitlines():
        s = line.strip()
        if s == "[[package]]":
            name = None
            continue
        m = re.match(r'name\s*=\s*"([^"]+)"', s)
        if m:
            name = m.group(1)
        elif s.startswith("source") and name:
            names.add(name)
    return names


def package_names(scratch):
    own, sourced = set(), set()
    for root, dirs, files in os.walk(scratch):
        dirs[:] = [d for d in dirs if d not in NOT_SOURCE]
        if "Cargo.toml" in files:
            name = package_name(Path(root, "Cargo.toml"))
            if name:
                own.add(name)
        if "Cargo.lock" in files:
            sourced |= sourced_names(Path(root, "Cargo.lock"))
    return own - sourced


def size(path):
    if path.is_symlink() or not path.is_dir():
        try:
            return path.lstat().st_size
        except OSError:
            return 0
    total = 0
    for root, _, files in os.walk(path):
        for n in files:
            try:
                total += os.lstat(os.path.join(root, n)).st_size
            except OSError:
                pass
    return total


def remove(path):
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path, ignore_errors=True)
    else:
        try:
            path.unlink()
        except FileNotFoundError:
            pass


def drop(target, names):
    """Remove every unit of a package in NAMES under TARGET (a host or cross profile); (units, bytes)."""
    units = freed = 0
    target = Path(target)
    for fingerprints in [*target.glob("*/.fingerprint"), *target.glob("*/*/.fingerprint")]:
        profile, hashes = fingerprints.parent, set()
        for unit in list(fingerprints.iterdir()):
            m = UNIT.match(unit.name)
            if m and m.group(1) in names:
                hashes.add(m.group(2))
                freed += size(unit)
                remove(unit)
                units += 1
        for folder in (profile / d for d in OUTPUT_DIRS):
            if not hashes or not folder.is_dir():
                continue
            for entry in list(folder.iterdir()):
                m = OUTPUT.search(entry.name)
                if m and m.group(1) in hashes:
                    freed += size(entry)
                    remove(entry)
    return units, freed


def foreign_runs(target):
    """Each build-script run folder under TARGET (a host or cross profile) whose `root-output` names an
    OUT_DIR other than its own `out`: the script ran in another target."""
    for build in [*Path(target).glob("*/build"), *Path(target).glob("*/*/build")]:
        for unit in sorted(build.iterdir()):
            ran_in = unit / "root-output"
            if not UNIT.match(unit.name) or not ran_in.is_file():
                continue
            said = ran_in.read_text(encoding="utf-8", errors="replace").strip()
            if said not in (str(unit / "out"), os.path.join(os.path.realpath(unit), "out")):
                yield unit


def drop_foreign(target):
    """Remove each run `foreign_runs` finds, with its fingerprint; (units, bytes)."""
    units = freed = 0
    for unit in list(foreign_runs(target)):
        for path in (unit, unit.parent.parent / ".fingerprint" / unit.name):
            freed += size(path)
            remove(path)
        units += 1
    return units, freed


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    actions = parser.add_subparsers(dest="action", required=True)
    drop_args = actions.add_parser("drop")
    drop_args.add_argument("--target", required=True)
    source = drop_args.add_mutually_exclusive_group(required=True)
    source.add_argument("--scratch", help="the checkout whose Cargo.toml files name the crates to drop")
    source.add_argument("--names", help="the package names themselves (the lane's dry twin)")
    foreign_args = actions.add_parser("foreign")
    foreign_args.add_argument("--target", required=True)
    args = parser.parse_args(argv)
    if args.action == "foreign":
        units, freed = drop_foreign(args.target)
        print(units, freed)
        return 0
    names = package_names(Path(args.scratch)) if args.scratch is not None else set(args.names.split())
    units, freed = drop(args.target, names)
    print(units, freed)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
