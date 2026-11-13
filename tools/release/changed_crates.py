#!/usr/bin/env python3
"""What `just pre-handin` should run for the paths a branch changed.

    changed_crates.py [--base REV] [--ran-test PACKAGE:TARGET]...
    changed_crates.py --workspaces

prints one fact a line, tab-separated, for the recipe to read:

    crate <workspace dir> <package> <cargo test arguments>
    doc|clippy|tests <workspace dir> <package> <shell command line, run in that dir>
    windows <workspace dir whose Windows build the change can break>
    ui <changed path under ui/>
    suite <window suite that looks at a changed ui file: named like it, or run by a test module that spells its name>
    suites <how many suites that is> <how many the window registers>

`--ran-test` names an integration test target the caller ran already: the
`tests` line leaves it out.

`--workspaces` prints instead the directories of the Cargo workspaces after the
root one, so that the recipe spells no workspace name.

The paths are the working tree against the merge base with main (else
origin/main), plus untracked files — what the branch would hand in. A crate is
a package of either Cargo workspace (the root one and zo-ide's) that owns a
changed path, in one of two ways, and no list of crates or folders is kept here:

  - the path is inside the package's directory (the longest manifest directory
    from `cargo metadata` that prefixes it);
  - the package reads the path from outside its directory: its sources name it
    in `include_str!`/`include_bytes!`/`include!` or in
    `concat!(env!("CARGO_MANIFEST_DIR"), "/…")`. Those are the files read at
    compile time, so a test that lists a folder at run time is not seen, nor
    is a build script's `cargo:rerun-if-changed=`: every one here formats its
    path from values known at run time, none is a quoted string.

Beside each crate's unit tests come three more checks, each the gate's own
command narrowed to that crate. The gate is the recipes its workspace's `verify`
depends on (read from `just --dump`, so no flag, crate or workspace name is kept
here, and of recipes only that one, `GATE_RECIPE`): of their `cargo doc`, `cargo clippy` and `cargo test` commands, each that
selects the crate (`-p crate`, or `--workspace` without `--exclude crate`) runs
with `--workspace --exclude …` replaced by `-p crate` and every other flag, the
recipe's `[env]` and the file's `unexport`s kept. A `cargo test` command runs
the crate's integration tests only (`--test name` for each target in `tests/`,
or the ones the command names): the unit tests are the `crate` line's. So a
change that only touches `tests/` owes clippy and those tests, and no unit test.

A workspace's Windows build (`just win-check` there, the cross-compile the
gate runs) is owed when the branch changed a Rust source or a manifest of one
of its packages (any target: `--all-targets` compiles the tests too), a build
script, or the workspace's own manifest, lock or `.cargo/` config. A file a
crate only reads at compile time cannot change what compiles for Windows.

Its unit tests are `--lib` when it has a library and `--bins` when it has
binaries, both when it has both. Which of them a path picks is the kind of
source that holds or reads it: `src/main.rs` and `src/bin/` are the bins' own,
`tests/`, `benches/` and `examples/` are targets of their own that this does
not run (the shell's contracts are stage one of the recipe), and any other
source — `src/lib.rs` and its modules, whose lib the bins link — picks all.
"""

import argparse
import json
import posixpath
import re
import shlex
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
OTHER_TARGET_DIRS = ("tests", "benches", "examples")   # targets of their own, not the unit tests of a lib or a bin
RUST_BUILD_FILES = ("Cargo.toml", "Cargo.lock", "build.rs")   # beside the `.rs` sources, what a cross-compile reads
GATE_RECIPE = "verify"           # the recipe of each workspace whose dependencies are its gate
GATE_KINDS = {"doc": "doc", "clippy": "clippy", "test": "tests"}   # the cargo subcommand the gate runs -> the fact it is printed as
ENV_WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
TEST_TARGET_FLAGS = {"--lib", "--bins", "--tests", "--doc", "--all-targets", "--examples", "--benches"}   # which targets `cargo test` builds


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


def unit_flags(package):
    """The `cargo test` flags of a package's unit tests: `--lib` for a library, `--bins` for binaries."""
    kinds = {kind for kinds, _ in package["targets"] for kind in kinds}
    return {flag for flag, kind in (("--lib", "lib"), ("--bins", "bin")) if kind in kinds}


def source_flags(package, path):
    """The unit-test flags a source file of `package` is part of, as the kind of source it is.

    `tests/`, `benches/` and `examples/` are targets of their own: none. The
    bins' own files (`src/main.rs`, `src/bin/`) are `--bins`; every other file
    is the library's, and the bins link the library, so all of them.
    """
    flags = unit_flags(package)
    first, _, rest = path[len(package["dir"]):].lstrip("/").partition("/")
    if first in OTHER_TARGET_DIRS:
        return set()
    if first == "src" and (rest == "main.rs" or rest.startswith("bin/")):
        return flags & {"--bins"} or flags
    return flags


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
    for pattern, base in ((FILE_READS, directory), (MANIFEST_READS, package_dir)):
        for name in pattern.findall(code):
            yield posixpath.normpath(posixpath.join(base, name.lstrip("/")))


def outside_reads(found, own_dir):
    """What a package reads from outside its own directory (and inside the repository), as sorted paths."""
    return sorted({path for path in found if path not in (".", "") and not path.startswith("..") and not inside(path, own_dir)})


def attach_reads(packages_by_workspace):
    """Give each package its "reads": {path outside its directory that its Rust sources name: the unit-test flags of the sources that name it}."""
    sources = [path for path in git("ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "*.rs")]
    found = {}
    for source in sources:
        hit = own(source, packages_by_workspace)
        if hit is None or not (REPO / source).is_file():
            continue
        text = (REPO / source).read_text(encoding="utf-8", errors="replace")
        by_flag = found.setdefault((hit[0], hit[1]["name"]), {})
        for flag in source_flags(hit[1], source):
            by_flag.setdefault(flag, set()).update(reads_of(text, posixpath.dirname(source), hit[1]["dir"]))

    for workspace, packages in packages_by_workspace.items():
        for package in packages:
            package["reads"] = {}
            for flag, paths in found.get((workspace, package["name"]), {}).items():
                for path in outside_reads(paths, package["dir"]):
                    package["reads"].setdefault(path, set()).add(flag)


def owners(path, packages_by_workspace):
    """(workspace dir, package, unit-test flags) a change to `path` can break: its directory's package, and each package that reads it."""
    found = []
    hit = own(path, packages_by_workspace)
    if hit:
        found.append((hit[0], hit[1], source_flags(hit[1], path)))
    for workspace, packages in packages_by_workspace.items():
        for package in packages:
            if hit and (workspace, package) == hit:
                continue
            flags = set().union(*(flags for read, flags in package.get("reads", {}).items() if inside(path, read)))
            if flags:
                found.append((workspace, package, flags))
    return found


def crates(paths, packages_by_workspace):
    """The (workspace dir, package name, cargo test arguments) of every crate the paths touch, once."""
    chosen = {}
    for path in paths:
        for workspace, package, flags in owners(path, packages_by_workspace):
            chosen.setdefault((workspace, package["name"]), set()).update(flags)
    return [(workspace, name, " ".join(flag for flag in ("--lib", "--bins") if flag in flags))
            for (workspace, name), flags in sorted(chosen.items()) if flags]


def windows_builds(paths, packages_by_workspace):
    """The workspaces, in their order, whose Windows build one of the paths can break."""
    found = set()
    for path in paths:
        rust = path.endswith(".rs") or posixpath.basename(path) in RUST_BUILD_FILES
        hit = own(path, packages_by_workspace)
        if rust and hit:
            found.add(hit[0])
        for workspace in packages_by_workspace:
            top = "" if workspace == "." else workspace + "/"
            if path in (top + "Cargo.toml", top + "Cargo.lock") or path.startswith(top + ".cargo/"):
                found.add(workspace)
    return [workspace for workspace in packages_by_workspace if workspace in found]


def touched(paths, packages_by_workspace):
    """The (workspace dir, package name) of every crate a path belongs to or is read by, once.

    A crate that only a `tests/` file changed is here though it owes no unit test:
    its lint and its integration tests are still owed.
    """
    found = set()
    for path in paths:
        for workspace, package, _ in owners(path, packages_by_workspace):
            found.add((workspace, package["name"]))
    return sorted(found)


def just_dump(workspace):
    """The workspace's justfile as `just` parses it (None when it has none)."""
    justfile = REPO / workspace / "justfile"
    if not justfile.is_file():
        return None
    done = subprocess.run(
        ["just", "--justfile", str(justfile), "--working-directory", str(justfile.parent),
         "--dump", "--dump-format", "json"],
        capture_output=True, text=True,
    )
    if done.returncode != 0:
        raise SystemExit(f"changed_crates: just --dump in {workspace}: {done.stderr.strip()}")
    return json.loads(done.stdout)


def body_line(fragments, assignments):
    """One recipe line as text, its `{{variable}}`s filled; None when it holds an expression this does not evaluate."""
    text = ""
    for piece in fragments:
        if isinstance(piece, str):
            text += piece
            continue
        for expression in piece:
            value = assignments.get(expression[1], {}).get("value") if expression[0] == "variable" else None
            if not isinstance(value, str):
                return None
            text += value
    return text


def cargo_commands(name, recipe, assignments):
    """The `cargo doc|clippy|test` commands of one recipe's body, as (sub command, env words, arguments, arguments after `--`).

    A command is a line (or one side of `&&`) that is, after its `NAME=value`
    words, `cargo <sub> …`. A script recipe (`#!`) and a line `shlex` cannot read
    are not read; a line that names `cargo` and uses an expression this does not
    evaluate stops the run, rather than going missing from the gate.
    """
    lines = recipe["body"]
    if lines and lines[0] and isinstance(lines[0][0], str) and lines[0][0].startswith("#!"):
        return
    for fragments in lines:
        line = body_line(fragments, assignments)
        if line is None:
            if any(isinstance(piece, str) and "cargo" in piece for piece in fragments):
                raise SystemExit(f"changed_crates: recipe {name} has a cargo line this cannot evaluate: {fragments}")
            continue
        line = line.strip().lstrip("@-")
        if not line or line.startswith("#"):
            continue
        try:
            words = shlex.split(line)
        except ValueError:
            continue
        segment = []
        for word in [*words, "&&"]:
            if word != "&&":
                segment.append(word)
                continue
            env = []
            while segment and ENV_WORD.match(segment[0]):
                env.append(segment.pop(0))
            if len(segment) > 1 and segment[0] == "cargo" and segment[1] in GATE_KINDS:
                rest = segment[2:]
                at = rest.index("--") if "--" in rest else len(rest)
                yield segment[1], env, rest[:at], rest[at + 1:]
            segment = []


def gate_commands(workspace):
    """What the workspace's gate runs of cargo doc, clippy and test, in the gate's order.

    The gate is the recipes `GATE_RECIPE` depends on, however deep. Each is
    (sub command, the `env …` words that open it, arguments, arguments after
    `--`): `env` carries the recipe's own `[env]` attributes and the recipe line's
    `NAME=value` words, and `-u` for each name the justfile `unexport`s, so the
    command runs as the gate runs it (a quoted `NAME=value` is no assignment to a
    shell, `env` is).
    """
    dump = just_dump(workspace)
    if dump is None or GATE_RECIPE not in dump["recipes"]:
        return []
    order, pending = [], [GATE_RECIPE]
    while pending:
        name = pending.pop(0)
        if name in order or name not in dump["recipes"]:
            continue
        order.append(name)
        pending.extend(dependency["recipe"] for dependency in dump["recipes"][name]["dependencies"])
    unset = [word for variable in dump.get("unexports", []) for word in ("-u", variable)]
    found = []
    for name in order:
        recipe = dump["recipes"][name]
        if recipe["parameters"]:
            continue
        attributes = [f"{pair[0]}={pair[1]}" for attribute in recipe["attributes"]
                      if isinstance(attribute, dict) for pair in [attribute.get("env")] if pair]
        for sub, env, arguments, trailing in cargo_commands(name, recipe, dump["assignments"]):
            words = [*unset, *attributes, *env]
            found.append((sub, ["env", *unset, *attributes, *env] if words else [], arguments, trailing))
    return found


def narrow(arguments, package):
    """A gate command's arguments for one package: the rest of them, or None when the command does not select it.

    `--workspace` (less each `--exclude`) and `-p` are the selection; they are
    replaced by the caller's own `-p`, every other word stays.
    """
    everything, excluded, chosen, rest = False, set(), set(), []
    words = iter(arguments)
    for word in words:
        if word in ("--workspace", "--all"):
            everything = True
        elif word == "--exclude":
            excluded.add(next(words, ""))
        elif word.startswith("--exclude="):
            excluded.add(word.partition("=")[2])
        elif word in ("-p", "--package"):
            chosen.add(next(words, ""))
        elif word.startswith("--package="):
            chosen.add(word.partition("=")[2])
        else:
            rest.append(word)
    return rest if package in chosen or (everything and package not in excluded) else None


def integration_tests(rest, package, ran=()):
    """A `cargo test` command's arguments cut to the crate's integration tests, or None when it has none to run.

    `--test NAME` stays as named; `--tests`, `--all-targets` or no target flag at
    all means every target in the crate's `tests/`. `--lib`, `--bins`, `--bin`,
    `--doc` and the like are the unit tests', which the `crate` line runs. A
    target in `ran` was run before the recipe asked: it is left out.
    """
    named, everything, named_any, kept = [], False, False, []
    words = iter(rest)
    for word in words:
        if word == "--test" or word.startswith("--test="):
            named.append(next(words, "") if word == "--test" else word.partition("=")[2])
            named_any = True
        elif word == "--bin":
            next(words, "")
            named_any = True
        elif word in TEST_TARGET_FLAGS:
            named_any = True
            everything = everything or word in ("--tests", "--all-targets")
        else:
            kept.append(word)
    available = [name for kinds, name in package["targets"] if "test" in kinds]
    wanted = [name for name in (available if everything or not named_any else named) if name not in ran]
    if not wanted:
        return None
    return [flag for name in wanted for flag in ("--test", name)] + kept


def gate_checks(package, commands, ran=()):
    """[(fact, shell command line)] the gate owes one crate: its rustdoc, its lint, its integration tests.

    Each fact is the gate's commands for that cargo subcommand, narrowed to the
    crate and joined by `&&` as the gate's own recipes join them. `ran` is the
    names of the package's test targets that were run already.
    """
    lines = {}
    for sub, env, arguments, trailing in commands:
        rest = narrow(arguments, package["name"])
        if rest is None:
            continue
        if sub == "test":
            rest = integration_tests(rest, package, ran)
            if rest is None:
                continue
        words = [*env, "cargo", sub, "-p", package["name"], *rest, *(["--", *trailing] if trailing else [])]
        line = shlex.join(words)
        if line not in lines.setdefault(GATE_KINDS[sub], []):
            lines[GATE_KINDS[sub]].append(line)
    return [(fact, " && ".join(lines[fact])) for fact in GATE_KINDS.values() if fact in lines]


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


def cargo_workspaces():
    """The Cargo workspaces that exist here, the root one first."""
    return [workspace for workspace in WORKSPACES if (REPO / workspace / "Cargo.toml").is_file()]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="the revision to measure from (default: merge base with main, else origin/main)")
    parser.add_argument("--ran-test", action="append", default=[], metavar="PACKAGE:TARGET",
                        help="an integration test target already run, left out of the `tests` line")
    parser.add_argument("--workspaces", action="store_true", help="print the Cargo workspaces after the root one, and stop")
    args = parser.parse_args(argv)
    if args.workspaces:
        for workspace in cargo_workspaces():
            if workspace != ".":
                print(workspace)
        return 0
    paths = changed_paths(merge_base(args.base))
    by_workspace = {workspace: packages(workspace) for workspace in cargo_workspaces()}
    attach_reads(by_workspace)
    units = {(workspace, name): arguments for workspace, name, arguments in crates(paths, by_workspace)}
    gates = {}
    for workspace, name in touched(paths, by_workspace):
        if (workspace, name) in units:
            print("crate", workspace, name, units[(workspace, name)], sep="\t")
        if workspace not in gates:
            gates[workspace] = gate_commands(workspace)
        package = next(found for found in by_workspace[workspace] if found["name"] == name)
        ran = [target for owner, _, target in (word.partition(":") for word in args.ran_test) if owner == name]
        for fact, line in gate_checks(package, gates[workspace], ran):
            print(fact, workspace, name, line, sep="\t")
    for workspace in windows_builds(paths, by_workspace):
        print("windows", workspace, sep="\t")
    ui = ui_paths(paths)
    for path in ui:
        print("ui", path, sep="\t")
    window = REPO / WINDOW
    if ui and window.is_file():
        tests = {path.stem: path.read_text(encoding="utf-8", errors="replace")
                 for path in window.parent.glob("*.mjs") if path != window}
        window_text = window.read_text(encoding="utf-8")
        hinted = suites_for(paths, window_text, tests)
        for name in hinted:
            print("suite", name, sep="\t")
        print("suites", len(hinted), len(SUITE.findall(window_text)), sep="\t")


if __name__ == "__main__":
    sys.exit(main())
