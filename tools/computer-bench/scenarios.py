"""The bench's scenario kinds (docs/design/computer-use-bench.md §1), table-driven:
each fixture, setup, oracle and teardown kind is written once here, and a
scenario is only rows of them in bench.json. Every desktop call goes through
the `shim` the runner hands in — `shim(argv) -> envelope or None` — so the
runner decides which evidence folder a call lands in, and every step that
opens or acts first calls the runner's `guard`, which ends the bench when the
desk is not the runner's to touch. Setup opens, waits and clears a display;
oracles read; teardown quits only what setup opened. A look that fails is
never taken for an empty desk.
"""
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import time
import zlib

DS_STORE = ".DS_Store"
DIGIT_RUN = re.compile(r"\d(?:[\d.,   ]*\d)?")
# A tree line of `read --app`: tabs for depth, the element's number, then its
# role and words (SnapshotRendering.swift `line(index:node:)`).
TREE_LINE = re.compile(r"^\t*\d+(?: (.*))?$")
# Role words whose digits are a key's face, not what a display shows.
KEY_ROLES = ("button",)


class SetupFailed(Exception):
    """A scenario that could not be set up: its run is not judged."""


def expand(reference):
    """A reference's command lines: an argv as it is, or
    {"for": name, "in": [...], "do": [argv...]} with "{name}" filled."""
    out = []
    for entry in reference:
        if isinstance(entry, dict):
            name = "{" + entry["for"] + "}"
            for value in entry["in"]:
                out.extend([word.replace(name, str(value)) for word in argv] for argv in entry["do"])
        else:
            out.append(list(entry))
    return out


# ---------------------------------------------------------------- fixtures --

def pdf_bytes(title):
    """A one-page PDF with `title` on it, byte-for-byte the same every time."""
    stream = f"BT /F1 24 Tf 72 720 Td ({title}) Tj ET".encode()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = b"%PDF-1.4\n"
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{offset:010d} 00000 n \n".encode() for offset in offsets)
    out += f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return out


def png1_bytes():
    """A 1×1 opaque white PNG."""
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
    header = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    pixels = zlib.compress(b"\x00\xff\xff\xff")
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", pixels) + chunk(b"IEND", b"")


def fixture_bytes(spec):
    kind = spec["kind"]
    if kind == "text":
        return spec["text"].encode()
    if kind == "pdf":
        return pdf_bytes(spec["title"])
    if kind == "png1":
        return png1_bytes()
    raise ValueError(f"unknown fixture kind {kind!r}")


def make_fixtures(work, specs):
    """Write every fixture under `work`; answers {relpath: sha256}."""
    hashes = {}
    for spec in specs:
        path = os.path.join(work, spec["path"])
        os.makedirs(os.path.dirname(path), exist_ok=True)
        body = fixture_bytes(spec)
        with open(path, "wb") as handle:
            handle.write(body)
        hashes[spec["path"]] = hashlib.sha256(body).hexdigest()
    return hashes


# -------------------------------------------------------------- the desktop --

def result(envelope):
    return (envelope or {}).get("result") or {}


def ok(envelope):
    return bool(envelope and envelope.get("ok"))


def app_pids(name):
    """The pids of the processes named `name` (pgrep -x); None when pgrep
    itself could not answer."""
    try:
        answer = subprocess.run(["pgrep", "-x", name], capture_output=True, text=True)
    except OSError:
        return None
    if answer.returncode not in (0, 1):
        return None
    return {int(word) for word in answer.stdout.split() if word.isdigit()} if answer.returncode == 0 else set()


def read_words(shim, app, ocr=False):
    """What `read --app` shows, a line per element: OCR's lines as read, or
    the accessibility tree's without their element numbers."""
    argv = ["read", "--app", app] + (["--ocr"] if ocr else [])
    answer = result(shim(argv))
    if ocr or answer.get("source") == "ocr":
        lines = [line.get("text", "") for line in answer.get("lines") or [] if isinstance(line, dict)]
        return lines or str(answer.get("text") or "").splitlines()
    words = []
    for line in str(answer.get("text") or "").splitlines():
        match = TREE_LINE.match(line)
        words.append((match.group(1) or "") if match else line.strip())
    return words


def display_lines(shim, app, ocr=False):
    """What a display can show: every line but a key's face."""
    return [line for line in read_words(shim, app, ocr) if ocr or not line.startswith(KEY_ROLES)]


def digit_runs(line):
    """Each number written on a line, its grouping (1,234 · 1 234 · 1.234) dropped."""
    return [re.sub(r"\D", "", run) for run in DIGIT_RUN.findall(line)]


def shows(lines, digits):
    return any(digits in digit_runs(line) for line in lines)


def windows(shim):
    """Every window on the desk, or None when the look failed."""
    answer = shim(["list-all-windows"])
    if not ok(answer):
        return None
    return [window for window in result(answer).get("windows") or [] if isinstance(window, dict)]


def window_app(window):
    app = window.get("app")
    return app.get("name") if isinstance(app, dict) else app


def matching_windows(shim, app, title):
    """The windows of `app` (any app when None) whose title holds `title`, or
    None when the look failed."""
    seen = windows(shim)
    if seen is None:
        return None
    return [window for window in seen if (not app or window_app(window) == app) and title in str(window.get("title") or "")]


# ------------------------------------------------------------------- setup --

def setup(work, steps, shim, values, state, guard):
    """Run a scenario's setup rows; raise SetupFailed with why. `guard()`
    runs before each step that opens or acts."""
    for step in steps:
        kind = step["kind"]
        if kind in ("open_fresh", "open_path"):
            target = [os.path.join(work, step["path"])] if step.get("path") else []
            argv = ["open", "-a", step["app"], *target] if kind == "open_fresh" else ["open", *target]
            guard()
            if subprocess.run(argv, capture_output=True).returncode != 0:
                raise SetupFailed(f"{' '.join(argv)} failed")
        elif kind == "wait_window":
            answer = shim(["wait-for", "--app", step["app"], "--window", step["title"], "--timeout-ms", str(values["setup_timeout_ms"])])
            if not ok(answer):
                raise SetupFailed(f"no {step['app']} window titled {step['title']!r}")
            if step.get("own"):
                # The process this run opened: teardown quits it and no other.
                state.setdefault("pids", {})[step["app"]] = app_pids(step["app"])
        elif kind == "clear":
            for argv in [["activate", "--app", step["app"]]] + [["key", "--key", step["key"]]] * step["times"]:
                guard()
                if not ok(shim(argv)):
                    raise SetupFailed(f"{' '.join(argv)} was refused")
        elif kind == "expect_display":
            if not shows(display_lines(shim, step["app"]), step["digits"]):
                raise SetupFailed(f"{step['app']} does not show {step['digits']}")
        elif kind == "display_lacks":
            for ocr in (False, True):
                if shows(display_lines(shim, step["app"], ocr), step["digits"]):
                    raise SetupFailed(f"{step['app']} already shows {step['digits']}" + (" (ocr)" if ocr else ""))
        elif kind == "read_lacks":
            text = "\n".join(read_words(shim, step["app"]))
            found = [word for word in step["words"] if re.search(r"\b" + re.escape(word) + r"\b", text)]
            if found:
                raise SetupFailed(f"{step['app']} shows {', '.join(found)}: not its basic face")
        else:
            raise SetupFailed(f"unknown setup kind {kind!r}")


def desk_ready(scenario, shim):
    """Before a run: none of its apps already running, none of its windows
    already open — else whatever is there is somebody's, and the run is not
    set up (SetupFailed)."""
    for app in scenario.get("apps_not_running") or []:
        pids = app_pids(app)
        if pids is None:
            raise SetupFailed(f"could not tell whether {app} is running")
        if pids:
            raise SetupFailed(f"{app} is already running")
    for title in scenario.get("stale_window_titles") or []:
        found = matching_windows(shim, None, title)
        if found is None:
            raise SetupFailed("could not list the windows")
        if found:
            raise SetupFailed(f"a window titled {title} is already open")


# ----------------------------------------------------------------- oracles --

def check(work, spec, shim, last_message, hashes):
    """One oracle check: (pass, detail)."""
    kind = spec["kind"]
    if kind == "file_text":
        path = os.path.join(work, spec["path"])
        try:
            with open(path, "rb") as handle:
                raw = handle.read()
        except OSError as error:
            return False, f"{spec['path']}: {error.strerror}"
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            return False, f"{spec['path']} is not UTF-8 text"
        text = text[:-1] if text.endswith("\n") else text
        return text == spec["expect"], f"{spec['path']} holds {text[:80]!r}"
    if kind == "no_window":
        found = matching_windows(shim, spec["app"], spec["title"])
        if found is None:
            return False, "could not list the windows"
        return not found, f"{len(found)} {spec['app']} window(s) titled {spec['title']!r}"
    if kind == "display_digits":
        for ocr in (False, True):
            if shows(display_lines(shim, spec["app"], ocr), spec["expect"]):
                return True, f"{spec['app']} shows {spec['expect']}" + (" (ocr)" if ocr else "")
        return False, f"{spec['app']} does not show {spec['expect']}"
    if kind == "answer_digits":
        digits = re.sub(r"\D", "", last_message or "")
        return spec["expect"] in digits, "the answer names it" if spec["expect"] in digits else "the answer does not name it"
    if kind == "tree":
        root = os.path.join(work, spec["root"])
        seen = {}
        for dirpath, dirs, files in os.walk(root):
            rel = os.path.relpath(dirpath, root)
            for name in dirs:
                seen[os.path.normpath(os.path.join(rel, name))] = "dir"
            for name in files:
                if name == DS_STORE:
                    continue
                with open(os.path.join(dirpath, name), "rb") as handle:
                    seen[os.path.normpath(os.path.join(rel, name))] = hashlib.sha256(handle.read()).hexdigest()
        want = {path: "dir" if kind_ == "dir" else hashes.get(kind_.split(":", 1)[1]) for path, kind_ in spec["expect"].items()}
        missing = sorted(set(want) - set(seen))
        extra = sorted(set(seen) - set(want))
        changed = sorted(path for path in set(want) & set(seen) if want[path] != seen[path])
        detail = "; ".join(f"{label}: {', '.join(paths)}" for label, paths in (("missing", missing), ("extra", extra), ("changed", changed)) if paths)
        return not (missing or extra or changed), detail or "the tree is as expected"
    raise ValueError(f"unknown oracle kind {kind!r}")


def oracle(work, specs, shim, last_message, hashes):
    """Every check of a scenario: all must pass."""
    checks = []
    for spec in specs:
        passed, detail = check(work, spec, shim, last_message, hashes)
        checks.append({"kind": spec["kind"], "pass": passed, "detail": detail})
    return {"pass": all(item["pass"] for item in checks), "checks": checks}


# ---------------------------------------------------------------- teardown --

def exited(app, within_ms):
    """Whether no process named `app` is left, waiting up to `within_ms`
    for one that is still on its way out after its last window closed."""
    deadline = time.monotonic() + within_ms / 1000
    while app_pids(app):
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.1)
    return True


def quit_own(step, shim, values, state):
    """Quit the app this run opened, and only it: not when another process
    of that name is running now, and never forced while it shows a window
    that is not the scenario's. Answers what was left, or None."""
    app = step["app"]
    own = (state.get("pids") or {}).get(app)
    now = app_pids(app)
    if not now:
        return None
    if own is None or now != own:
        return f"{app} left running: it is not the process this run opened"
    shim(["quit", "--app", app])
    gone = shim(["wait-for", "--app", app, "--window", step.get("title") or app, "--absent", "--timeout-ms", str(values["quit_wait_ms"])])
    if ok(gone) and exited(app, values["quit_wait_ms"]):
        return None
    seen = matching_windows(shim, app, "")
    own_titles = step.get("titles") or [step.get("title") or app]
    foreign = None if seen is None else [window for window in seen if not any(title in str(window.get("title") or "") for title in own_titles)]
    if foreign is None or foreign:
        return f"{app} left running: it did not quit and shows a window that is not the bench's"
    if app_pids(app) == own:
        shim(["quit", "--app", app, "--force"])
        exited(app, values["quit_wait_ms"])
    return None


def teardown(work, steps, shim, values, state, work_root):
    """Leave the desk as it was found: quit what setup opened, close the
    scenario's windows — then remove the work folder, fenced to the bench's
    own root. Answers what was left as it was."""
    left = []
    for step in steps:
        kind = step["kind"]
        if kind == "quit":
            note = quit_own(step, shim, values, state)
            if note:
                left.append(note)
        elif kind == "close_windows":
            for title in step["titles"]:
                found = matching_windows(shim, step.get("app"), title)
                if found is None:
                    left.append(f"could not list the windows to close {title}")
                    continue
                for window in found:
                    shim(["window-close", "--id", str(window["id"])])
    remove_work(work, work_root)
    return left


def remove_work(work, work_root):
    real, fence = os.path.realpath(work), os.path.realpath(work_root)
    if real != fence and real.startswith(fence + os.sep) and os.path.isdir(real):
        shutil.rmtree(real, ignore_errors=True)


def claim(last_message, pattern):
    """The last RESULT: line of the model's last message, or None."""
    found = None
    for line in (last_message or "").splitlines():
        match = re.match(pattern, line.strip())
        if match:
            found = match.group(1)
    return found


def dump(value):
    return json.dumps(value, ensure_ascii=False, indent=2) + "\n"
