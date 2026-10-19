"""Observe only this invocation's process tree. Run on the serialized build line.

Use the reviewed runtime's Python with psutil installed. No model or runtime is
downloaded by this tool. Arguments after -- name the compiled native-proof
example, including --await-driver and an explicit isolated store/report folder.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import socket
import subprocess
import sys
import threading
import time

import psutil


POLICY = "(version 1)(allow default)(deny network-outbound)"
INTERVAL = 0.1
CLEANUP_SECONDS = 5


def fingerprint(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        while block := stream.read(65536):
            digest.update(block)
    return digest.hexdigest()


def process_key(process):
    return process.pid, process.create_time()


def live(key):
    try:
        process = psutil.Process(key[0])
        return process.create_time() == key[1] and process.status() != psutil.STATUS_ZOMBIE
    except psutil.NoSuchProcess:
        return False


def offline_control(python):
    # A refused connection to our own listener proves the active sandbox policy.
    # It requires neither Internet access nor a connection to another service.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        probe = (
            "import errno,socket,sys; s=socket.socket(); s.settimeout(1); "
            "code=s.connect_ex(('127.0.0.1',int(sys.argv[1]))); "
            "sys.exit(0 if code in (errno.EPERM,errno.EACCES) else 1)"
        )
        result = subprocess.run(
            ["/usr/bin/sandbox-exec", "-p", POLICY, python, "-I", "-c", probe,
             str(listener.getsockname()[1])],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            timeout=5, check=False,
        )
    if result.returncode != 0:
        raise RuntimeError("active_offline_policy_not_confirmed")
    return {"rc": result.returncode, "policy": POLICY, "target": "owned_loopback_listener"}


class Observer:
    def __init__(self, command, report, cap_seconds, rss_cap):
        self.command = command
        self.folder = report
        self.cap_seconds = cap_seconds
        self.rss_cap = rss_cap
        self.started = time.monotonic()
        self.known = {}
        self.lines = queue.Queue(maxsize=128)
        self.reader_done = threading.Event()
        self.failure = None
        self.peak = 0
        self.phase = "starting"
        self.controls = []
        self.parent = None
        self.process = None
        self.trace = None

    def seconds(self):
        return time.monotonic() - self.started

    def read_lines(self):
        try:
            while line := self.process.stdout.readline(65537):
                if len(line) > 65536:
                    raise RuntimeError("phase_line_exceeds_cap")
                self.lines.put(line, timeout=1)
        except Exception as error:
            self.failure = str(error)
        finally:
            self.reader_done.set()

    def sample(self):
        try:
            children = self.parent.children(recursive=True)
        except psutil.NoSuchProcess:
            children = []
        for process in children:
            try:
                key = process_key(process)
                if key not in self.known:
                    executable = Path(process.exe()).resolve()
                    # Never capture command lines: native arguments contain a key.
                    self.known[key] = {
                        "pid": key[0], "createdEpochSeconds": key[1],
                        "name": executable.name,
                        "executableSha256": fingerprint(executable),
                        "firstSeenSeconds": self.seconds(),
                    }
            except psutil.NoSuchProcess:
                pass
        resident = 0
        rows = []
        for key, record in self.known.items():
            if not live(key):
                record.setdefault("endedSeconds", self.seconds())
                continue
            process = psutil.Process(key[0])
            try:
                rss = process.memory_info().rss
                cpu = process.cpu_times()
                resident += rss
                rows.append({"pid": key[0], "rssBytes": rss,
                             "cpuSeconds": cpu.user + cpu.system})
            except psutil.NoSuchProcess:
                pass
        self.peak = max(self.peak, resident)
        self.trace.write(json.dumps({"elapsedSeconds": self.seconds(), "phase": self.phase,
                                     "runtimeRssBytes": resident, "processes": rows}) + "\n")
        self.trace.flush()
        if resident > self.rss_cap:
            raise RuntimeError("observed_runtime_rss_exceeded_cap")
        if self.seconds() > self.cap_seconds:
            raise RuntimeError("observer_time_cap")
        if self.failure:
            raise RuntimeError(self.failure)

    def descendants(self):
        return [key for key in self.known if live(key)]

    def native_present(self):
        return any(row["name"] == "llama-server" and live(key)
                   for key, row in self.known.items())

    def await_cleanup(self):
        began = time.monotonic()
        while True:
            self.sample()
            remaining = self.descendants()
            if not remaining:
                return {"elapsedSeconds": time.monotonic() - began, "remainingProcesses": 0}
            if time.monotonic() - began >= CLEANUP_SECONDS:
                raise RuntimeError("owned_runtime_processes_remain")
            time.sleep(INTERVAL)

    def reply(self, value):
        self.process.stdin.write(value + "\n")
        self.process.stdin.flush()

    def handle(self, line):
        value = json.loads(line)
        self.phase = value["phase"]
        if self.phase == "runtime_ready":
            self.controls.append(offline_control(value["details"]["python"]))
            self.reply("ready")
        elif self.phase in ("load_cancel_window", "check_cancel_window"):
            # Cancellation must overlap an observed native child, not a mock.
            until = time.monotonic() + 3
            while not self.native_present():
                self.sample()
                if time.monotonic() >= until:
                    raise RuntimeError("no_native_process_at_cancellation")
                time.sleep(INTERVAL)
            self.reply("cancel")
        elif self.phase in ("load_cancel_complete", "check_cancel_complete", "stop_complete"):
            cleanup = self.await_cleanup()
            self.controls.append({"phase": self.phase, **cleanup})
            self.reply("stopped" if self.phase == "stop_complete" else "cancelled")

    def run(self):
        result = 1
        error = None
        self.folder.mkdir(parents=True, exist_ok=True)
        with (self.folder / "runtime-rss.jsonl").open("x") as trace, \
                (self.folder / "proof-stderr.log").open("x") as errors:
            self.trace = trace
            try:
                self.process = subprocess.Popen(
                    self.command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=errors, text=True, start_new_session=True,
                )
                self.parent = psutil.Process(self.process.pid)
                threading.Thread(target=self.read_lines, daemon=True).start()
                while (self.process.poll() is None or not self.reader_done.is_set()
                       or not self.lines.empty()):
                    self.sample()
                    try:
                        line = self.lines.get(timeout=INTERVAL)
                    except queue.Empty:
                        continue
                    self.handle(line)
                if self.process.returncode != 0:
                    raise RuntimeError("proof_process_failed")
                if self.phase != "parent_exit_armed":
                    raise RuntimeError("parent_exit_proof_missing")
                self.controls.append({"phase": "parent_eof", **self.await_cleanup()})
                proof = json.loads((self.folder / "report.json").read_text())
                if not proof["ok"] or proof.get("positiveNonCacheResponses", 0) < 9:
                    raise RuntimeError("real_protocol_responses_missing")
                result = 0
            except Exception as failure:
                error = str(failure)
            finally:
                # On failure, kill only processes whose identity this run observed.
                remaining = len(self.descendants())
                for key in reversed(list(self.known)):
                    if live(key):
                        try:
                            psutil.Process(key[0]).kill()
                        except psutil.NoSuchProcess:
                            pass
                if self.process is not None:
                    if self.process.poll() is None:
                        self.process.kill()
                    self.process.wait(timeout=5)
                receipt = {
                    "rc": result, "error": error, "platform": platform.platform(),
                    "architecture": platform.machine(), "observerSha256": fingerprint(__file__),
                    "binarySha256": fingerprint(self.command[0]),
                    "durationSeconds": self.seconds(), "sampleIntervalSeconds": INTERVAL,
                    "maxRuntimeRssBytes": self.peak, "rssCapBytes": self.rss_cap,
                    "controls": self.controls, "processes": list(self.known.values()),
                    "remainingProcessesBeforeFailureCleanup": remaining,
                }
                (self.folder / "observer.json").write_text(json.dumps(receipt, indent=2) + "\n")
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--cap-seconds", type=int, default=900)
    parser.add_argument("--rss-cap-bytes", type=int, default=4294967296)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    command = options.command
    if command and command[0] == "--":
        command = command[1:]
    if not command or "--await-driver" not in command or not Path(command[0]).is_absolute():
        parser.error("explicit binary and --await-driver are required")
    if not options.report.is_absolute() or options.cap_seconds <= 0 or options.rss_cap_bytes <= 0:
        parser.error("explicit report path and positive caps are required")
    report_index = command.index("--report") + 1 if "--report" in command else 0
    if not report_index or Path(command[report_index]).resolve() != options.report.resolve():
        parser.error("the proof and observer must share the evidence folder")
    return Observer(command, options.report, options.cap_seconds, options.rss_cap_bytes).run()


if __name__ == "__main__":
    sys.exit(main())
