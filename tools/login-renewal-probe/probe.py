#!/usr/bin/env python3
"""Does a login renewal row ask a model? (t-10915)

The window renews a login it does not run by starting the vendor's CLI once
with the words of a `LOGIN_RENEWALS` row (crates/zerocode-core/src/
login_renewal.rs). A row must be answered by the CLI itself: a row the CLI
hands to a model spends the person's plan on every renewal. This probe runs
the row against a fake store and a fake API and counts the model calls:

* the store is a temporary directory holding an EXPIRED made-up login and a
  made-up account profile — never a person's;
* `ANTHROPIC_BASE_URL` is a local HTTP server that counts `/v1/messages`
  and answers everything else with 500;
* `HTTPS_PROXY` is a local listener that writes down which hosts the CLI
  tried to reach and closes every connection, so nothing leaves the machine.

The control is the same words with an ordinary sentence on stdin: it MUST
reach `/v1/messages`, or the counter is not counting. Exit 0 when the row
made no model call and the control made one.

    python3 tools/login-renewal-probe/probe.py [--cli PATH] [--json]

The row is read out of the Rust source, not copied here.
"""

from __future__ import annotations

import argparse
import http.server
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HEADLESS_SOURCE = ROOT / "crates/zerocode-core/src/type_value.rs"
RENEWAL_SOURCE = ROOT / "crates/zerocode-core/src/login_renewal.rs"
CONTROL_STDIN = "hello"
WALL_S = 30.0


def claude_row(headless: str, renewal: str) -> tuple[list[str], str]:
    """The Claude row's argv and stdin, out of the two source files."""
    block = re.search(r"pub const CLAUDE_HEADLESS: &\[&str\] = &\[(.*?)\];", headless, re.S)
    if not block:
        raise ValueError("CLAUDE_HEADLESS not found")
    argv = re.findall(r'"((?:[^"\\]|\\.)*)"', block.group(1))
    row = re.search(r"pub const CLAUDE_RENEWAL: LoginRenewal = LoginRenewal \{(.*?)\};", renewal, re.S)
    if not row or "CLAUDE_HEADLESS" not in row.group(1):
        raise ValueError("CLAUDE_RENEWAL does not name CLAUDE_HEADLESS")
    stdin = re.search(r'stdin: "((?:[^"\\]|\\.)*)"', row.group(1))
    if not stdin:
        raise ValueError("CLAUDE_RENEWAL has no stdin")
    return argv, stdin.group(1)


def fake_store(directory: Path, now_ms: int) -> None:
    """An expired made-up login and a made-up profile, as a CLI's store."""
    directory.mkdir(parents=True, exist_ok=True)
    (directory / ".credentials.json").write_text(json.dumps({"claudeAiOauth": {
        "accessToken": "probe-access-not-a-token",
        "refreshToken": "probe-refresh-not-a-token",
        "expiresAt": now_ms - 3_600_000,
        "scopes": ["user:inference", "user:profile"],
        "subscriptionType": "max",
    }}))
    (directory / ".claude.json").write_text(json.dumps({
        "hasCompletedOnboarding": True,
        "theme": "dark",
        "oauthAccount": {
            "accountUuid": "00000000-0000-0000-0000-000000000001",
            "emailAddress": "dev@example.com",
            "organizationUuid": "00000000-0000-0000-0000-000000000002",
            "organizationName": "Example",
            "organizationType": "claude_max",
        },
    }))


class Api(http.server.ThreadingHTTPServer):
    """The fake API: counts model calls, answers everything with 500."""

    daemon_threads = True

    def __init__(self) -> None:
        self.messages = 0
        self.lock = threading.Lock()
        super().__init__(("127.0.0.1", 0), ApiHandler)


class ApiHandler(http.server.BaseHTTPRequestHandler):
    def answer(self) -> None:
        if self.path.split("?")[0].endswith("/v1/messages"):
            with self.server.lock:
                self.server.messages += 1
        self.send_response(500)
        self.send_header("content-length", "0")
        self.end_headers()

    do_GET = do_POST = do_PUT = do_HEAD = answer

    def log_message(self, *_args) -> None:
        pass


class Proxy:
    """Writes down the first line of every proxied request, then hangs up."""

    def __init__(self) -> None:
        self.lines: list[str] = []
        self.socket = socket.socket()
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen(32)
        threading.Thread(target=self.serve, daemon=True).start()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.socket.getsockname()[1]}"

    def serve(self) -> None:
        while True:
            try:
                connection, _ = self.socket.accept()
            except OSError:
                return
            try:
                first = connection.recv(4096).split(b"\r\n")[0].decode(errors="replace")
                self.lines.append(first)
            finally:
                connection.close()


def run(cli: str, argv: list[str], stdin: str, api: Api, proxy: Proxy, store: Path) -> dict:
    """One run of `cli` under the fake store, API and proxy; ended as soon as
    a model call lands, since the fake API never lets one finish."""
    env = {key: value for key, value in os.environ.items()
           if key not in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN")}
    env.update({
        "CLAUDE_CONFIG_DIR": str(store),
        "CLAUDE_SECURESTORAGE_CONFIG_DIR": str(store),
        "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{api.server_address[1]}",
        "HTTPS_PROXY": proxy.url,
        "https_proxy": proxy.url,
        "NO_PROXY": "127.0.0.1",
        "no_proxy": "127.0.0.1",
    })
    before = api.messages
    started = time.monotonic()
    with tempfile.TemporaryDirectory() as empty:
        child = subprocess.Popen([cli, *argv], cwd=empty, env=env, stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                 start_new_session=True)
        child.stdin.write(stdin.encode())
        child.stdin.close()
        while child.poll() is None and time.monotonic() - started < WALL_S:
            if api.messages > before:
                break
            time.sleep(0.05)
        ended = child.poll() is not None
        if not ended:
            os.killpg(child.pid, 9)
        out = child.stdout.read().decode(errors="replace")
        child.wait()
    said: dict = {}
    try:
        said = json.loads(out.strip().splitlines()[-1]) if out.strip() else {}
    except ValueError:
        pass
    usage = said.get("usage") or {}
    tokens = sum(usage.get(key) or 0 for key in (
        "input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
    return {
        "messages": api.messages - before,
        "ended": ended,
        "rc": child.returncode if ended else None,
        "ms": round((time.monotonic() - started) * 1000),
        "num_turns": said.get("num_turns"),
        "tokens": tokens if said else None,
    }


def probe(cli: str) -> dict:
    argv, stdin = claude_row(HEADLESS_SOURCE.read_text(), RENEWAL_SOURCE.read_text())
    api = Api()
    threading.Thread(target=api.serve_forever, daemon=True).start()
    proxy = Proxy()
    root = Path(tempfile.mkdtemp(prefix="login-renewal-probe-"))
    try:
        now_ms = int(time.time() * 1000)
        fake_store(root / "row", now_ms)
        fake_store(root / "control", now_ms)
        row = run(cli, argv, stdin, api, proxy, root / "row")
        control = run(cli, argv, CONTROL_STDIN, api, proxy, root / "control")
    finally:
        api.shutdown()
        proxy.socket.close()
        shutil.rmtree(root, ignore_errors=True)
    return {
        "argv": argv,
        "stdin": stdin,
        "row": row,
        "control": control,
        "ok": row["messages"] == 0 and control["messages"] >= 1,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--cli", default=shutil.which("claude"), help="the claude program")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if not args.cli:
        print("claude not found on PATH; pass --cli", file=sys.stderr)
        return 2
    result = probe(args.cli)
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        row, control = result["row"], result["control"]
        print(f"row     {result['stdin']!r}: model calls {row['messages']}, "
              f"turns {row['num_turns']}, tokens {row['tokens']}, {row['ms']} ms")
        print(f"control {CONTROL_STDIN!r}: model calls {control['messages']}")
        print("ok" if result["ok"] else "FAIL")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
