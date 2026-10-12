#!/usr/bin/env python3
"""Execute one gate and attest it through the window's authenticated coordinator.

`run` is an execution, not a dry run. `verify` only hashes observations and asks the live ledger.
An old window without the three native gate verbs produces a miss, never a file-based fallback.
The default scope and input closure are unknown. A coordinator must establish a deterministic,
complete input closure before selecting `--scope deterministic --closure declared`.
"""

import argparse
import hashlib
import importlib.util
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
import uuid

HERE = Path(__file__).resolve().parent
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
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


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
    if (before.st_ino, before.st_size, before.st_mtime_ns, before.st_mode) != (
        after.st_ino, after.st_size, after.st_mtime_ns, after.st_mode
    ):
        raise UnknownInput("input changed while hashing")
    return hasher.hexdigest()


def paths_digest(paths):
    """Hash file contents, relative names and modes. Unknown links fail closed.

    The root's identity is hashed too: an external path can be a command input. Root paths and
    file names never appear in stored metadata. Empty or missing root lists remain unknown.
    """
    if not paths:
        return None
    roots = []
    for name in sorted(set(str(Path(path).absolute()) for path in paths)):
        root = Path(name)
        if root.is_symlink() or not root.exists():
            raise UnknownInput("input root is missing or linked")
        entries = []
        files = sorted(root.rglob("*")) if root.is_dir() else [root]
        for path in files:
            mode = path.lstat().st_mode
            relative = str(path.relative_to(root)) if path != root else "."
            if stat.S_ISDIR(mode):
                entries.append([relative, stat.S_IMODE(mode), "directory"])
            else:
                entries.append([relative, stat.S_IMODE(mode), file_digest(path)])
        if not entries:
            raise UnknownInput("empty dependency or toolchain input root")
        roots.append([name, entries])
    return digest(encoded(roots))


def observed(call):
    try:
        return call()
    except (OSError, UnknownInput, subprocess.SubprocessError, ValueError):
        return None


def git(repo, *args):
    result = subprocess.run(
        ["git", "-c", "core.fsmonitor=false", "-C", str(repo), *args], check=True, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, timeout=30, env={
            "PATH": os.environ.get("PATH", os.defpath), "LANG": "C", "LC_ALL": "C",
            "GIT_OPTIONAL_LOCKS": "0", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        },
    )
    return result.stdout


def source_identity(repo):
    head = git(repo, "rev-parse", "--verify", "HEAD").decode().strip()
    tree = git(repo, "rev-parse", "--verify", "HEAD^{tree}").decode().strip()
    if git(repo, "status", "--porcelain=v1", "--untracked-files=all"):
        raise UnknownInput("source checkout is dirty")
    entries = []
    for raw in git(repo, "ls-files", "--stage", "-z").split(b"\0"):
        if not raw:
            continue
        mode_oid_stage, name = raw.split(b"\t", 1)
        mode, _, stage = mode_oid_stage.split()
        # Submodules and symlinks have inputs beyond this tracked tree.
        if mode not in (b"100644", b"100755") or stage != b"0":
            raise UnknownInput("source includes an unresolved external or merged input")
        path = repo / os.fsdecode(name)
        entries.append([os.fsdecode(name), mode.decode(), file_digest(path)])
    if not entries or head != git(repo, "rev-parse", "HEAD").decode().strip():
        raise UnknownInput("source moved while hashing")
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
    """Inherited values may be low-entropy secrets: never persist a digest of that environment."""
    if not args.clean_env:
        return dict(os.environ)
    executable = command_executable(Path(args.repo).resolve(), args.command[0], os.environ)
    environment = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C", "TZ": "UTC"}
    if executable:
        environment["PATH"] = str(executable.parent) + os.pathsep + os.defpath
    if os.name == "nt" and os.environ.get("SystemRoot"):
        environment["SystemRoot"] = os.environ["SystemRoot"]
    return environment


def public_environment(environment):
    if set(environment) - {"PATH", "LANG", "LC_ALL", "TZ", "SystemRoot"}:
        return False
    if any(environment.get(key) != value for key, value in (("LANG", "C"), ("LC_ALL", "C"), ("TZ", "UTC"))):
        return False
    # These are filesystem search roots, not free-form strings or URLs carrying credentials.
    if "SystemRoot" in environment and not Path(environment["SystemRoot"]).is_dir():
        return False
    return all(Path(part).is_absolute() and not re.search(r"[\r\n=@]", part)
               for part in environment.get("PATH", "").split(os.pathsep))


def public_command(command):
    return not any(re.search(r"password|passwd|secret|token|cookie|authorization|api.?key", word, re.IGNORECASE)
                   for word in command)


def fingerprint(args, environment, command):
    """Capture all dimensions. Unknown closure or unreadable inputs remain explicit nulls."""
    repo = Path(args.repo).resolve()
    # A full HEAD is needed even for an unknown source; the native ledger checks it independently.
    head = git(repo, "rev-parse", "--verify", "HEAD").decode().strip()
    source = observed(lambda: source_identity(repo))
    locks = [repo / path for path in args.lock_file]
    if not locks:
        locks = [path for path in (repo / "Cargo.lock", repo / "zo-ide/Cargo.lock") if path.exists()]
    scripts = [Path(__file__).resolve(), HERE / "pii-scan.py", HERE / "pii-rules.json"]
    scripts += [repo / path for path in args.script_file]
    executable = command_executable(repo, command[0], environment)
    tools = [Path(sys.executable).resolve()]
    if executable:
        tools.append(executable)
    declared = args.closure == "declared"
    return {
        "version": 1, "gate": args.gate, "head": head,
        "tree": source[1] if source else None, "source": source[2] if source else None,
        "deps": observed(lambda: paths_digest(args.dependency_root)) if declared else None,
        "lock": observed(lambda: paths_digest(locks)),
        "script": observed(lambda: paths_digest(scripts)) if declared else None,
        "recipe": digest(encoded(command)) if declared and public_command(command) else None,
        "toolchain": observed(lambda: paths_digest([*args.toolchain_root, *tools]))
        if args.toolchain_root and executable else None,
        "platform": digest(encoded([platform.system(), platform.release(), platform.version(), platform.machine()])),
        "profile": digest(encoded([args.profile, args.closure])) if args.profile else None,
        "env": digest(encoded(sorted(environment.items())))
        if args.clean_env and declared and public_environment(environment) else None, "scope": args.scope,
    }


def test_count(log):
    """Only complete passing libtest summaries establish a count; unsupported logs give zero."""
    text = log.decode("utf-8", errors="replace").replace("\r\n", "\n")
    summary_lines = [line for line in text.splitlines() if "test result:" in line]
    summaries = [SUMMARY.fullmatch(line) for line in summary_lines]
    if any(row is None for row in summaries):
        return 0
    if not summaries or any(row[1] != "ok" or int(row[3]) for row in summaries):
        return 0
    # A skipped, truncated or measured suite is never a reusable test execution.
    if re.search(r"\b(?:SKIPPED|timed[ _]out|cancelled)\b", text, re.IGNORECASE):
        return 0
    if re.search(r"^error: test failed|^failures:", text, re.MULTILINE):
        return 0
    begins = re.findall(r"^==> verify recipe (\S+)$", text, re.MULTILINE)
    ends = re.findall(r"^<== verify recipe (\S+) rc=(\d+) \d+s$", text, re.MULTILINE)
    if (begins or ends) and (begins != [recipe for recipe, _ in ends] or any(rc != "0" for _, rc in ends)):
        return 0
    if any(re.search(r"; [1-9]\d* measured;", row[0]) for row in summaries):
        return 0
    return sum(int(row[2]) for row in summaries)


def mask_log(log, environment):
    """Use the release scanner's shared rules and known secret environment values."""
    spec = importlib.util.spec_from_file_location("gate_pii_scan", HERE / "pii-scan.py")
    scanner = importlib.util.module_from_spec(spec)
    # Read the measured source, rather than accepting a cached bytecode file from another run.
    exec(compile((HERE / "pii-scan.py").read_bytes(), str(HERE / "pii-scan.py"), "exec"), scanner.__dict__)
    text = log.decode("utf-8", errors="replace")
    for key, value in sorted(environment.items(), key=lambda pair: len(pair[1]), reverse=True):
        if value and re.search(r"TOKEN|PASSWORD|SECRET|KEY|COOKIE|AUTH", key, re.IGNORECASE):
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

    def __init__(self):
        self.executable = shutil.which("zerocode-orc")
        if not self.executable:
            raise UnknownInput("authenticated window ledger is unavailable")

    def call(self, verb, args, value, retry, extra=()):
        result = subprocess.run(
            [self.executable, verb, "--task", args.task, "--attempt", args.attempt,
             "--body", encoded(value).decode(), "--retry-request", retry, *extra],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=60,
        )
        if result.returncode != 0:
            raise UnknownInput(f"native ledger refused {verb} (exit {result.returncode})")
        answer = json.loads(result.stdout)
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
        stdin=subprocess.DEVNULL, start_new_session=os.name == "posix",
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
    command = args.command
    environment = execution_environment(args)
    before = fingerprint(args, environment, command)
    ledger = ledger or Ledger()
    nonce = uuid.uuid4().hex
    registered = ledger.call("gate-register", args, before, f"gate-register-{nonce}")
    registration = registered.get("registration")
    lease = registered.get("lease")
    if not isinstance(registration, str) or not isinstance(lease, str) or not isinstance(registered.get("registeredMs"), int):
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
        log = mask_log(raw_log, {**os.environ, **environment})
        raw_count, masked_count = test_count(raw_log), test_count(log)
        metadata = {
            "version": 1, "registration": registration, "lease": lease, "execution": started["execution"],
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
            "outcome": metadata["outcome"], "tests": metadata["tests"]}


def verify(args, ledger=None):
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
        "tests": test_count(log),
    }
    answer = (ledger or Ledger()).call("gate-reuse", args, request, f"gate-reuse-{uuid.uuid4().hex}")
    if answer.get("executed") is not False or answer.get("newExecutions") != 0:
        raise UnknownInput("unsupported native reuse decision")
    if answer.get("decision") not in ("hit", "miss") or not isinstance(answer.get("decisionRecord"), str):
        raise UnknownInput("native ledger did not record a reuse decision")
    if answer.get("decision") == "hit" and (
        answer.get("gate") not in ("gate-root", "gate-zo")
        or answer.get("mandatoryChecks") != ["windows", "flake-judgment", "publish"]
    ):
        raise UnknownInput("native decision has an unsupported reuse scope")
    return answer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("run", "verify"))
    parser.add_argument("--repo", required=True)
    parser.add_argument("--gate", choices=("gate-root", "gate-zo"), required=True)
    parser.add_argument("--task", required=True)
    parser.add_argument("--attempt", required=True)
    parser.add_argument("--scope", choices=("unknown", "deterministic", "measurement", "nondeterministic"), default="unknown")
    parser.add_argument("--closure", choices=("unknown", "declared"), default="unknown")
    parser.add_argument("--clean-env", action="store_true", help="execute with a fixed public environment instead of inheriting secrets")
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
    arguments = sys.argv[1:]
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
