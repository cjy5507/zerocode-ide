import os
from pathlib import Path


def prepare(args, root, producer):
    root = Path(root).resolve()
    repo = Path(args.repo).resolve()
    toolchain = Path(os.environ["HERDR_FIXTURE_TOOLCHAIN"]).resolve(strict=True)
    developer = Path(os.environ["HERDR_FIXTURE_DEVELOPER_DIR"]).resolve(strict=True)
    sdk = Path(os.environ["HERDR_FIXTURE_SDKROOT"]).resolve(strict=True)
    linker = Path(os.environ["HERDR_FIXTURE_LINKER"]).resolve(strict=True)
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
    args.toolchain_root.extend([str(toolchain), str(developer)])
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
                   "CC": str(linker), "SDKROOT": str(sdk), "DEVELOPER_DIR": str(developer),
                   "RUSTFLAGS": "-C linker=" + str(linker), "TMPDIR": str(temporary), "TMP": str(temporary), "TEMP": str(temporary)}
    return environment, [str(path) for path in outputs]
