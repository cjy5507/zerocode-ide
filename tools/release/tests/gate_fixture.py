"""Create independent fixture-only trust declarations for producer tests."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys


def load_module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def configuration_declarations(repo, home, root, producer, launcher, outputs):
    searches = [
        {"directory": str(Path(repo).resolve()), "ancestors": True, "names": [".cargo/config", ".cargo/config.toml"]},
        {"directory": str(home), "ancestors": False, "names": [".cargo/config", ".cargo/config.toml", ".rustup/settings.toml"]},
        {"directory": str(home / "cargo"), "ancestors": False, "names": ["config", "config.toml"]},
        {"directory": str(home / "rustup"), "ancestors": False, "names": ["settings.toml"]},
    ]
    entries = [{"path": str(home), "sha256": producer.paths_digest([home], excluded=outputs)}]
    for path in launcher.search_paths(searches, producer):
        if path.is_relative_to(home):
            continue
        if path.exists() or path.is_symlink():
            if not path.is_relative_to(root):
                raise AssertionError("fixture refuses existing account or system configuration")
            entries.append({"path": str(path), "sha256": producer.paths_digest([path])})
        else:
            entries.append({"path": str(path), "state": "absent"})
    return searches, entries


def authorize(args, root, producer, environment_overrides=None, output_roots=()):
    root = Path(root).resolve()
    home = root / "child-home"
    home.mkdir(exist_ok=True)
    (home / "fixture-config").write_text("no account credentials\n")
    wrapper = root / "recipe.py"
    wrapper.write_text(
        "import subprocess, sys\n"
        "print('==> verify recipe fixture', flush=True)\n"
        "result = subprocess.run(sys.argv[1:], check=False)\n"
        "print('<== verify recipe fixture rc=%d 0s' % result.returncode, flush=True)\n"
        "raise SystemExit(result.returncode)\n"
    )
    if str(wrapper) not in args.command[:5]:
        args.command = [sys.executable, "-I", "-S", "-B", str(wrapper), *args.command]
    folder = Path(producer.__file__).resolve().parent
    launcher = load_module(folder / "gate_launcher.py", "fixture_launcher")
    python = Path(sys.executable).resolve()
    git = Path(shutil.which("git", path=os.defpath)).resolve()
    transitive = [Path(found).resolve() for word in ("node", "sh", "curl") if (found := shutil.which(word))]
    environment = {"HOME": str(home), "USERPROFILE": str(home),
                   "CARGO_HOME": str(home / "cargo"), "RUSTUP_HOME": str(home / "rustup"),
                   "XDG_CONFIG_HOME": str(home / "config"),
                   "XDG_CACHE_HOME": str(home / "cache"), "XDG_DATA_HOME": str(home / "data"),
                   "APPDATA": str(home / "appdata"), "LOCALAPPDATA": str(home / "localappdata"),
                   "PATH": os.pathsep.join(dict.fromkeys([str(python.parent), *(str(path.parent) for path in transitive), *os.defpath.split(os.pathsep)])),
                   "LANG": "C", "LC_ALL": "C", "TZ": "UTC"}
    if os.name == "nt":
        environment["SystemRoot"] = os.environ["SystemRoot"]
    environment.update(environment_overrides or {})
    searches, configuration = configuration_declarations(args.repo, home, root, producer, launcher, output_roots)
    row = lambda path: {"path": str(Path(path).absolute()), "sha256": producer.paths_digest([path])}
    policy = {"version": 1, "trustedCommit": "a" * 40,
              "bundle": {role: {"path": str(path), "sha256": producer.file_digest(path)} for role, path in {
                  "launcher": folder / "gate_launcher.py", "producer": folder / "gate_receipt.py",
                  "scanner": folder / "pii-scan.py", "rules": folder / "pii-rules.json"}.items()},
              "gates": {args.gate: {"command": args.command, "recipes": ["fixture"], "profile": args.profile,
                  "environment": environment, "dependencyRoots": [row(path) for path in args.dependency_root],
                  "toolchainRoots": [row(path) for path in [*args.toolchain_root, python, git, *transitive]],
                  "configurationRoots": configuration, "configurationSearches": searches,
                  "lockFiles": [row(path) for path in args.lock_file or [Path(args.repo) / "Cargo.lock"]],
                  "scriptFiles": [row(path) for path in [wrapper, *args.script_file]], "outputRoots": list(output_roots),
                  "mandatoryChecks": launcher.MANDATORY}}}
    policy_path = root / "trusted-policy.json"
    raw = producer.encoded(policy)
    policy_path.write_bytes(raw)
    context = launcher.load_policy(policy_path, producer.digest(raw), args.repo)
    context["configure"] = lambda parsed: launcher.configure(context, parsed, producer)
    context["repo"] = str(Path(args.repo).resolve())
    args.trust = context
    producer.provenance(args)
    return context


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--producer", required=True)
    parser.add_argument("--child", required=True)
    parser.add_argument("--test", required=True)
    options = parser.parse_args()
    root = Path(options.root)
    args = argparse.Namespace(repo=options.repo, gate="gate-root", profile="test-debug",
        scope="deterministic", closure="declared", dependency_root=[str(root / "deps")],
        toolchain_root=[str(root / "toolchain"), str(root / "bin")], lock_file=[], script_file=[],
        command=[options.child, "--exact", options.test, "--nocapture"])
    producer = load_module(Path(options.producer), "fixture_producer")
    context = authorize(args, root, producer)
    print(json.dumps({"policySha256": context["policySha256"], "command": args.command}))


if __name__ == "__main__":
    main()
