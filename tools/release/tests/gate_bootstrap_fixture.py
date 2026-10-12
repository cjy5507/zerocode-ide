import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import sys
import sysconfig


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
    python.parent.mkdir(parents=True)
    shutil.copy2(Path(sys.executable).resolve(), python)
    source = Path(sysconfig.get_path("stdlib"))
    size = 0
    for directory, children, files in os.walk(source):
        children[:] = [name for name in children if name not in {"site-packages", "__pycache__"}]
        if any((Path(directory) / name).is_symlink() for name in children + files):
            raise AssertionError("fixture standard library contains an undeclared link")
        size += sum((Path(directory) / name).stat().st_size for name in files)
        if size > 256 * 1024 * 1024:
            raise AssertionError("fixture standard library exceeds its scratch allocation")
    shutil.copytree(source, stdlib,
                    ignore=shutil.ignore_patterns("site-packages", "__pycache__"))
    return ["--launcher", str(Path(launcher).resolve()), "--launcher-sha256", file_hash(launcher),
            "--python", str(python), "--python-sha256", file_hash(python),
            "--stdlib", str(runtime), "--stdlib-sha256", tree_hash(runtime)]


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--launcher", required=True)
    options = parser.parse_args()
    print(json.dumps(prepare(options.root, options.launcher)))
