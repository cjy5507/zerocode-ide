import json
import os
from pathlib import Path
import shutil
import stat
import subprocess

SDK_ENTRY_LIMIT = 100000
SDK_BYTE_LIMIT = 1024 ** 3


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


def fixture_sdk(source, destination):
    source = Path(source).resolve(strict=True)
    destination = Path(destination).absolute()
    if not source.is_dir() or destination.exists() or destination.is_symlink() or destination.is_relative_to(source):
        raise AssertionError("fixture SDK needs a separate new directory")
    links = {}
    logical_bytes = 0
    regular_files = 0
    entries = 0
    for directory, children, leaves in os.walk(source, followlinks=False):
        for name in sorted([*children, *leaves]):
            entries += 1
            path = Path(directory) / name
            information = path.lstat()
            if stat.S_ISREG(information.st_mode):
                logical_bytes += information.st_size
                regular_files += 1
            elif stat.S_ISLNK(information.st_mode):
                try:
                    target = path.resolve(strict=True)
                except (OSError, RuntimeError) as error:
                    raise AssertionError("fixture SDK link is dangling or cyclic") from error
                if not target.is_relative_to(source):
                    raise AssertionError("fixture SDK link leaves its input root")
                links[path.relative_to(source).as_posix()] = {
                    "target": target.relative_to(source).as_posix(), "directory": target.is_dir(),
                    "raw": os.readlink(path),
                }
            elif not stat.S_ISDIR(information.st_mode):
                raise AssertionError("fixture SDK contains a non-file input")
            if entries + 1 > SDK_ENTRY_LIMIT or logical_bytes > SDK_BYTE_LIMIT:
                raise AssertionError("fixture SDK exceeds the bounded copy budget")
    if regular_files == 0:
        raise AssertionError("fixture SDK has no file inputs")
    complete = set()
    cut = set()

    def visit(name, active):
        if name in cut or name in complete:
            return
        if name in active:
            cut.add(name)
            return
        link = links[name]
        if link["directory"]:
            for child in sorted(links):
                if link["target"] == "." or child.startswith(link["target"] + "/"):
                    visit(child, active | {name})
                    if name in cut:
                        break
        complete.add(name)

    for name in sorted(links):
        visit(name, set())

    manifest = (json.dumps({
        "version": 1, "cycleLinks": [{"path": name, **links[name]} for name in sorted(cut)],
    }, sort_keys=True) + "\n").encode()
    link_bytes = sum(max(len(os.fsencode(link["raw"])),
                         len(os.fsencode(os.path.relpath(destination / link["target"], (destination / name).parent))))
                     for name, link in links.items() if name not in cut)
    if logical_bytes + len(manifest) + link_bytes > SDK_BYTE_LIMIT:
        raise AssertionError("fixture SDK exceeds the bounded copy budget")
    copied_entries = 1
    copied_bytes = len(manifest) + link_bytes

    def omitted(directory, names):
        nonlocal copied_entries
        excluded = []
        for name in names:
            path = Path(directory) / name
            relative = path.relative_to(source).as_posix()
            if path.is_symlink():
                if relative not in links or os.readlink(path) != links[relative]["raw"]:
                    raise AssertionError("fixture SDK links changed while copying")
            elif relative in links:
                raise AssertionError("fixture SDK links changed while copying")
            if relative in cut:
                excluded.append(name)
        copied_entries += len(names) - len(excluded)
        if copied_entries > SDK_ENTRY_LIMIT:
            raise AssertionError("fixture SDK exceeds the bounded copy budget")
        return excluded

    def copy_file(original, owned):
        nonlocal copied_bytes
        descriptor = os.open(original, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as input_file:
            before = os.fstat(input_file.fileno())
            if not stat.S_ISREG(before.st_mode):
                raise AssertionError("fixture SDK contains a non-file input")
            with Path(owned).open("xb") as output_file:
                while block := input_file.read(min(1024 ** 2, SDK_BYTE_LIMIT - copied_bytes + 1)):
                    copied_bytes += len(block)
                    if copied_bytes > SDK_BYTE_LIMIT:
                        raise AssertionError("fixture SDK exceeds the bounded copy budget")
                    output_file.write(block)
            after = os.fstat(input_file.fileno())
            identity = lambda row: (row.st_dev, row.st_ino, row.st_size, row.st_mtime_ns, row.st_ctime_ns, row.st_mode)
            if identity(before) != identity(after):
                raise AssertionError("fixture SDK changed while copying")
        shutil.copystat(original, owned, follow_symlinks=False)

    try:
        shutil.copytree(source, destination, symlinks=True, ignore=omitted, copy_function=copy_file)
        for name, link in links.items():
            if name in cut:
                continue
            alias = destination / name
            alias.unlink()
            alias.symlink_to(os.path.relpath(destination / link["target"], alias.parent),
                             target_is_directory=link["directory"])
        with (destination / ".fixture-sdk-links.json").open("xb") as output:
            output.write(manifest)
    except BaseException:
        if destination.is_dir():
            shutil.rmtree(destination)
        raise
    return destination


def prepare(args, root, producer):
    root = Path(root).resolve()
    repo = Path(args.repo).resolve()
    toolchain = installed_path("HERDR_FIXTURE_TOOLCHAIN", ["rustc", "--print", "sysroot"])
    developer = installed_path("HERDR_FIXTURE_DEVELOPER_DIR", ["xcode-select", "-p"])
    sdk = installed_path("HERDR_FIXTURE_SDKROOT", ["xcrun", "--show-sdk-path"])
    linker = installed_path("HERDR_FIXTURE_LINKER", ["xcrun", "--find", "clang"])
    if not sdk.is_relative_to(developer) or not linker.is_relative_to(developer):
        raise AssertionError("offline fixture needs a declared SDK and linker closure")
    linker_tools = linker.parent
    linker_libraries = linker_tools.parent / "lib"
    archive = (linker_tools / "ar").resolve(strict=True)
    linker_driver = (linker_tools / "ld").resolve(strict=True)
    clang_libraries = (linker_libraries / "clang").resolve(strict=True)
    sdk = fixture_sdk(sdk, root / "fixture-sdk" / sdk.name)
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
                   sdk, linker, linker_driver, archive, clang_libraries]
    tool_inputs.extend(sorted(linker_libraries.glob("*.dylib")))
    args.toolchain_root.extend(str(path) for path in tool_inputs)
    args.fixture_tools = {name: toolchain / "bin" / name for name in ("cargo", "rustc", "rustdoc")}
    args.fixture_tools.update({"cc": linker, "clang": linker, "ld": linker_driver, "ar": archive})
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
