#!/usr/bin/env python3
"""Launch a pinned receipt producer from a coordinator-reviewed external manifest.

The root coordinator creates and updates the manifest from a reviewed trusted commit.
It independently pins this launcher and the manifest before invoking Python with -I -S.
The manifest is never generated from candidate declarations. This is a declared-input
cache, not isolation from arbitrary code running as the same operating-system user.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
import types

MAX_POLICY = 1024 * 1024
MANDATORY = ["native", "windows", "flake-judgment", "publish"]
BUILD_ENV = {
    "PATH", "LANG", "LC_ALL", "TZ", "SystemRoot", "HOME", "USERPROFILE",
    "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA",
    "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL", "CARGO_NET_OFFLINE", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
    "RUSTFLAGS", "RUSTDOCFLAGS", "CC", "CXX", "AR", "SDKROOT", "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET", "TMPDIR", "TMP", "TEMP", "NODE_PATH",
    "npm_config_cache", "npm_config_offline",
}
PATH_ENV = {
    "HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "APPDATA",
    "LOCALAPPDATA", "CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR", "SDKROOT", "DEVELOPER_DIR",
    "TMPDIR", "TMP", "TEMP", "npm_config_cache",
}


class Untrusted(Exception):
    """Independent provenance or declared input closure is unavailable."""


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
    if not isinstance(policy, dict) or set(policy) != {"version", "trustedCommit", "bundle", "gates"} or policy["version"] != 1:
        raise Untrusted("unsupported trust manifest")
    if not isinstance(policy["trustedCommit"], str) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", policy["trustedCommit"]):
        raise Untrusted("manifest has no root-reviewed trusted commit")
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


def configure(context, args, producer):
    selected = context["policy"]["gates"].get(args.gate)
    required = {"command", "recipes", "profile", "environment", "dependencyRoots", "toolchainRoots",
                "configurationRoots", "lockFiles", "scriptFiles", "outputRoots", "mandatoryChecks"}
    if not isinstance(selected, dict) or set(selected) != required:
        raise producer.UnknownInput("gate has no complete independently trusted declaration")
    if selected["command"] != args.command or selected["profile"] != args.profile:
        raise producer.UnknownInput("gate command or profile differs from its trusted recipe")
    if selected["mandatoryChecks"] != MANDATORY or not isinstance(selected["recipes"], list) or not selected["recipes"]:
        raise producer.UnknownInput("gate has no complete recipe list and mandatory non-reuse checks")
    if len(selected["recipes"]) != len(set(selected["recipes"])) or any(
        not re.fullmatch(r"[A-Za-z0-9_-]+", name) for name in selected["recipes"]
    ):
        raise producer.UnknownInput("gate recipe list is invalid")
    if args.scope != "deterministic" or args.closure != "declared":
        raise producer.UnknownInput("trusted cache needs the deterministic declared scope")
    environment = selected["environment"]
    if not isinstance(environment, dict) or set(environment) - BUILD_ENV or any(
        not isinstance(value, str) or re.search(r"[\r\n\0]", value) for value in environment.values()
    ):
        raise producer.UnknownInput("child environment includes undeclared values or authority")
    if any(environment.get(key) != value for key, value in (("LANG", "C"), ("LC_ALL", "C"), ("TZ", "UTC"))):
        raise producer.UnknownInput("child locale is not fixed")
    repo = Path(args.repo).resolve()
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
    roots = {}
    for role in ("dependencyRoots", "toolchainRoots", "configurationRoots", "lockFiles", "scriptFiles"):
        rows = selected[role]
        if not isinstance(rows, list) or not rows:
            raise producer.UnknownInput("gate input role is undeclared")
        roots[role] = []
        for row in rows:
            if set(row) != {"path", "sha256"} or not Path(row["path"]).is_absolute():
                raise producer.UnknownInput("gate input root has no identity")
            if producer.paths_digest([row["path"]]) != row["sha256"]:
                raise producer.UnknownInput("declared input root changed")
            roots[role].append(row["path"])
    outputs = selected["outputRoots"]
    if not isinstance(outputs, list) or any(not Path(value).is_absolute() for value in outputs):
        raise producer.UnknownInput("output roots must be explicit absolute paths")
    input_paths = [Path(value).resolve() for values in roots.values() for value in values]
    output_paths = [Path(value).resolve() for value in outputs]
    if any(path.is_relative_to(repo) for path in (Path(value).resolve() for value in roots["toolchainRoots"])):
        raise producer.UnknownInput("candidate-owned toolchain is untrusted")
    if any(path == repo or repo.is_relative_to(path) for path in output_paths):
        raise producer.UnknownInput("output root can hide candidate inputs")
    if any(a.is_relative_to(b) or b.is_relative_to(a) for a in input_paths for b in output_paths):
        raise producer.UnknownInput("input and output declarations overlap")
    for key in PATH_ENV.intersection(environment):
        value = Path(environment[key])
        if not value.is_absolute() or not any(value.resolve().is_relative_to(root) for root in input_paths + output_paths):
            raise producer.UnknownInput("configuration or cache path has no declared role")
    for part in environment.get("NODE_PATH", "").split(os.pathsep) if "NODE_PATH" in environment else []:
        value = Path(part)
        if not value.is_absolute() or not any(value.resolve().is_relative_to(root) for root in input_paths):
            raise producer.UnknownInput("node module search path has no declared input identity")
    executable = producer.command_executable(repo, args.command[0], environment)
    tool_roots = [Path(value).resolve() for value in roots["toolchainRoots"]]
    required_tools = [Path(sys.executable).resolve(), executable,
                      producer.command_executable(repo, "git", environment)]
    if any(tool is None or not any(tool.is_relative_to(root) for root in tool_roots) for tool in required_tools):
        raise producer.UnknownInput("an invoked tool has no independently declared identity")
    producer.GIT_EXECUTABLE = str(required_tools[2])
    producer.GIT_ENVIRONMENT = {key: environment[key] for key in ("PATH", "LANG", "LC_ALL", "HOME")}
    ignored = []
    for name in producer.git(repo, "ls-files", "--others", "--ignored", "--exclude-standard", "-z").split(b"\0"):
        if name:
            candidate = (repo / os.fsdecode(name)).absolute()
            target = candidate.resolve(strict=True)
            if not any(target.is_relative_to(root) for root in input_paths + output_paths):
                raise producer.UnknownInput("ignored candidate input is undeclared")
            ignored.append([os.fsdecode(name), os.readlink(candidate) if candidate.is_symlink() else None, str(target)])
    args.dependency_root = roots["dependencyRoots"] + roots["configurationRoots"]
    args.toolchain_root = roots["toolchainRoots"]
    args.lock_file = roots["lockFiles"]
    args.script_file = roots["scriptFiles"]
    args.child_home = str(home)
    args.trust = {**context, "selected": selected, "environment": dict(environment), "ignoredBindings": ignored}
    return args.trust


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True)
    parser.add_argument("--policy-sha256", required=True)
    parser.add_argument("--repo", required=True)
    words = sys.argv[1:] if argv is None else argv
    if "--" not in words:
        parser.error("separate the producer arguments with --")
    split = words.index("--")
    args = parser.parse_args(words[:split])
    producer_argv = words[split + 1:]
    try:
        if not sys.flags.isolated or not sys.flags.no_site:
            raise Untrusted("independent invocation requires Python -I -S")
        context = load_policy(args.policy, args.policy_sha256, args.repo)
        module = types.ModuleType("trusted_gate_receipt")
        module.__file__ = context["policy"]["bundle"]["producer"]["path"]
        exec(compile(context["bytes"]["producer"], module.__file__, "exec"), module.__dict__)
        context["configure"] = lambda parsed: configure(context, parsed, module)
        context["repo"] = str(Path(args.repo).resolve())
        return module.main(trust=context, argv=producer_argv)
    except (OSError, ValueError, TypeError, KeyError, Untrusted):
        print(json.dumps({"decision": "miss", "reason": "independent provenance or input closure unavailable",
                          "executed": False, "newExecutions": 0}, sort_keys=True))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
