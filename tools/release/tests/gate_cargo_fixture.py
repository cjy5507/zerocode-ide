import os
from pathlib import Path
import subprocess


def installed_path(variable, command):
    value = os.environ.get(variable)
    if not value:
        environment = {name: os.environ[name] for name in
                       ("PATH", "HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "DEVELOPER_DIR", "SDKROOT")
                       if name in os.environ}
        environment.update(LANG="C", LC_ALL="C", RUSTUP_AUTO_INSTALL="0")
        result = subprocess.run(command, env=environment, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, check=False, timeout=30)
        if result.returncode:
            raise AssertionError("offline fixture tool discovery failed: %s rc=%d: %s" %
                                 (command[0], result.returncode, result.stderr.decode(errors="replace")))
        value = result.stdout.decode().strip()
        if not value or "\n" in value or "\r" in value:
            raise AssertionError("offline fixture tool discovery is ambiguous")
    path = Path(value)
    if not path.is_absolute():
        raise AssertionError("offline fixture tool discovery needs an absolute path")
    return path.resolve(strict=True)


def prepare(args, root, producer):
    root = Path(root).resolve()
    repo = Path(args.repo).resolve()
    toolchain = installed_path("HERDR_FIXTURE_TOOLCHAIN", ["rustc", "--print", "sysroot"])
    developer = installed_path("HERDR_FIXTURE_DEVELOPER_DIR", ["xcode-select", "-p"])
    sdk = installed_path("HERDR_FIXTURE_SDKROOT", ["xcrun", "--show-sdk-path"])
    linker = installed_path("HERDR_FIXTURE_LINKER", ["xcrun", "--find", "clang"])
    if not sdk.is_relative_to(developer) or not linker.is_relative_to(developer):
        raise AssertionError("offline fixture needs a declared SDK and linker closure")
    package = repo / "cargo-fixture"
    (package / "src").mkdir(parents=True)
    (package / "Cargo.toml").write_text(
        '[package]\nname = "gate-fixture"\nversion = "0.1.0"\nedition = "2024"\n'
        '[workspace]\n[lib]\ndoctest = false\n'
    )
    (package / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "gate-fixture"\nversion = "0.1.0"\n')
    (package / "src/lib.rs").write_text(
        '#[test]\nfn cancelled_and_skipped_are_test_names() {\n'
        '    assert_eq!(env!("FIXTURE_INPUT"), "declared");\n}\n'
        '#[test]\n#[ignore]\nfn fixture_optional() { panic!("ignored fixture ran"); }\n'
    )
    cargo_home = root / "child-home/cargo"
    cargo_home.mkdir(parents=True, exist_ok=True)
    (cargo_home / "config.toml").write_text('[env]\nFIXTURE_INPUT = "declared"\n[net]\noffline = true\n')
    for name in ("registry", "git"):
        (cargo_home / name).mkdir(exist_ok=True)
    (repo / ".gitignore").write_text("cargo-fixture/target/\ntools/release/__pycache__/\n")
    (repo / "tools/release").mkdir(parents=True, exist_ok=True)
    (repo / "tools/release/first_write.py").write_text("VALUE = 1\n")
    script = root / "offline-cargo-recipe.py"
    command = [str(toolchain / "bin/cargo"), "test", "--manifest-path", str(package / "Cargo.toml"),
               "--locked", "--offline", "--lib", "--", "--test-threads=1", "--nocapture"]
    script.write_text(
        "import importlib.util, pathlib, subprocess\n"
        "path = pathlib.Path('tools/release/first_write.py')\n"
        "spec = importlib.util.spec_from_file_location('fixture_first_write', path)\n"
        "spec.loader.exec_module(importlib.util.module_from_spec(spec))\n"
        f"raise SystemExit(subprocess.run({command!r}, check=False).returncode)\n"
    )
    args.command = [str(Path(os.sys.executable).resolve()), str(script)]
    args.script_file = [str(script)]
    args.lock_file = [str(repo / "Cargo.lock"), str(package / "Cargo.lock")]
    tool_inputs = [toolchain / "lib", *(toolchain / "bin" / name for name in ("cargo", "rustc", "rustdoc")),
                   sdk, linker, developer / "usr/bin/ld", developer / "usr/bin/ar", developer / "usr/lib/clang"]
    tool_inputs.extend(sorted((developer / "usr/lib").glob("*.dylib")))
    args.toolchain_root.extend(str(path) for path in tool_inputs)
    args.fixture_tools = {name: toolchain / "bin" / name for name in ("cargo", "rustc", "rustdoc")}
    args.fixture_tools.update({"cc": linker, "clang": linker, "ld": developer / "usr/bin/ld",
                              "ar": developer / "usr/bin/ar"})
    temporary = root / "child-home/tmp"
    temporary.mkdir(exist_ok=True)
    outputs = [package / "target", repo / "tools/release/__pycache__", root / "child-home/cache", temporary]
    outputs.extend(cargo_home / name for name in ("registry", "git", ".package-cache", ".package-cache-mutate", ".global-cache",
                                                ".global-cache-journal", ".global-cache-wal", ".global-cache-shm"))
    environment = {"CARGO_TARGET_DIR": str(package / "target"), "CARGO_BUILD_JOBS": "2",
                   "CARGO_INCREMENTAL": "0", "CARGO_NET_OFFLINE": "true",
                   "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_TEST_DEBUG": "0",
                   "RUSTC": str(toolchain / "bin/rustc"), "RUSTDOC": str(toolchain / "bin/rustdoc"),
                   "CC": str(linker), "SDKROOT": str(sdk),
                   "RUSTFLAGS": "-C linker=" + str(linker), "TMPDIR": str(temporary), "TMP": str(temporary), "TEMP": str(temporary)}
    return environment, [str(path) for path in outputs]
