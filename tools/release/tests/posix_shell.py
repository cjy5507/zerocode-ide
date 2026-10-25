"""The POSIX shell the release tools' tests start their bash scripts with.

Python starts a bare "bash" through CreateProcess, which looks in the Windows
directory before PATH. On a GitHub Windows runner that finds WSL's launcher
(System32\\bash.exe: "Windows Subsystem for Linux has no installed
distributions", exit 1), not the Git for Windows bash the justfile pins with
`set windows-shell`. t-74384: 78 of test_lane.py's 146 Windows failures print
that line, 47 more fail the same way without printing it, and a bash that always
exits 1 reproduces 129 of the 146 on a Mac. Elsewhere `BASH` is the plain
"bash", so nothing changes on macOS or Linux.
"""

import ntpath
import os
import shutil
from pathlib import Path


def pick_bash(path_entries, windows_dir, isfile):
    """The first bash.exe on PATH that is not in the Windows directory, or None.
    ntpath on purpose: the rule is about Windows paths and is tested on any OS."""
    root = ntpath.normcase(ntpath.normpath(windows_dir))
    for directory in path_entries:
        if not directory:
            continue
        here = ntpath.normcase(ntpath.normpath(directory))
        if here == root or here.startswith(root + "\\"):
            continue
        candidate = ntpath.join(directory, "bash.exe")
        if isfile(candidate):
            return candidate
    return None


def find_bash() -> str:
    if os.name != "nt":
        return "bash"
    found = pick_bash(os.environ.get("PATH", "").split(os.pathsep),
                      os.environ.get("SystemRoot") or os.environ.get("windir") or r"C:\Windows", os.path.isfile)
    if found:
        return found
    git = shutil.which("git")  # Git for Windows keeps bash.exe beside git: ...\Git\bin or ...\Git\cmd\..\bin
    if git:
        folder = Path(git).resolve().parent
        for relative in ("bin/bash.exe", "../bin/bash.exe", "../usr/bin/bash.exe"):
            candidate = (folder / relative).resolve()
            if candidate.is_file():
                return str(candidate)
    raise RuntimeError("no POSIX bash outside the Windows directory on PATH (Git for Windows ships one)")


BASH = find_bash()

# What a child needs on Windows to start at all when a test hands it a bare
# environment (Python and Git for Windows both read SystemRoot).
WINDOWS_ENV = ({key: os.environ[key] for key in ("SystemRoot", "SystemDrive", "TEMP", "TMP", "COMSPEC", "PATHEXT")
                if key in os.environ} if os.name == "nt" else {})


def posix(path) -> str:
    """A path in the form bash's own tools split on: C:/a/b, not C:\\a\\b.
    On macOS and Linux it is the string the path already was."""
    return Path(path).as_posix()
