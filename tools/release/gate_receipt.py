#!/usr/bin/env python3
"""Execute one gate and attest it through the window's authenticated coordinator.

`run` is an execution, not a dry run. `verify` only hashes observations and asks the live ledger.
An old window without the three native gate verbs produces a miss, never a file-based fallback.
The root coordinator supplies independently pinned launcher and manifest bytes.
Candidate declarations never establish producer or recipe provenance. The
cache covers declared inputs; it does not isolate arbitrary same-user code.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import threading
import time
import types
import uuid

GIT_EXECUTABLE = shutil.which("git", path=os.defpath)
GIT_ENVIRONMENT = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
GIT_GUARD = None
MAX_METADATA = 16 * 1024
MAX_LOG = 64 * 1024 * 1024
SUMMARY = re.compile(
    r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
    r"\d+ measured; \d+ filtered out;(?: finished in .*s)?$", re.MULTILINE
)


class UnknownInput(Exception):
    """An observation cannot establish a stable identity."""


class ExecutionUnattested(UnknownInput):
    """The command ran but no authenticated completion was acknowledged."""


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    path = Path(path)
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode):
        raise UnknownInput("input is missing, linked or not a regular file")
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(block)
    after = path.lstat()
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns, before.st_mode) != (
        after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns, after.st_mode
    ):
        raise UnknownInput("input changed while hashing")
    return hasher.hexdigest()


def check_link_cycles(links):
    complete = set()

    def visit(name, active):
        if name in active:
            raise UnknownInput("reachable directory links form a cycle")
        if name in complete:
            return
        target, directory = links[name]
        if directory:
            children = [value for value in links if target == "." or value.startswith(target + "/")]
            for child in children:
                visit(child, active | {name})
        complete.add(name)

    for name in links:
        visit(name, set())


def paths_digest(paths, excluded=()):
    """Hash file contents, relative names and modes. Unknown links fail closed.

    The root's identity is hashed too: an external path can be a command input. Root paths and
    file names never appear in stored metadata. Empty or missing root lists remain unknown.
    """
    if not paths:
        return None
    omitted = [Path(value).absolute() for value in excluded]
    roots = []
    for name in sorted(set(str(Path(path).absolute()) for path in paths)):
        root = Path(name)
        if any(root.is_relative_to(value) for value in omitted):
            raise UnknownInput("an output declaration hides an input root")
        if root.is_symlink() or not root.exists():
            raise UnknownInput("input root is missing or linked")
        entries = []
        links = {}
        files = [root]
        if root.is_dir():
            for directory, children, leaves in os.walk(root, followlinks=False):
                children[:] = sorted(name for name in children if not any((Path(directory) / name).is_relative_to(value) for value in omitted))
                files.extend(Path(directory) / name for name in [*children, *sorted(leaves)])
        for path in files:
            if any(path.is_relative_to(value) for value in omitted):
                continue
            if path.name.lower() in {".env", ".netrc", ".npmrc", ".pypirc", "credentials", "credentials.toml", "id_rsa", "id_ed25519"}:
                raise UnknownInput("credential files are outside the declared public closure")
            mode = path.lstat().st_mode
            relative = path.relative_to(root).as_posix() if path != root else "."
            if stat.S_ISDIR(mode):
                entries.append([relative, stat.S_IMODE(mode), "directory"])
            elif stat.S_ISLNK(mode):
                try:
                    target = path.resolve(strict=True)
                except (OSError, RuntimeError) as why:
                    raise UnknownInput("declared link is dangling or cyclic") from why
                if not target.is_relative_to(root):
                    raise UnknownInput("declared link escapes its input root")
                if any(target.is_relative_to(value) for value in omitted):
                    raise UnknownInput("an input link reaches excluded cache output")
                target_name = target.relative_to(root).as_posix()
                links[relative] = (target_name, target.is_dir())
                entries.append([relative, stat.S_IMODE(mode), "link", os.readlink(path), target_name])
            else:
                entries.append([relative, stat.S_IMODE(mode), file_digest(path)])
        if not entries:
            raise UnknownInput("empty dependency or toolchain input root")
        check_link_cycles(links)
        roots.append([name, entries])
    return digest(encoded(roots))


def observed(call):
    try:
        return call()
    except (OSError, UnknownInput, subprocess.SubprocessError, ValueError, RuntimeError, TypeError, KeyError):
        return None


def git(repo, *args):
    if GIT_GUARD is not None:
        GIT_GUARD(repo)
    result = subprocess.run(
        [GIT_EXECUTABLE, "-c", "core.fsmonitor=false", "-C", str(repo), *args], check=True, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, timeout=30, env={
            **GIT_ENVIRONMENT,
            "GIT_OPTIONAL_LOCKS": "0", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_SYSTEM": os.devnull, "GIT_CONFIG_COUNT": "0",
        },
    )
    return result.stdout


def source_identity(repo):
    repo = Path(repo).resolve()
    head = git(repo, "rev-parse", "--verify", "HEAD").decode().strip()
    tree = git(repo, "rev-parse", "--verify", head + "^{tree}").decode().strip()
    if git(repo, "status", "--porcelain=v1", "--untracked-files=all"):
        raise UnknownInput("source checkout is dirty")
    tracked = {}
    for raw in git(repo, "ls-tree", "-rz", head).split(b"\0"):
        if raw:
            fields, name = raw.split(b"\t", 1)
            mode, kind, oid = fields.split()
            if kind != b"blob" or mode not in (b"100644", b"100755", b"120000"):
                raise UnknownInput("source has an undeclared submodule input")
            tracked[os.fsdecode(name)] = (mode.decode(), oid.decode())
    entries = []
    links = {}
    for name, (mode, oid) in sorted(tracked.items()):
        path = repo / name
        before = path.lstat()
        if mode == "120000":
            if not stat.S_ISLNK(before.st_mode):
                raise UnknownInput("tracked link mode changed")
            data = os.fsencode(os.readlink(path))
            try:
                target = path.resolve(strict=True)
            except (OSError, RuntimeError) as why:
                raise UnknownInput("tracked link is dangling or cyclic") from why
            if not target.is_relative_to(repo):
                raise UnknownInput("tracked link escapes the source root")
            target_name = target.relative_to(repo).as_posix()
            if target_name not in tracked and not any(value.startswith(target_name + "/") for value in tracked):
                raise UnknownInput("tracked link reaches an undeclared input")
            closure = target_name
            links[name] = (target_name, target.is_dir())
        else:
            if not stat.S_ISREG(before.st_mode) or bool(before.st_mode & 0o111) != (mode == "100755"):
                raise UnknownInput("tracked source mode changed")
            data = path.read_bytes()
            closure = None
        after = path.lstat()
        identity = lambda item: (item.st_dev, item.st_ino, item.st_mode, item.st_size, item.st_mtime_ns, item.st_ctime_ns)
        if identity(before) != identity(after):
            raise UnknownInput("source changed during the snapshot")
        algorithm = hashlib.sha1 if len(oid) == 40 else hashlib.sha256
        if algorithm(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() != oid:
            raise UnknownInput("source bytes differ from the frozen tree")
        entries.append([name, mode, digest(data), closure])
    check_link_cycles(links)
    if not entries or head != git(repo, "rev-parse", "HEAD").decode().strip():
        raise UnknownInput("source moved while hashing")
    if git(repo, "status", "--porcelain=v1", "--untracked-files=all"):
        raise UnknownInput("source changed during the snapshot")
    return head, tree, digest(encoded(entries))


def command_executable(repo, word, environment):
    """Resolve exactly as a child whose working directory is the repository would resolve it."""
    if os.path.dirname(word):
        path = Path(word)
        path = path if path.is_absolute() else repo / path
        return path.resolve() if path.is_file() and os.access(path, os.X_OK) else None
    search = os.pathsep.join(
        str(Path(part) if Path(part).is_absolute() else repo / part)
        for part in environment.get("PATH", os.defpath).split(os.pathsep)
    )
    found = shutil.which(word, path=search)
    return Path(found).resolve() if found else None


def execution_environment(args):
    """Give every child explicit values, without the parent's ledger or account authority."""
    trust = getattr(args, "trust", None)
    if trust:
        return dict(trust["environment"])
    executable = command_executable(Path(args.repo).resolve(), args.command[0], {"PATH": os.defpath})
    environment = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C", "TZ": "UTC"}
    if executable:
        environment["PATH"] = str(executable.parent) + os.pathsep + os.defpath
    home = getattr(args, "child_home", None)
    if home:
        environment.update(HOME=home, USERPROFILE=home, CARGO_HOME=str(Path(home) / "cargo"),
                           RUSTUP_HOME=str(Path(home) / "rustup"), XDG_CONFIG_HOME=str(Path(home) / "config"))
    return environment



def public_command(command):
    return not any(re.search(r"password|passwd|secret|token|cookie|authorization|api.?key", word, re.IGNORECASE)
                   for word in command)


def provenance(args):
    trust = getattr(args, "trust", None)
    if not trust or not callable(trust.get("configure")):
        raise UnknownInput("independently pinned launcher and trust manifest are required")
    repo = Path(args.repo).resolve()
    if repo != Path(trust["repo"]) or Path(__file__).resolve().is_relative_to(repo):
        raise UnknownInput("candidate-contained or mismatched producer")
    bundle = trust["policy"]["bundle"]
    if Path(bundle["producer"]["path"]).resolve() != Path(__file__).resolve():
        raise UnknownInput("producer invocation differs from the independent pin")
    if digest(Path(trust["policyPath"]).read_bytes()) != trust["policySha256"]:
        raise UnknownInput("trusted manifest changed")
    if digest(trust["policyBytes"]) != trust["policySha256"] or encoded(json.loads(trust["policyBytes"])) != encoded(trust["policy"]):
        raise UnknownInput("loaded manifest differs from the independent pin")
    for role, entry in bundle.items():
        path = Path(entry["path"])
        if path.resolve().is_relative_to(repo) or file_digest(path) != entry["sha256"]:
            raise UnknownInput("trusted bundle changed")
        if digest(trust["bytes"][role]) != entry["sha256"]:
            raise UnknownInput("loaded trusted bytes changed")
    return trust["configure"](args)


def fingerprint(args, environment, command):
    started = time.monotonic_ns()
    try:
        return fingerprint_inputs(args, environment, command)
    finally:
        if isinstance(getattr(args, "trust", None), dict):
            args.trust.setdefault("fingerprintMs", []).append((time.monotonic_ns() - started) // 1_000_000)


def fingerprint_inputs(args, environment, command):
    """Capture all dimensions. Unknown closure or unreadable inputs remain explicit nulls."""
    repo = Path(args.repo).resolve()
    trust = observed(lambda: provenance(args))
    source = observed(lambda: source_identity(repo))
    head = source[0] if source else git(repo, "rev-parse", "--verify", "HEAD").decode().strip()
    locks = [repo / path for path in args.lock_file]
    if not locks:
        locks = [path for path in (repo / "Cargo.lock", repo / "zo-ide/Cargo.lock") if path.exists()]
    scripts = [Path(entry["path"]) for entry in trust["policy"]["bundle"].values()] if trust else []
    scripts += [repo / path for path in args.script_file]
    executable = command_executable(repo, command[0], environment)
    tools = [Path(sys.executable).resolve()]
    if executable:
        tools.append(executable)
    declared = args.closure == "declared" and trust is not None
    handed_in = getattr(args, "handed_in_head", None)
    ancestor = observed(lambda: git(repo, "merge-base", "--is-ancestor", handed_in, head) == b"") if handed_in else False
    return {
        "version": 1, "gate": args.gate, "head": head,
        "tree": source[1] if source else None, "source": source[2] if source else None,
        "deps": observed(lambda: digest(encoded([paths_digest(args.dependency_root), trust["configurationIdentities"],
                                                 trust["outputBindings"], trust["ignoredBindings"]]))) if declared else None,
        "lock": observed(lambda: paths_digest(locks)),
        "script": observed(lambda: paths_digest(scripts)) if declared else None,
        "recipe": digest(encoded(command)) if declared and public_command(command) else None,
        "toolchain": observed(lambda: digest(encoded([paths_digest([*args.toolchain_root, *tools]), trust["pathBindings"]])))
        if declared and args.toolchain_root and executable else None,
        "platform": digest(encoded([platform.system(), platform.release(), platform.version(), platform.machine()])),
        "profile": digest(encoded([args.profile, args.closure])) if args.profile else None,
        "env": digest(encoded(sorted(environment.items())))
        if declared and environment == trust["environment"] else None,
        "policy": trust["policySha256"] if trust else None,
        "producer": digest(encoded({role: entry["sha256"] for role, entry in trust["policy"]["bundle"].items()})) if trust else None,
        "handedInHead": handed_in, "sourceAncestor": ancestor is True, "scope": args.scope,
    }


def test_count(log, recipes=(), test_recipes=None):
    """Count complete libtest, node TAP/spec and unittest runs; incomplete suites give zero."""
    text = log.decode("utf-8", errors="replace").replace("\r\n", "\n")
    lines = text.splitlines()
    total = 0
    libtest = []
    node_headers = 0
    node_summaries = 0
    node_active = False
    node_plan = False
    spec_pending = False
    ui_checks = ui_passes = 0
    ui_pending = False
    unittest_headers = 0
    index = 0
    while index < len(lines):
        line = lines[index]
        ui_result = re.fullmatch(r"(PASS|FAIL)  (\S.*)", line)
        ui_footer = re.fullmatch(r"(\d+)/(\d+) passed", line)
        if ui_result:
            ui_pending = True
            ui_checks += 1
            if ui_result[1] != "PASS":
                return 0
            ui_passes += 1
        elif ui_footer:
            passed, checks = map(int, ui_footer.groups())
            if checks == 0 or passed != checks or passed != ui_passes or checks != ui_checks:
                return 0
            total += passed
            ui_checks = ui_passes = 0
            ui_pending = False
        elif re.fullmatch(r"SUITE  \S.*", line):
            ui_pending = True
        elif re.match(r"^(?:PASS|FAIL|SUITE)\s|^[+-]?\d+/[+-]?\d+\s+pass", line):
            return 0
        elif "test result:" in line:
            row = SUMMARY.fullmatch(line)
            if row is None or row[1] != "ok" or int(row[3]):
                return 0
            if re.search(r"; [1-9]\d* measured;", line):
                return 0
            libtest.append(int(row[2]) + int(row[4]))
            total += int(row[2])
        elif line == "TAP version 13":
            if node_active:
                return 0
            node_headers += 1
            node_active, node_plan = True, False
        elif line.startswith("TAP version"):
            return 0
        elif re.fullmatch(r"1\.\.\d+", line):
            if node_active:
                node_plan = True
        elif re.match(r"^\s*[✔✖] .+ \(\d+(?:\.\d+)?ms\)$", line):
            spec_pending = True
        elif re.fullmatch(r"(?:#|ℹ) tests \d+", line):
            prefix = line[0]
            values = {}
            names = ("tests", "suites", "pass", "fail", "cancelled", "skipped", "todo", "duration_ms")
            for name in names:
                if index >= len(lines):
                    return 0
                match = re.fullmatch(re.escape(prefix) + r" " + name + r" (\d+(?:\.\d+)?)", lines[index])
                if match is None or name != "duration_ms" and "." in match[1]:
                    return 0
                values[name] = float(match[1]) if name == "duration_ms" else int(match[1])
                index += 1
            if any(values[name] for name in ("fail", "cancelled", "skipped", "todo")):
                return 0
            if values["tests"] == 0 or values["tests"] != values["pass"]:
                return 0
            if prefix == "#":
                node_summaries += 1
                if not node_active or not node_plan:
                    return 0
                node_active = False
            else:
                spec_pending = False
            total += values["pass"]
            continue
        elif re.fullmatch(r"Ran \d+ tests? in \d+(?:\.\d+)?s", line):
            unittest_headers += 1
            count = int(line.split()[1])
            if count == 0:
                return 0
            index += 1
            while index < len(lines) and not lines[index]:
                index += 1
            if index >= len(lines) or not re.fullmatch(r"OK(?: \(skipped=0\))?", lines[index]):
                return 0
            total += count
        elif re.match(r"(?:#|ℹ) (?:pass|fail|cancelled|skipped|todo|suites|duration_ms)\b", line):
            return 0
        elif re.match(r"Ran \d|test result:|not ok\b|\s+not ok\b|FAILED(?:\b|\s*\()|failures:", line):
            return 0
        elif re.match(r"^(?:SKIPPED\b|timed[ _]out\b|cancelled\b)", line, re.IGNORECASE):
            return 0
        elif re.match(r"error: test failed|verify: red recipes:|\d+ (?:passed|failed),", line):
            return 0
        index += 1
    running = [int(value) for value in re.findall(r"^running (\d+) tests?$", text, re.MULTILINE)]
    if running and running != libtest:
        return 0
    if node_headers != node_summaries or spec_pending or ui_pending:
        return 0
    if re.search(r"^OK(?:\s|$)", text, re.MULTILINE) and not unittest_headers:
        return 0
    begins = re.findall(r"^==> verify recipe (\S+)$", text, re.MULTILINE)
    ends = re.findall(r"^<== verify recipe (\S+) rc=(\d+) \d+s$", text, re.MULTILINE)
    if begins != [recipe for recipe, _ in ends] or any(rc != "0" for _, rc in ends):
        return 0
    if recipes and begins != list(recipes):
        return 0
    if recipes:
        required_tests = set(recipes if test_recipes is None else test_recipes)
        if not required_tests or not required_tests.issubset(recipes):
            return 0
        active = None
        counts = {}
        for position, line in enumerate(lines):
            begin = re.fullmatch(r"==> verify recipe (\S+)", line)
            end = re.fullmatch(r"<== verify recipe (\S+) rc=0 \d+s", line)
            if begin:
                if active is not None:
                    return 0
                active = (begin[1], position + 1)
            elif end:
                if active is None or active[0] != end[1]:
                    return 0
                counts[end[1]] = test_count(("\n".join(lines[active[1]:position]) + "\n").encode())
                active = None
        if active is not None or any(counts.get(name, 0) == 0 for name in required_tests) or sum(counts.values()) != total:
            return 0
    return total


def credential_environment_name(name):
    separated = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    separated = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", separated)
    words = set(re.findall(r"[A-Z0-9]+", separated.upper()))
    sensitive = {"TOKEN", "PASSWORD", "PASSWD", "SECRET", "KEY", "COOKIE", "AUTH",
                 "AUTHORIZATION", "AUTHENTICATION", "CREDENTIAL", "CREDENTIALS"}
    compact = {"APIKEY", "ACCESSKEY", "PRIVATEKEY", "SECRETKEY", "SIGNINGKEY", "ENCRYPTIONKEY",
               "AUTHTOKEN", "ACCESSTOKEN", "REFRESHTOKEN", "BEARERTOKEN", "CLIENTSECRET", "SESSIONCOOKIE"}
    endings = compact | {"TOKEN", "PASSWORD", "PASSWD", "SECRET", "COOKIE", "AUTHORIZATION", "CREDENTIAL", "CREDENTIALS"}
    return any(word in sensitive or any(word.endswith(ending) for ending in endings) for word in words)


def mask_log(log, environment, trust):
    """Use the release scanner's shared rules and known secret environment values."""
    scanner = types.ModuleType("gate_pii_scan")
    scanner.__file__ = trust["policy"]["bundle"]["scanner"]["path"]
    scanner.VERIFIED_RULES_BYTES = trust["bytes"]["rules"]
    exec(compile(trust["bytes"]["scanner"], scanner.__file__, "exec"), scanner.__dict__)
    text = log.decode("utf-8", errors="replace")
    for key, value in sorted(environment.items(), key=lambda pair: len(pair[1]), reverse=True):
        if value and credential_environment_name(key):
            text = text.replace(value, "[private]")
    for rule in scanner.RULES:
        text = re.sub(rule["find"], "[private]", text)
    banned = set(scanner.BANISHED)
    text = scanner.TOKEN.sub(
        lambda match: "[private]" if (len(match[0]), digest(match[0].lower().encode())) in banned else match[0], text
    )
    return text.encode()


class Ledger:
    """Only a live window call can authenticate an attestation; no file/export reader exists."""

    def __init__(self, authority=None):
        if authority is None or authority.fileno() < 3:
            raise UnknownInput("a native gate job connection is required")
        self.authority = authority

    def read_exact(self, count):
        result = bytearray()
        while len(result) < count:
            block = self.authority.recv(count - len(result))
            if not block:
                raise UnknownInput("native gate job connection closed")
            result.extend(block)
        return bytes(result)

    def call(self, verb, args, value, retry, extra=()):
        request = {"verb": verb, "body": value, "retryRequest": retry}
        for index in range(0, len(extra), 2):
            key = {"--start": "start", "--cancel": "cancel", "--lease": "lease"}.get(extra[index])
            if key is None or index + 1 >= len(extra):
                raise UnknownInput("unsupported native gate record mode")
            request[key] = extra[index + 1]
        body = encoded(request)
        if len(body) > 32 * 1024:
            raise UnknownInput("native gate job request is too large")
        self.authority.sendall(len(body).to_bytes(4, "big") + body)
        length = int.from_bytes(self.read_exact(4), "big")
        if not 0 < length <= 32 * 1024:
            raise UnknownInput("native gate job reply is too large")
        reply = json.loads(self.read_exact(length))
        if not isinstance(reply, dict) or reply.get("ok") is not True:
            raise UnknownInput("native gate job refused this operation")
        answer = reply.get("value")
        if not isinstance(answer, dict):
            raise UnknownInput("native ledger returned an unsupported answer")
        return answer


def write_new(path, data):
    """Never replace an original artifact, or follow a pre-existing link."""
    with Path(path).open("xb") as target:
        target.write(data)


def stop(child):
    try:
        if os.name == "posix":
            os.killpg(child.pid, signal.SIGKILL)
        else:
            # An interrupted Windows execution is never reusable, even if only its parent can stop.
            child.kill()
    except ProcessLookupError:
        pass


def execute(command, repo, environment, timeout):
    started_ms = time.time_ns() // 1_000_000
    started = time.monotonic_ns()
    child = subprocess.Popen(
        command, cwd=repo, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL, start_new_session=os.name == "posix", close_fds=True,
    )
    outcome = "passed"
    captured = bytearray()
    overflow = threading.Event()
    read_error = threading.Event()

    def drain():
        try:
            with child.stdout:
                while block := child.stdout.read(64 * 1024):
                    room = MAX_LOG - len(captured)
                    captured.extend(block[:room])
                    if len(block) > room:
                        overflow.set()
        except OSError:
            read_error.set()

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    try:
        while child.poll() is None:
            if overflow.is_set():
                outcome = "unknown"
                break
            if (time.monotonic_ns() - started) / 1_000_000_000 >= timeout:
                outcome = "timed_out"
                break
            time.sleep(0.02)
    except KeyboardInterrupt:
        outcome = "cancelled"
    if outcome != "passed":
        stop(child)
    child.wait()
    reader.join(timeout=1)
    if reader.is_alive() or read_error.is_set() or overflow.is_set():
        stop(child)
        outcome = "unknown"
        reader.join(timeout=1)
    if child.returncode != 0 and outcome == "passed":
        outcome = "failed"
    return {
        "startedMs": started_ms, "endedMs": time.time_ns() // 1_000_000,
        "durationMs": (time.monotonic_ns() - started) // 1_000_000,
        "rc": child.returncode, "outcome": outcome,
    }, bytes(captured)


def produce(args, ledger=None):
    wall_started = time.monotonic_ns()
    command = args.command
    trust = provenance(args)
    environment = execution_environment(args)
    before = fingerprint(args, environment, command)
    ledger = ledger or Ledger(trust.get("authority"))
    nonce = uuid.uuid4().hex
    registered = ledger.call("gate-register", args, before, f"gate-register-{nonce}")
    registration = registered.get("registration")
    policy_pin = registered.get("policyPin")
    lease = registered.get("lease")
    if not isinstance(registration, str) or not isinstance(policy_pin, str) or not isinstance(lease, str) or not isinstance(registered.get("registeredMs"), int):
        raise UnknownInput("native gate registration was not acknowledged")
    if time.time_ns() // 1_000_000 < registered["registeredMs"]:
        raise UnknownInput("clock precedes the registration event")
    out = Path(args.output).resolve()
    if out.is_relative_to(Path(args.repo).resolve()):
        raise UnknownInput("gate artifacts must be outside the source checkout")
    out.mkdir(parents=True, exist_ok=False)
    just_before = fingerprint(args, environment, command)
    started = ledger.call("gate-record", args, just_before, f"gate-start-{nonce}",
                          extra=("--start", registration, "--lease", lease))
    if not isinstance(started.get("execution"), str) or not isinstance(started.get("startedMs"), int):
        raise UnknownInput("native execution start was not acknowledged")
    if time.time_ns() // 1_000_000 < started["startedMs"]:
        raise UnknownInput("clock precedes the native execution start")
    observed_end, raw_log = execute(command, args.repo, environment, args.timeout)
    try:
        after = observed(lambda: fingerprint(args, environment, command))
        log = mask_log(raw_log, {**os.environ, **environment}, trust)
        recipes = trust["selected"]["recipes"]
        raw_count, masked_count = test_count(raw_log, recipes, trust["selected"]["testRecipes"]), test_count(log, recipes, trust["selected"]["testRecipes"])
        if observed_end["outcome"] == "passed" and (raw_count == 0 or raw_count != masked_count or before != after):
            observed_end["outcome"] = "unknown"
        metadata = {
            "version": 1, "registration": registration, "policyPin": policy_pin, "lease": lease, "execution": started["execution"],
            "attempt": args.attempt, "inputs": before,
            **observed_end, "tests": raw_count if raw_count == masked_count else 0,
            "logSha256": digest(log), "logBytes": len(log),
            "inputsUnchanged": before == after, "executed": True,
        }
        body = encoded(metadata)
        if len(body) > MAX_METADATA:
            raise UnknownInput("gate metadata exceeds its native size limit")
        write_new(out / "gate.log", log)
        write_new(out / "metadata.json", body)
        completed = ledger.call("gate-record", args, metadata, f"gate-record-{nonce}")
        if completed.get("metadataSha256") != digest(body) or not isinstance(completed.get("original"), str):
            raise UnknownInput("native ledger did not attest these metadata bytes")
    except (OSError, UnknownInput, subprocess.SubprocessError, ValueError) as why:
        raise ExecutionUnattested("real execution has no acknowledged completion") from why
    return {"state": "recorded", "original": completed["original"], "registration": registration,
            "metadataSha256": digest(body), "executed": True, "newExecutions": 1,
            "outcome": metadata["outcome"], "tests": metadata["tests"],
            "timings": {"fingerprintMs": trust.get("fingerprintMs", []), "wallMs": (time.monotonic_ns() - wall_started) // 1_000_000}}


def verify(args, ledger=None):
    wall_started = time.monotonic_ns()
    trust = provenance(args)
    # A file supplies only the registration locator and observed bytes. The ledger supplies trust.
    metadata_path = Path(args.metadata)
    file_digest(metadata_path)  # Refuse links and changing files before interpreting bytes.
    raw = metadata_path.read_bytes()
    if len(raw) > MAX_METADATA:
        raise UnknownInput("gate metadata exceeds its native size limit")
    metadata = json.loads(raw)
    if not isinstance(metadata, dict) or not isinstance(metadata.get("registration"), str):
        raise UnknownInput("gate metadata has no original registration")
    log_path = Path(args.log)
    if log_path.stat().st_size > MAX_LOG:
        raise UnknownInput("gate log exceeds its read limit")
    file_digest(log_path)
    log = log_path.read_bytes()
    current = fingerprint(args, execution_environment(args), args.command)
    request = {
        "version": 1, "registration": metadata["registration"], "lease": metadata.get("lease", "unknown"), "inputs": current,
        "metadataSha256": digest(raw), "logSha256": digest(log), "logBytes": len(log),
        "tests": test_count(log, trust["selected"]["recipes"], trust["selected"]["testRecipes"]),
    }
    answer = (ledger or Ledger(trust.get("authority"))).call("gate-reuse", args, request, f"gate-reuse-{uuid.uuid4().hex}")
    if answer.get("executed") is not False or answer.get("newExecutions") != 0:
        raise UnknownInput("unsupported native reuse decision")
    if answer.get("decision") not in ("hit", "miss") or not isinstance(answer.get("decisionRecord"), str):
        raise UnknownInput("native ledger did not record a reuse decision")
    if answer.get("decision") == "hit" and (
        answer.get("gate") not in ("gate-root", "gate-zo")
        or answer.get("mandatoryChecks") != ["native", "windows", "flake-judgment", "publish"]
    ):
        raise UnknownInput("native decision has an unsupported reuse scope")
    answer["timings"] = {"fingerprintMs": trust.get("fingerprintMs", []), "wallMs": (time.monotonic_ns() - wall_started) // 1_000_000}
    return answer


def main(trust=None, argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("action", choices=("run", "verify"))
    parser.add_argument("--repo", required=True)
    parser.add_argument("--gate", choices=("gate-root", "gate-zo"), required=True)
    parser.add_argument("--task", required=True)
    parser.add_argument("--attempt", required=True)
    parser.add_argument("--handed-in-head")
    parser.add_argument("--scope", choices=("unknown", "deterministic", "measurement", "nondeterministic"), default="unknown")
    parser.add_argument("--closure", choices=("unknown", "declared"), default="unknown")
    parser.add_argument("--clean-env", action="store_true", help="compatibility flag; every child uses only the trusted explicit environment")
    parser.add_argument("--profile")
    parser.add_argument("--dependency-root", action="append", default=[])
    parser.add_argument("--toolchain-root", action="append", default=[])
    parser.add_argument("--lock-file", action="append", default=[])
    parser.add_argument("--script-file", action="append", default=[])
    parser.add_argument("--output")
    parser.add_argument("--metadata")
    parser.add_argument("--log")
    parser.add_argument("--timeout", type=float, default=1800)
    # Split the child argv explicitly: REMAINDER can consume this tool's options after `action`.
    arguments = sys.argv[1:] if argv is None else argv
    if "--" not in arguments:
        parser.error("separate the gate command with --")
    split = arguments.index("--")
    args = parser.parse_args(arguments[:split])
    args.command = arguments[split + 1:]
    if not args.command or not 0 < args.timeout <= 86400:
        parser.error("one command and a finite positive timeout are required")
    if args.action == "run" and not args.output or args.action == "verify" and not (args.metadata and args.log):
        parser.error("run needs --output; verify needs --metadata and --log")
    try:
        if trust is None:
            raise UnknownInput("use the independently pinned external launcher")
        args.trust = trust
        provenance(args)
        answer = produce(args) if args.action == "run" else verify(args)
    except ExecutionUnattested:
        answer = {"state": "unattested", "reason": "execution has no authenticated completion",
                  "executed": True, "newExecutions": 1}
    except (OSError, UnknownInput, subprocess.SubprocessError, ValueError):
        # Exception text may contain a private path, an argument or an environment value.
        answer = {"decision": "miss", "reason": "input or authenticated ledger observation unavailable",
                  "executed": False, "newExecutions": 0}
    print(json.dumps(answer, sort_keys=True))
    return 0 if answer.get("decision") == "hit" or answer.get("state") == "recorded" and answer.get("outcome") == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
