import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import sysconfig

LIBRARY_DECLARATION = "runtime-libraries.json"
LIBRARY_CAP = 256 * 1024 * 1024


def macho(path):
    with Path(path).open("rb") as file:
        return file.read(4) in (b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xcf\xfa\xed\xfe",
                                b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca", b"\xca\xfe\xba\xbf", b"\xbf\xba\xfe\xca")


def load_names(path):
    result = subprocess.run(["/usr/bin/otool", "-l", str(path)], env={"LANG": "C", "LC_ALL": "C"},
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=30)
    if result.returncode or len(result.stdout) > 1024 * 1024:
        raise AssertionError("fixture runtime dependency metadata unavailable")
    needed, names = False, []
    commands = {"LC_LOAD_DYLIB", "LC_LOAD_WEAK_DYLIB", "LC_REEXPORT_DYLIB", "LC_LOAD_UPWARD_DYLIB", "LC_LAZY_LOAD_DYLIB", "LC_LOAD_DYLINKER"}
    for line in result.stdout.decode().splitlines():
        line = line.strip()
        if line.startswith("cmd "):
            if needed:
                raise AssertionError("fixture runtime dependency is unknown")
            needed = line[4:] in commands
        elif needed and line.startswith("name ") and " (offset " in line:
            names.append(line[5:].rsplit(" (offset ", 1)[0])
            needed = False
    if needed:
        raise AssertionError("fixture runtime dependency is incomplete")
    return names


def runtime_libraries(python, source):
    if sys.platform != "darwin" or not macho(python):
        raise AssertionError("fixture runtime dependency format is unsupported")
    pending, seen, libraries = [Path(python).resolve()], set(), {}
    for directory, children, files in os.walk(source):
        children[:] = [name for name in children if name not in {"site-packages", "__pycache__"}]
        pending.extend((Path(directory) / name).resolve() for name in files if macho(Path(directory) / name))
    while pending:
        image = pending.pop()
        if image in seen:
            continue
        seen.add(image)
        if not macho(image):
            raise AssertionError("fixture transitive runtime dependency is unsupported")
        for name in load_names(image):
            path = Path(name)
            if not path.is_absolute() or ".." in path.parts:
                raise AssertionError("fixture runtime dependency cannot be resolved exactly")
            if path.is_relative_to("/System/Library") or path.is_relative_to("/usr/lib"):
                continue
            resolved = path.resolve(strict=True)
            if not resolved.is_file():
                raise AssertionError("fixture runtime dependency is not a regular file")
            if name not in libraries:
                libraries[name] = {"path": name, "resolvedPath": str(resolved), "sha256": file_hash(resolved)}
                pending.append(resolved)
    rows = [libraries[name] for name in sorted(libraries)]
    if len(rows) > 16 or len({row["resolvedPath"] for row in rows}) != len(rows) or sum(Path(row["resolvedPath"]).stat().st_size for row in rows) > LIBRARY_CAP:
        raise AssertionError("fixture runtime dependency allocation or identity is unsupported")
    return rows


def file_hash(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def tree_hash(root):
    root = Path(root)
    rows = []
    pending = [root]
    while pending:
        path = pending.pop()
        meta = path.lstat()
        relative = path.relative_to(root).as_posix()
        mode = stat.S_IMODE(meta.st_mode) if os.name == "posix" else int(not os.access(path, os.W_OK))
        if path.is_symlink():
            rows.append([relative, "link", mode, os.readlink(path)])
        elif path.is_dir():
            rows.append([relative, "directory", mode])
            pending.extend(reversed(sorted(path.iterdir())))
        else:
            rows.append([relative, "file", mode, file_hash(path)])
    return hashlib.sha256(json.dumps(rows, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def prepare(root, launcher):
    root = Path(root).resolve()
    runtime = root / "python-runtime"
    stdlib = runtime / "lib" / ("python%d.%d" % sys.version_info[:2])
    python = runtime / "bin" / ("python%d.%d" % sys.version_info[:2])
    original = Path(sys.executable).resolve()
    source = Path(sysconfig.get_path("stdlib")).resolve()
    libraries = runtime_libraries(original, source)
    declaration = json.dumps(libraries, sort_keys=True, separators=(",", ":")) + "\n"
    if len(declaration.encode()) > 64 * 1024:
        raise AssertionError("fixture runtime declaration exceeds its allocation")
    allowed = {Path(row["resolvedPath"]): row["sha256"] for row in libraries}
    linked = []
    size = original.stat().st_size + len(declaration.encode())
    for directory, children, files in os.walk(source):
        children[:] = [name for name in children if name not in {"site-packages", "__pycache__"}]
        for name in children + files:
            path = Path(directory) / name
            if path.is_symlink():
                target = path.resolve(strict=True)
                if not target.is_file() or not (target.is_relative_to(source) or target in allowed):
                    raise AssertionError("fixture standard library contains an undeclared link")
                linked.append((path.relative_to(source), target, file_hash(target)))
        size += sum((Path(directory) / name).stat().st_size for name in files)
        if size > 256 * 1024 * 1024:
            raise AssertionError("fixture standard library exceeds its scratch allocation")
    python.parent.mkdir(parents=True)
    shutil.copy2(original, python)
    shutil.copytree(source, stdlib,
                    ignore=shutil.ignore_patterns("site-packages", "__pycache__"))
    for relative, target, expected in linked:
        if file_hash(target) != expected or file_hash(stdlib / relative) != expected or (stdlib / relative).is_symlink():
            raise AssertionError("fixture linked runtime input changed while copying")
    for entry in libraries:
        if str(Path(entry["path"]).resolve(strict=True)) != entry["resolvedPath"] or file_hash(entry["resolvedPath"]) != entry["sha256"]:
            raise AssertionError("fixture runtime input changed while copying")
    (runtime / LIBRARY_DECLARATION).write_text(declaration)
    return ["--launcher", str(Path(launcher).resolve()), "--launcher-sha256", file_hash(launcher),
            "--python", str(python), "--python-sha256", file_hash(python),
            "--stdlib", str(runtime), "--stdlib-sha256", tree_hash(runtime)]


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--launcher", required=True)
    options = parser.parse_args()
    print(json.dumps(prepare(options.root, options.launcher)))
