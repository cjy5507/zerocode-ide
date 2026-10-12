#!/usr/bin/env python3
"""Launch a pinned receipt producer from a coordinator-reviewed external manifest.

The native bootstrap verifies launcher and Python runtime identities before Python starts.
The coordinator pins the external manifest and bundle. This declared-input cache does
not isolate arbitrary code running as the same operating-system user.
"""

import argparse
import configparser
import encodings
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import socket
import sys
import types

MAX_POLICY = 1024 * 1024
MANDATORY = ["native", "windows", "flake-judgment", "publish"]
BUILD_ENV = {
    "PATH", "LANG", "LC_ALL", "TZ", "SystemRoot", "HOME", "USERPROFILE",
    "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA",
    "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL", "CARGO_NET_OFFLINE", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
    "RUSTFLAGS", "RUSTDOCFLAGS", "RUSTC", "RUSTDOC", "CC", "CXX", "AR", "SDKROOT", "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET", "TMPDIR", "TMP", "TEMP", "NODE_PATH",
    "npm_config_cache", "npm_config_offline",
    "NODE_OPTIONS", "GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM",
}
PATH_ENV = {
    "HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "APPDATA",
    "LOCALAPPDATA", "CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR", "SDKROOT", "DEVELOPER_DIR",
    "TMPDIR", "TMP", "TEMP", "npm_config_cache",
}
CONFIGURATION_ENV = {
    "HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA",
    "CARGO_HOME", "RUSTUP_HOME",
}
OUTPUT_ENV = {"XDG_CACHE_HOME", "CARGO_TARGET_DIR", "TMPDIR", "TMP", "TEMP", "npm_config_cache"}


def node_searches(repo, scripts, dependencies):
    bases = {Path(repo).resolve(), *(Path(value).resolve().parent for value in scripts)}
    bases.update(Path(value).resolve() if Path(value).is_dir() else Path(value).resolve().parent for value in dependencies)
    return [{"directory": str(base), "ancestors": True, "names": ["node_modules"]} for base in sorted(bases)]


def git_metadata_paths(repo, producer):
    marker = Path(repo) / ".git"
    paths = []
    if marker.is_symlink():
        raise producer.UnknownInput("linked Git metadata is unsupported")
    if marker.is_dir():
        directory = marker.resolve()
    else:
        raw = read_configuration(marker, producer).decode().strip()
        if not raw.startswith("gitdir: ") or "\n" in raw or "\0" in raw:
            raise producer.UnknownInput("Git metadata locator is unsupported")
        named = Path(raw[8:])
        directory = (named if named.is_absolute() else Path(repo) / named).resolve(strict=True)
        paths.append(marker)
    common_marker = directory / "commondir"
    paths.append(common_marker)
    common = directory
    if common_marker.exists() or common_marker.is_symlink():
        raw = read_configuration(common_marker, producer).decode().strip()
        if not raw or "\n" in raw or "\0" in raw:
            raise producer.UnknownInput("Git common metadata locator is unsupported")
        common = (directory / raw).resolve(strict=True)
    paths += [directory / "config", common / "config", directory / "config.worktree",
              common / "info/attributes", common / "info/exclude", common / "hooks"]
    return sorted(set(paths))


def read_configuration(path, producer):
    try:
        return read_regular(path)
    except (Untrusted, OSError) as why:
        raise producer.UnknownInput("configuration cannot be read without following links") from why


def safe_git_configuration(paths, producer):
    allowed = {
        "core": {"repositoryformatversion", "filemode", "bare", "logallrefupdates", "ignorecase",
                 "precomposeunicode", "abbrev", "quotepath", "autocrlf", "eol", "symlinks", "fsmonitor"},
        "user": {"name", "email"}, "remote": {"url", "fetch", "pushurl"},
        "branch": {"remote", "merge"}, "extensions": {"objectformat", "worktreeconfig"},
        "init": {"defaultbranch"},
    }
    for path in paths:
        if path.name == "hooks" and path.exists():
            if any(not child.name.endswith(".sample") for child in path.iterdir()):
                raise producer.UnknownInput("active Git hooks are unsupported by this closure")
        if path.name not in ("config", "config.worktree") or not path.exists():
            continue
        parser = configparser.RawConfigParser(strict=True, allow_no_value=True, interpolation=None)
        try:
            parser.read_string(read_configuration(path, producer).decode())
        except (configparser.Error, UnicodeError) as why:
            raise producer.UnknownInput("Git configuration cannot be inspected safely") from why
        if parser.defaults():
            raise producer.UnknownInput("Git default sections are unsupported")
        for section in parser.sections():
            if any(value is not None and ("\n" in value or "\r" in value) for value in parser[section].values()):
                raise producer.UnknownInput("ambiguous multiline Git configuration is unsupported")
            prefix = section.split(maxsplit=1)[0].lower()
            if prefix not in allowed or set(parser[section]) - allowed[prefix]:
                raise producer.UnknownInput("Git configuration has an undeclared file or program dependency")
            if prefix == "core" and "fsmonitor" in parser[section] and str(parser[section]["fsmonitor"] or "true").lower() not in ("false", "0", "no"):
                raise producer.UnknownInput("Git fsmonitor configuration is unsupported")


class Untrusted(Exception):
    """Independent provenance or declared input closure is unavailable."""

    def __init__(self, message, code="identity_unavailable"):
        super().__init__(message)
        self.code = code


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def read_regular(path):
    path = Path(path).absolute()
    if any(part.is_symlink() for part in (path, *path.parents)):
        raise Untrusted("trusted code or configuration uses a link")
    before = path.stat()
    if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_POLICY * 8:
        raise Untrusted("trusted file is not a bounded regular file")
    data = path.read_bytes()
    after = path.stat()
    fields = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns, item.st_ctime_ns, item.st_mode)
    if fields(before) != fields(after):
        raise Untrusted("trusted bytes changed while reading")
    return data


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Untrusted("duplicate manifest field")
        result[key] = value
    return result


def load_policy(path, expected, repo, launcher_path=None):
    repo = Path(repo).resolve()
    path = Path(path).absolute()
    if path.resolve().is_relative_to(repo) or not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise Untrusted("manifest needs an independently supplied external identity")
    raw = read_regular(path)
    if len(raw) > MAX_POLICY or sha256(raw) != expected:
        raise Untrusted("manifest identity changed")
    policy = json.loads(raw, object_pairs_hook=unique_object)
    if not isinstance(policy, dict) or set(policy) != {"version", "bundle", "runtime", "gates"} or policy["version"] != 3:
        raise Untrusted("unsupported trust manifest")
    runtime = policy["runtime"]
    if not isinstance(runtime, dict) or set(runtime) != {"python", "stdlib", "libraries"}:
        raise Untrusted("unsupported runtime declaration")
    for entry in (runtime["python"], runtime["stdlib"]):
        if (not isinstance(entry, dict) or set(entry) != {"path", "sha256"}
                or not isinstance(entry["path"], str) or not Path(entry["path"]).is_absolute()
                or Path(entry["path"]).resolve().is_relative_to(repo)
                or not isinstance(entry["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", entry["sha256"])):
            raise Untrusted("runtime needs independently pinned external identities")
    library_bindings(runtime["libraries"], repo)
    bundle = policy["bundle"]
    if not isinstance(bundle, dict) or set(bundle) != {"launcher", "producer", "scanner", "rules"} or not isinstance(policy["gates"], dict):
        raise Untrusted("incomplete trusted bundle")
    retained = {}
    for role, entry in bundle.items():
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256"}:
            raise Untrusted("unsupported trusted file identity")
        named = Path(entry["path"])
        if not named.is_absolute() or named.resolve().is_relative_to(repo):
            raise Untrusted("candidate-contained producer or scanner")
        data = read_regular(named)
        if sha256(data) != entry["sha256"]:
            raise Untrusted("trusted bundle identity changed")
        retained[role] = data
    if Path(bundle["launcher"]["path"]).resolve() != Path(launcher_path or __file__).resolve():
        raise Untrusted("launcher invocation does not match the independent pin")
    return {"policy": policy, "policySha256": expected, "policyPath": str(path), "policyBytes": raw, "bytes": retained}


def library_bindings(libraries, repo):
    if not isinstance(libraries, list) or len(libraries) > 16:
        raise Untrusted("runtime libraries are not bounded", "digest_mismatch")
    names, targets, bindings, total = set(), {}, [], 0
    for entry in libraries:
        if (not isinstance(entry, dict) or set(entry) != {"path", "resolvedPath", "sha256"}
                or any(not isinstance(entry[key], str) for key in entry)):
            raise Untrusted("unsupported runtime library identity", "digest_mismatch")
        named, target = Path(entry["path"]), Path(entry["resolvedPath"])
        if (not named.is_absolute() or not target.is_absolute() or ".." in named.parts or target != target.resolve(strict=True)
                or named.resolve(strict=True) != target or target.is_relative_to(repo)
                or entry["path"] in names
                or entry["resolvedPath"] in targets and targets[entry["resolvedPath"]] != entry["sha256"]
                or not target.is_file()):
            raise Untrusted("runtime library binding changed or duplicated", "digest_mismatch")
        before = target.lstat()
        if not stat.S_ISREG(before.st_mode) or before.st_size > 128 * 1024 * 1024:
            raise Untrusted("runtime library file is not bounded and regular", "digest_mismatch")
        hasher = hashlib.sha256()
        with target.open("rb") as file:
            for block in iter(lambda: file.read(1024 * 1024), b""):
                hasher.update(block)
        after = target.lstat()
        stamp = lambda value: (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns, value.st_mode)
        if entry["resolvedPath"] not in targets:
            total += before.st_size
        if stamp(before) != stamp(after) or total > 256 * 1024 * 1024 or hasher.hexdigest() != entry["sha256"]:
            raise Untrusted("runtime library bytes or allocation changed", "digest_mismatch")
        names.add(entry["path"])
        targets[entry["resolvedPath"]] = entry["sha256"]
        bindings.append([entry["path"], entry["resolvedPath"], entry["sha256"]])
    return bindings


def runtime_locations(policy):
    runtime = policy["runtime"]
    root = Path(runtime["stdlib"]["path"]).resolve(strict=True)
    python = Path(runtime["python"]["path"]).resolve(strict=True)
    paths = read_regular(Path(str(python) + "._pth")).decode().splitlines()
    arguments = getattr(sys, "orig_argv", None)
    images = [sys.executable, getattr(sys, "_base_executable", None),
              arguments[0] if isinstance(arguments, list) and arguments else None]
    if (len(paths) != 2 or sys.path != paths
            or any(not isinstance(value, str) or not Path(value).is_absolute()
                   or Path(value).resolve(strict=True) != python for value in images)):
        raise Untrusted("runtime startup paths or process image differ from the native binding", "digest_mismatch")
    stdlib, dynload = (Path(path) for path in paths)
    if (any(not path.is_absolute() or path.resolve(strict=True) != path for path in (stdlib, dynload))
            or stdlib.parent != root / "lib" or dynload != stdlib / "lib-dynload"):
        raise Untrusted("runtime startup paths leave the pinned tree", "digest_mismatch")
    locations = [sys.prefix, sys.base_prefix, sys.exec_prefix, sys.base_exec_prefix, getattr(sys, "_stdlib_dir", None)]
    if any(not isinstance(value, str) or not Path(value).is_absolute()
           or not Path(value).resolve().is_relative_to(root) for value in locations):
        raise Untrusted("runtime prefixes leave the pinned tree", "digest_mismatch")
    if any(not Path(module.__file__).resolve(strict=True).is_relative_to(stdlib) for module in (json, encodings)):
        raise Untrusted("runtime startup modules leave the pinned standard library", "digest_mismatch")


def search_paths(searches, producer):
    if not isinstance(searches, list) or not searches:
        raise producer.UnknownInput("configuration search scopes are undeclared")
    paths = set()
    for search in searches:
        if not isinstance(search, dict) or set(search) != {"directory", "ancestors", "names"}:
            raise producer.UnknownInput("unsupported configuration search scope")
        root = Path(search["directory"])
        if not root.is_absolute() or not isinstance(search["ancestors"], bool) or not isinstance(search["names"], list) or not search["names"]:
            raise producer.UnknownInput("configuration search scope is incomplete")
        names = [Path(value) for value in search["names"]]
        if any(value.is_absolute() or ".." in value.parts or str(value) == "." for value in names):
            raise producer.UnknownInput("configuration search name escapes its scope")
        for directory in [root, *root.parents] if search["ancestors"] else [root]:
            paths.update(directory / name for name in names)
    return sorted(paths)


def path_bindings(environment, roots, producer):
    bindings = []
    for name in environment["PATH"].split(os.pathsep):
        directory = Path(name)
        if not directory.is_absolute() or not directory.is_dir():
            raise producer.UnknownInput("tool search directory is missing or relative")
        entries = []
        for path in sorted(directory.iterdir()):
            try:
                target = path.resolve(strict=True)
            except (OSError, RuntimeError) as why:
                raise producer.UnknownInput("PATH contains a missing or cyclic tool link") from why
            if not target.is_file() or not any(target == root or root.is_dir() and target.is_relative_to(root) for root in roots):
                raise producer.UnknownInput("PATH exposes a tool without a declared identity")
            entries.append([path.name, stat.S_IMODE(path.lstat().st_mode),
                            os.readlink(path) if path.is_symlink() else None, str(target)])
        bindings.append([str(directory), str(directory.resolve()), entries])
    return bindings


def cache_bindings(outputs, producer):
    bindings = []
    for name in outputs:
        path = Path(name)
        if path != path.resolve() or any(parent.is_symlink() for parent in path.parents):
            raise producer.UnknownInput("cache output path is linked or not canonical")
        if path.is_dir():
            for child in path.rglob("*"):
                if child.is_symlink():
                    try:
                        target = child.resolve(strict=True)
                    except (OSError, RuntimeError) as why:
                        raise producer.UnknownInput("cache output contains a missing or cyclic link") from why
                    if not target.is_relative_to(path):
                        raise producer.UnknownInput("cache output link reaches an input outside its output root")
        bindings.append(str(path))
    return bindings


def configure(context, args, producer):
    producer.GIT_GUARD = lambda repo: safe_git_configuration(git_metadata_paths(repo, producer), producer)
    selected = context["policy"]["gates"].get(args.gate)
    required = {"command", "recipes", "testRecipes", "profile", "environment", "dependencyRoots", "toolchainRoots",
                "configurationRoots", "configurationSearches", "lockFiles", "scriptFiles", "outputRoots", "mandatoryChecks",
                "target", "timeout", "maxJobTtlSeconds", "overheadTimeoutSeconds", "verificationTimeoutSeconds"}
    if not isinstance(selected, dict) or set(selected) != required:
        raise producer.UnknownInput("gate has no complete independently trusted declaration")
    if selected["command"] != args.command or selected["profile"] != args.profile:
        raise producer.UnknownInput("gate command or profile differs from its trusted recipe")
    if isinstance(selected["timeout"], bool) or not isinstance(selected["timeout"], (int, float)) or not 0 < selected["timeout"] <= 7200 or selected["timeout"] != args.timeout:
        raise producer.UnknownInput("gate timeout differs from its trusted declaration")
    if any(type(selected[key]) is not int or not 1 <= selected[key] <= cap for key, cap in
           (("maxJobTtlSeconds", 7200), ("overheadTimeoutSeconds", 3600), ("verificationTimeoutSeconds", 3600))):
        raise producer.UnknownInput("gate job lifetime is not bounded by its trusted declaration")
    if (selected["timeout"] + selected["overheadTimeoutSeconds"] > selected["maxJobTtlSeconds"]
            or selected["verificationTimeoutSeconds"] > selected["maxJobTtlSeconds"]):
        raise producer.UnknownInput("gate phase budgets exceed the pinned maximum lifetime")
    if selected["mandatoryChecks"] != MANDATORY or not isinstance(selected["recipes"], list) or not selected["recipes"]:
        raise producer.UnknownInput("gate has no complete recipe list and mandatory non-reuse checks")
    if any(not isinstance(name, str) for name in selected["recipes"]) or len(selected["recipes"]) != len(set(selected["recipes"])) or any(
        not re.fullmatch(r"[A-Za-z0-9_-]+", name) for name in selected["recipes"]
    ):
        raise producer.UnknownInput("gate recipe list is invalid")
    if (not isinstance(selected["testRecipes"], list) or not selected["testRecipes"]
            or any(not isinstance(name, str) for name in selected["testRecipes"])
            or len(set(selected["testRecipes"])) != len(selected["testRecipes"])
            or not set(selected["testRecipes"]).issubset(selected["recipes"])):
        raise producer.UnknownInput("test recipes need a nonempty unique recipe subset")
    if args.scope != "deterministic" or args.closure != "declared":
        raise producer.UnknownInput("trusted cache needs the deterministic declared scope")
    environment = selected["environment"]
    if not isinstance(environment, dict) or set(environment) - BUILD_ENV or any(
        not isinstance(value, str) or re.search(r"[\r\n\0]", value) for value in environment.values()
    ):
        raise producer.UnknownInput("child environment includes undeclared values or authority")
    if any(environment.get(key) != value for key, value in (("LANG", "C"), ("LC_ALL", "C"), ("TZ", "UTC"))):
        raise producer.UnknownInput("child locale is not fixed")
    if (environment.get("NODE_OPTIONS") != "--no-global-search-paths" or "NODE_PATH" in environment
            or environment.get("GIT_CONFIG_NOSYSTEM") != "1"
            or environment.get("GIT_CONFIG_GLOBAL") != os.devnull or environment.get("GIT_CONFIG_SYSTEM") != os.devnull):
        raise producer.UnknownInput("Node and Git fallbacks are not explicitly disabled")
    repo = Path(args.repo).resolve()
    try:
        runtime_bindings = library_bindings(context["policy"]["runtime"]["libraries"], repo)
    except (Untrusted, OSError, ValueError) as why:
        raise producer.UnknownInput("runtime library input changed") from why
    account = Path(os.environ.get("HOME", os.path.expanduser("~"))).resolve()
    home = Path(environment.get("HOME", ""))
    if not home.is_absolute() or home.resolve().is_relative_to(repo) or home.resolve() == account:
        raise producer.UnknownInput("child needs an explicit separate home")
    required_homes = {"USERPROFILE", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME",
                      "APPDATA", "LOCALAPPDATA", "CARGO_HOME", "RUSTUP_HOME"}
    if not required_homes.issubset(environment) or Path(environment["USERPROFILE"]).resolve() != home.resolve():
        raise producer.UnknownInput("child account and tool configuration fallbacks are undeclared")
    if any(not Path(part).is_absolute() for part in environment.get("PATH", "").split(os.pathsep)):
        raise producer.UnknownInput("child PATH has a relative search entry")
    roles = ("dependencyRoots", "toolchainRoots", "configurationRoots", "lockFiles", "scriptFiles")
    roots = {role: [] for role in roles}
    absent = []
    for role in roles:
        rows = selected[role]
        if not isinstance(rows, list) or not rows:
            raise producer.UnknownInput("gate input role is undeclared")
        for row in rows:
            if not isinstance(row, dict) or not Path(row.get("path", "")).is_absolute():
                raise producer.UnknownInput("gate input root has no identity")
            if set(row) == {"path", "state"} and row["state"] == "absent" and role == "configurationRoots":
                absent.append(Path(row["path"]).resolve())
            elif set(row) == {"path", "sha256"}:
                roots[role].append(row["path"])
            else:
                raise producer.UnknownInput("unsupported input identity or absence declaration")
    outputs = selected["outputRoots"]
    if not isinstance(outputs, list) or any(not Path(value).is_absolute() for value in outputs):
        raise producer.UnknownInput("output roots must be explicit absolute paths")
    input_paths = [Path(value).resolve() for values in roots.values() for value in values]
    output_paths = [Path(value).resolve() for value in outputs]
    config_paths = [Path(value).resolve() for value in roots["configurationRoots"]]
    protected = [Path(value).resolve() for role in roles if role != "configurationRoots" for value in roots[role]]
    searched = search_paths(selected["configurationSearches"], producer)
    cargo_searches = [directory / ".cargo" / name for directory in (repo, *repo.parents) for name in ("config", "config.toml")]
    cargo_searches += [Path(environment["CARGO_HOME"]) / name for name in ("config", "config.toml")]
    cargo_searches += [home / ".cargo" / name for name in ("config", "config.toml")]
    cargo_searches += [Path(environment["RUSTUP_HOME"]) / "settings.toml", home / ".rustup/settings.toml"]
    if not set(cargo_searches).issubset(searched):
        raise producer.UnknownInput("Cargo configuration search fallbacks are undeclared")
    node_paths = search_paths(node_searches(repo, roots["scriptFiles"], roots["dependencyRoots"]), producer)
    git_paths = git_metadata_paths(repo, producer)
    if not set(node_paths).issubset(searched) or not set(git_paths).issubset(searched):
        raise producer.UnknownInput("Node or Git configuration search scopes are undeclared")
    config_environment = [Path(environment[key]).resolve() for key in CONFIGURATION_ENV.intersection(environment)]
    if any(path.is_relative_to(repo) for path in (Path(value).resolve() for value in roots["toolchainRoots"])):
        raise producer.UnknownInput("candidate-owned toolchain is untrusted")
    if any(path == repo or repo.is_relative_to(path) for path in output_paths):
        raise producer.UnknownInput("output root can hide candidate inputs")
    if any(a.is_relative_to(b) or b.is_relative_to(a) for a in protected for b in output_paths):
        raise producer.UnknownInput("cache output hides a dependency, script, lock or tool input")
    if any(a.is_relative_to(b) for a in [*config_paths, *absent, *searched, *config_environment] for b in output_paths):
        raise producer.UnknownInput("cache output hides configuration or declared absence")
    config_directories = [path for path in config_paths if path.is_dir()]
    for candidate in searched:
        target = candidate.resolve()
        if target not in absent and not any(target == root or root.is_dir() and target.is_relative_to(root) for root in config_paths):
            raise producer.UnknownInput("configuration search has an undeclared input or absence")
    for key in PATH_ENV.intersection(environment):
        value = Path(environment[key])
        allowed = input_paths + output_paths if key in OUTPUT_ENV else config_directories if key in CONFIGURATION_ENV else input_paths
        if not value.is_absolute() or not any(value.resolve().is_relative_to(root) for root in allowed):
            raise producer.UnknownInput("configuration or cache path has no declared role")
    configuration_identities = [["runtimeLibraries", runtime_bindings]]
    for role in ("configurationRoots", "dependencyRoots", "lockFiles", "scriptFiles", "toolchainRoots"):
        for row in selected[role]:
            path = Path(row["path"])
            if row.get("state") == "absent":
                if path.exists() or path.is_symlink():
                    raise producer.UnknownInput("declared configuration absence changed")
                actual = None
            else:
                actual = producer.paths_digest([path], excluded=outputs if role == "configurationRoots" else ())
                if actual != row["sha256"]:
                    raise producer.UnknownInput("declared input root changed")
            if role == "configurationRoots":
                configuration_identities.append([str(path), str(path.resolve()), row.get("state", "present"), actual])
    output_bindings = cache_bindings(outputs, producer)
    safe_git_configuration(git_paths, producer)
    configuration_identities += [["search", str(path), str(path.resolve()), os.readlink(path) if path.is_symlink() else None,
                                  "present" if path.exists() or path.is_symlink() else "absent"] for path in sorted(searched)]
    for part in environment.get("NODE_PATH", "").split(os.pathsep) if "NODE_PATH" in environment else []:
        value = Path(part)
        if not value.is_absolute() or not any(value.resolve().is_relative_to(root) for root in input_paths):
            raise producer.UnknownInput("node module search path has no declared input identity")
    executable = producer.command_executable(repo, args.command[0], environment)
    tool_roots = [Path(value).resolve() for value in roots["toolchainRoots"]]
    required_tools = [Path(sys.executable).resolve(), executable,
                      producer.command_executable(repo, "git", environment)]
    required_tools += [producer.command_executable(repo, environment[key], environment)
                       for key in ("CC", "CXX", "AR", "RUSTC", "RUSTDOC") if key in environment]
    if any(tool is None or not any(tool.is_relative_to(root) for root in tool_roots) for tool in required_tools):
        raise producer.UnknownInput("an invoked tool has no independently declared identity")
    tools = path_bindings(environment, tool_roots, producer)
    producer.GIT_EXECUTABLE = str(required_tools[2])
    producer.GIT_ENVIRONMENT = {key: environment[key] for key in ("PATH", "LANG", "LC_ALL", "HOME")}
    target = {"repo": str(repo), "head": producer.git(repo, "rev-parse", "--verify", "HEAD").decode().strip(), "scope": args.scope}
    if selected["target"] != target:
        raise producer.UnknownInput("gate target differs from its trusted declaration")
    ignored = []
    for name in producer.git(repo, "ls-files", "--others", "--ignored", "--exclude-standard", "-z").split(b"\0"):
        if name:
            candidate = (repo / os.fsdecode(name)).absolute()
            try:
                target = candidate.resolve(strict=True)
            except (OSError, RuntimeError) as why:
                raise producer.UnknownInput("ignored candidate link is missing or cyclic") from why
            if any(candidate.is_relative_to(root) and target.is_relative_to(root) for root in output_paths):
                continue
            if not any(target.is_relative_to(root) for root in input_paths + output_paths):
                raise producer.UnknownInput("ignored candidate input is undeclared")
            ignored.append([os.fsdecode(name), os.readlink(candidate) if candidate.is_symlink() else None, str(target)])
    args.dependency_root = roots["dependencyRoots"]
    args.toolchain_root = roots["toolchainRoots"]
    args.lock_file = roots["lockFiles"]
    args.script_file = roots["scriptFiles"]
    args.child_home = str(home)
    args.trust = {**context, "selected": selected, "environment": dict(environment), "ignoredBindings": ignored,
                  "configurationIdentities": configuration_identities, "outputBindings": output_bindings,
                  "pathBindings": tools}
    return args.trust


def launcher_prefix(words):
    if "--" not in words:
        raise Untrusted("producer separator is required", "invalid_request")
    split = words.index("--")
    allowed = {"--policy", "--policy-sha256", "--repo", "--bootstrap-launcher", "--authority-fd", "--bound-policy-sha256"}
    prefix = words[:split]
    if len(prefix) != 2 * len(allowed) or split + 1 == len(words):
        raise Untrusted("complete native launcher arguments are required", "invalid_request")
    seen = set()
    for key, value in zip(prefix[::2], prefix[1::2]):
        if key not in allowed or key in seen or not value or value.startswith("--"):
            raise Untrusted("launcher options need unique exact pairs", "invalid_request")
        seen.add(key)
    return prefix, words[split + 1:]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--policy", required=True)
    parser.add_argument("--policy-sha256", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--bootstrap-launcher")
    parser.add_argument("--authority-fd", type=int)
    parser.add_argument("--bound-policy-sha256")
    words = sys.argv[1:] if argv is None else argv
    authority = None
    try:
        prefix, producer_argv = launcher_prefix(words)
        args = parser.parse_args(prefix)
        if not sys.flags.isolated or not sys.flags.no_site:
            raise Untrusted("independent invocation requires Python -I -S")
        if not args.bootstrap_launcher or sys.argv[0] != "-" or args.authority_fd is None or args.authority_fd < 3:
            raise Untrusted("native bootstrap requires retained launcher bytes on stdin")
        os.set_inheritable(args.authority_fd, False)
        authority = socket.socket(fileno=args.authority_fd)
        authority.settimeout(60)
        if args.policy_sha256 != args.bound_policy_sha256:
            raise Untrusted("policy differs from the authenticated job binding", "digest_mismatch")
        context = load_policy(args.policy, args.policy_sha256, args.repo, launcher_path=args.bootstrap_launcher)
        runtime_locations(context["policy"])
        context["authority"] = authority
        module = types.ModuleType("trusted_gate_receipt")
        module.__file__ = context["policy"]["bundle"]["producer"]["path"]
        exec(compile(context["bytes"]["producer"], module.__file__, "exec"), module.__dict__)
        context["configure"] = lambda parsed: configure(context, parsed, module)
        context["repo"] = str(Path(args.repo).resolve())
        return module.main(trust=context, argv=producer_argv)
    except (OSError, ValueError, TypeError, KeyError, RuntimeError, Untrusted) as why:
        print(json.dumps({"decision": "miss", "reason": "independent provenance or input closure unavailable",
                          "code": getattr(why, "code", "identity_unavailable"),
                          "executed": False, "newExecutions": 0}, sort_keys=True))
        return 1
    finally:
        if authority is not None:
            authority.close()


if __name__ == "__main__":
    raise SystemExit(main())
