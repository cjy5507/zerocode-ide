"""Create independent fixture-only trust declarations for producer tests."""

import argparse
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import sys


def load_module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def configuration_declarations(repo, home, root, producer, launcher, outputs, scripts, dependencies):
    searches = [
        {"directory": str(Path(repo).resolve()), "ancestors": True, "names": [".cargo/config", ".cargo/config.toml"]},
        {"directory": str(home), "ancestors": False, "names": [".cargo/config", ".cargo/config.toml", ".rustup/settings.toml"]},
        {"directory": str(home / "cargo"), "ancestors": False, "names": ["config", "config.toml"]},
        {"directory": str(home / "rustup"), "ancestors": False, "names": ["settings.toml"]},
    ]
    searches += launcher.node_searches(repo, scripts, dependencies)
    git_paths = launcher.git_metadata_paths(repo, producer)
    searches += [{"directory": str(path.parent), "ancestors": False, "names": [path.name]} for path in git_paths]
    launcher.safe_git_configuration(git_paths, producer)
    entries = [{"path": str(home), "sha256": producer.paths_digest([home], excluded=outputs)}]
    for path in launcher.search_paths(searches, producer):
        if path.is_relative_to(home):
            continue
        if path.exists() or path.is_symlink():
            if not path.is_relative_to(root):
                raise AssertionError("fixture refuses existing account or system configuration")
            target = path
            if path.name == "node_modules" and path.is_symlink():
                target = path.resolve(strict=True)
                if not any(target.is_relative_to(Path(value).resolve()) for value in dependencies):
                    raise AssertionError("fixture Node module link leaves declared dependencies")
            entries.append({"path": str(target), "sha256": producer.paths_digest([target])})
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
        "import os, stat, subprocess, sys\n"
        "for key in ['ZEROCODE_AGENT_TEAM_TOKEN', 'ZEROCODE_AGENT_TEAM_TOKEN_FILE', 'ZEROCODE_HOOK_TOKEN']:\n"
        "    assert key not in os.environ, 'fixture first child inherited authority environment'\n"
        "for fd in range(3, 256):\n"
        "    try: mode = os.fstat(fd).st_mode\n"
        "    except OSError: continue\n"
        "    assert not stat.S_ISSOCK(mode), 'fixture first child inherited socket descriptor %d' % fd\n"
        "print('==> verify recipe fixture', flush=True)\n"
        "result = subprocess.run(sys.argv[1:], check=False)\n"
        "print('<== verify recipe fixture rc=%d 0s' % result.returncode, flush=True)\n"
        "raise SystemExit(result.returncode)\n"
    )
    ui_zero = getattr(args, "fixture_ui_zero", False)
    if ui_zero:
        prefix = wrapper.read_text().split("print('==> verify recipe fixture'", 1)[0]
        wrapper.write_text(prefix +
            "print('==> verify recipe rust', flush=True)\n"
            "rust = subprocess.run(sys.argv[1:], check=False)\n"
            "print('<== verify recipe rust rc=%d 0s' % rust.returncode, flush=True)\n"
            "print('==> verify recipe ui', flush=True)\n"
            "ui = subprocess.run(['node', 'fixture_ui_zero.mjs'], check=False)\n"
            "print('<== verify recipe ui rc=%d 0s' % ui.returncode, flush=True)\n"
            "raise SystemExit(rust.returncode or ui.returncode)\n")
    bootstrap = getattr(args, "fixture_bootstrap", None)
    python = Path(bootstrap[bootstrap.index("--python") + 1] if bootstrap else sys.executable).resolve(strict=True)
    if str(wrapper) not in args.command[:5]:
        args.command = [str(python), "-I", "-S", "-B", str(wrapper), *args.command]
    folder = Path(producer.__file__).resolve().parent
    launcher = load_module(folder / "gate_launcher.py", "fixture_launcher")
    git = Path(shutil.which("git", path=os.defpath)).resolve()
    tool_paths = {"python3": python, "git": git}
    tool_paths.update({word: Path(found).resolve() for word in ("node", "sh", "curl") if (found := shutil.which(word))})
    tool_paths.update(getattr(args, "fixture_tools", {}))
    search = root / "path-bin"
    search.mkdir(exist_ok=True)
    for name, target in tool_paths.items():
        alias = search / name
        if alias.is_symlink():
            alias.unlink()
        alias.symlink_to(target)
    transitive = list(tool_paths.values())
    environment = {"HOME": str(home), "USERPROFILE": str(home),
                   "CARGO_HOME": str(home / "cargo"), "RUSTUP_HOME": str(home / "rustup"),
                   "XDG_CONFIG_HOME": str(home / "config"),
                   "XDG_CACHE_HOME": str(home / "cache"), "XDG_DATA_HOME": str(home / "data"),
                   "APPDATA": str(home / "appdata"), "LOCALAPPDATA": str(home / "localappdata"),
                   "PATH": str(search),
                   "LANG": "C", "LC_ALL": "C", "TZ": "UTC", "NODE_OPTIONS": "--no-global-search-paths",
                   "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull}
    if os.name == "nt":
        environment["SystemRoot"] = os.environ["SystemRoot"]
    environment.update(environment_overrides or {})
    searches, configuration = configuration_declarations(args.repo, home, root, producer, launcher, output_roots,
                                                         [wrapper, *args.script_file], args.dependency_root)
    row = lambda path: {"path": str(Path(path).absolute()), "sha256": producer.paths_digest([path])}
    runtime = ({role: {"path": bootstrap[bootstrap.index(path_flag) + 1],
                       "sha256": bootstrap[bootstrap.index(hash_flag) + 1]}
                for role, path_flag, hash_flag in (("python", "--python", "--python-sha256"),
                                                   ("stdlib", "--stdlib", "--stdlib-sha256"))}
               if bootstrap else {"python": {"path": str(python), "sha256": producer.file_digest(python)},
                                  "stdlib": row(args.toolchain_root[0])})
    runtime["libraries"] = getattr(args, "fixture_libraries", [])
    policy = {"version": 3, "runtime": runtime,
              "bundle": {role: {"path": str(path), "sha256": producer.file_digest(path)} for role, path in {
                  "launcher": folder / "gate_launcher.py", "producer": folder / "gate_receipt.py",
                  "scanner": folder / "pii-scan.py", "rules": folder / "pii-rules.json"}.items()},
              "gates": {args.gate: {"command": args.command, "recipes": ["rust", "ui"] if ui_zero else ["fixture"], "testRecipes": ["rust"] if ui_zero else ["fixture"], "profile": args.profile,
                  "target": {"repo": str(Path(args.repo).resolve()), "head": producer.git(args.repo, "rev-parse", "--verify", "HEAD").decode().strip(), "scope": args.scope},
                  "timeout": args.timeout,
                  "maxJobTtlSeconds": math.ceil(args.timeout) + 300,
                  "overheadTimeoutSeconds": 300, "verificationTimeoutSeconds": 300,
                  "environment": environment, "dependencyRoots": [row(path) for path in args.dependency_root],
                  "toolchainRoots": [row(path) for path in [*args.toolchain_root, Path(sys.executable).resolve(), python, git, *transitive]],
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
    parser.add_argument("--cargo", action="store_true")
    parser.add_argument("--bootstrap", action="store_true")
    parser.add_argument("--ui-zero", action="store_true")
    options = parser.parse_args()
    root = Path(options.root)
    args = argparse.Namespace(repo=options.repo, gate="gate-root", profile="test-debug",
        scope="deterministic", closure="declared", dependency_root=[str(root / "deps")],
        toolchain_root=[str(root / "toolchain"), str(root / "bin")], lock_file=[], script_file=[],
        command=[options.child, "--exact", options.test, "--nocapture"], timeout=120)
    producer = load_module(Path(options.producer), "fixture_producer")
    args.fixture_ui_zero = options.ui_zero
    environment, outputs = {}, []
    if options.cargo:
        fixture = load_module(Path(__file__).with_name("gate_cargo_fixture.py"), "offline_cargo_fixture")
        environment, outputs = fixture.prepare(args, root, producer)
        for words in (["add", "."], ["commit", "-qm", "offline cargo fixture"]):
            producer.git(options.repo, *words)
    bootstrap = []
    if options.bootstrap:
        fixture = load_module(Path(__file__).with_name("gate_bootstrap_fixture.py"), "native_bootstrap_fixture")
        bootstrap = fixture.prepare(root, Path(options.producer).with_name("gate_launcher.py"))
        args.fixture_bootstrap = bootstrap
        args.fixture_libraries = json.loads((Path(bootstrap[bootstrap.index("--stdlib") + 1]) / "runtime-libraries.json").read_bytes())
        args.toolchain_root.append(bootstrap[bootstrap.index("--stdlib") + 1])
        args.toolchain_root.extend(entry["resolvedPath"] for entry in args.fixture_libraries)
    context = authorize(args, root, producer, environment_overrides=environment, output_roots=outputs)
    bundle = context["policy"]["bundle"]
    print(json.dumps({"policySha256": context["policySha256"],
                      "producerSha256": producer.digest(producer.encoded({role: row["sha256"] for role, row in bundle.items()})),
                      "command": args.command, "bootstrap": bootstrap}))


if __name__ == "__main__":
    main()
