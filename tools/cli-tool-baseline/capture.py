#!/usr/bin/env python3
"""What a real Claude Code / Codex CLI advertises to its model, captured off the wire.

Rerun by hand when either CLI updates, then run `cargo test -p tools --lib wire_names` (t-96186):
  python3 tools/cli-tool-baseline/capture.py --claude "$(command -v claude)" --codex "$(command -v codex)" \
      --out zo-ide/crates/tools/tests/fixtures/wire-tools

Each CLI runs once under `sandbox-exec` with every network address but 127.0.0.1
shut, the person's own agent homes and keychains unreadable, a fresh temporary
HOME, a fake key, and its base URL pointed at a fake server on 127.0.0.1. The
server answers one short text turn and keeps the first request body that carries
tools. What is written is only the shape: each tool's name, its parameter names,
which are required, its description's length, and whether the CLI sent it on the
wire or held it back (`defer_loading`). No description text, no path, no key.
"""
import argparse
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

FAKE_KEY = "sk-ant-fake-0000000000000000000000000000"
CAPTURED = {}


def sse(event, payload):
    return f"event: {event}\ndata: {json.dumps(payload)}\n\n"


def anthropic_reply(model):
    usage = {"input_tokens": 1, "output_tokens": 1}
    body = sse("message_start", {"type": "message_start", "message": {
        "id": "msg_fake", "type": "message", "role": "assistant", "content": [],
        "model": model, "stop_reason": None, "stop_sequence": None, "usage": usage}})
    body += sse("content_block_start", {"type": "content_block_start", "index": 0,
                                        "content_block": {"type": "text", "text": ""}})
    body += sse("content_block_delta", {"type": "content_block_delta", "index": 0,
                                        "delta": {"type": "text_delta", "text": "ok"}})
    body += sse("content_block_stop", {"type": "content_block_stop", "index": 0})
    body += sse("message_delta", {"type": "message_delta",
                                  "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                                  "usage": usage})
    body += sse("message_stop", {"type": "message_stop"})
    return body


def responses_reply(model):
    item = {"type": "message", "id": "msg_fake", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": "ok", "annotations": []}]}
    resp = {"id": "resp_fake", "object": "response", "model": model, "status": "completed",
            "output": [item], "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2,
                                        "input_tokens_details": {"cached_tokens": 0},
                                        "output_tokens_details": {"reasoning_tokens": 0}}}
    body = sse("response.created", {"type": "response.created", "response": {**resp, "status": "in_progress", "output": []}})
    body += sse("response.output_item.added", {"type": "response.output_item.added", "output_index": 0, "item": item})
    body += sse("response.output_text.delta", {"type": "response.output_text.delta", "output_index": 0,
                                               "item_id": "msg_fake", "content_index": 0, "delta": "ok"})
    body += sse("response.output_item.done", {"type": "response.output_item.done", "output_index": 0, "item": item})
    body += sse("response.completed", {"type": "response.completed", "response": resp})
    return body


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"data":[],"models":[]}')

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length", "0") or 0))
        try:
            body = json.loads(raw or b"{}")
        except ValueError:
            body = {}
        tools = body.get("tools") or []
        if len(tools) >= 3 and self.server.label not in CAPTURED:
            CAPTURED[self.server.label] = body
        model = body.get("model", "fake")
        if self.path.rstrip("/").endswith("/responses"):
            payload = responses_reply(model)
        elif self.path.split("?")[0].rstrip("/").endswith("/messages"):
            payload = anthropic_reply(model)
        else:
            payload = None
        self.send_response(200)
        if payload is None:
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(b"{}")
            return
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        self.wfile.write(payload.encode())


def serve(label):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.label = label
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def sandbox_profile(real_home):
    shut = [os.path.join(real_home, d) for d in (".claude", ".codex", ".config", "Library/Keychains")]
    shut.append("/Library/Keychains")
    deny = "".join(f'(deny file-read* file-write* (subpath "{p}"))' for p in shut)
    return ('(version 1)(allow default)(deny network*)'
            '(allow network* (remote ip "localhost:*"))(allow network* (local ip "localhost:*"))' + deny)


def run(label, argv, env, cwd, timeout, tty=False):
    """Run one CLI until its first request with tools lands. With `tty`, the CLI
    gets a pseudo-terminal of its own, so it starts as the interactive program a
    person sees (which offers tools a `-p` run leaves out)."""
    profile = sandbox_profile(os.path.expanduser("~"))
    if tty:
        import pty
        primary, secondary = pty.openpty()
        os.set_blocking(primary, False)
        proc = subprocess.Popen(["sandbox-exec", "-p", profile, *argv], env=env, cwd=cwd,
                                stdin=secondary, stdout=secondary, stderr=secondary)
        os.close(secondary)
    else:
        primary = None
        proc = subprocess.Popen(["sandbox-exec", "-p", profile, *argv], env=env, cwd=cwd,
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    seen = b""
    deadline = time.time() + timeout
    while time.time() < deadline and label not in CAPTURED and proc.poll() is None:
        if primary is not None:
            try:
                chunk = os.read(primary, 65536)
                seen = (seen + chunk)[-4096:]
                # Answer the cursor-position query a TUI may wait on; never a
                # focus report (that makes the CLI read the clipboard).
                if b"\x1b[6n" in chunk:
                    os.write(primary, b"\x1b[1;1R")
            except (BlockingIOError, OSError):
                pass
        time.sleep(0.2)
    if proc.poll() is None:
        proc.kill()  # our own child, by its handle
    if primary is None:
        seen = proc.communicate()[0]
    else:
        proc.wait()
        os.close(primary)
    if label not in CAPTURED:
        sys.exit(f"{label}: no request with tools arrived; CLI said:\n{seen.decode(errors='replace')[-2000:]}")


def schema_shape(schema):
    schema = schema or {}
    props = schema.get("properties") or {}
    return {"parameters": sorted(props), "required": sorted(schema.get("required") or [])}


def anthropic_shape(body):
    rows = []
    for tool in body.get("tools") or []:
        rows.append({"name": tool.get("name"), "deferred": bool(tool.get("defer_loading")),
                     "description_chars": len(tool.get("description") or ""),
                     **schema_shape(tool.get("input_schema"))})
    return rows


DEFERRED_LEAD = "The following deferred tools are now available via ToolSearch."


def deferred_names(body):
    """Claude Code holds its deferred tools back by NAME in a reminder, not in
    `tools`; the names are the lines after the lead until a blank line."""
    text = json.dumps(body.get("system")) + json.dumps(body.get("messages"))
    text = json.loads(json.dumps(text))  # one plain string
    at = text.find(DEFERRED_LEAD)
    if at < 0:
        return []
    tail = text[at:].encode().decode("unicode_escape", errors="replace")
    lines = tail.split("\n")[1:]
    names = []
    for line in lines:
        line = line.strip()
        if not line or " " in line:
            break
        names.append(line)
    return sorted(names)


def responses_shape(body):
    rows = []
    for tool in body.get("tools") or []:
        kind = tool.get("type")
        if kind == "function":
            fn = tool.get("function", tool)
            rows.append({"name": fn.get("name"), "kind": "function", "deferred": bool(tool.get("defer_loading")),
                         "description_chars": len(fn.get("description") or ""),
                         **schema_shape(fn.get("parameters"))})
        else:
            rows.append({"name": tool.get("name") or kind, "kind": kind,
                         "deferred": bool(tool.get("defer_loading")),
                         "description_chars": len(tool.get("description") or ""),
                         "parameters": [], "required": []})
    return rows


def version(binary):
    return subprocess.run([binary, "--version"], capture_output=True, text=True).stdout.strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--claude")
    parser.add_argument("--claude-model", default="claude-opus-5-5")
    parser.add_argument("--codex")
    parser.add_argument("--codex-model", default="gpt-5.5")
    parser.add_argument("--out", required=True)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--raw", help="also keep each whole request body here (never commit it)")
    args = parser.parse_args()
    os.makedirs(args.out, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="cli-tool-baseline-") as tmp:
        tmp = os.path.realpath(tmp)
        work = os.path.join(tmp, "work")
        os.makedirs(work)
        base_env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TERM": "dumb", "LANG": "en_US.UTF-8"}

        if args.claude:
            home = os.path.join(tmp, "claude-home")
            config = os.path.join(home, ".claude")
            os.makedirs(config)
            with open(os.path.join(config, ".claude.json"), "w") as f:
                json.dump({"hasCompletedOnboarding": True,
                           "customApiKeyResponses": {"approved": [FAKE_KEY[-20:]], "rejected": []},
                           "projects": {work: {"hasTrustDialogAccepted": True}}}, f)
            server = serve("claude")
            env = {**base_env, "HOME": home, "CLAUDE_CONFIG_DIR": config, "ANTHROPIC_API_KEY": FAKE_KEY,
                   "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{server.server_port}",
                   "DISABLE_AUTOUPDATER": "1", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"}
            # Interactive, in a terminal of its own: what a person's session offers.
            run("claude", [args.claude, "--model", args.claude_model, "say ok"],
                {**env, "TERM": "xterm-256color"}, work, args.timeout, tty=True)
            server.shutdown()
            write(args.out, "claude-code", version(args.claude), CAPTURED["claude"], anthropic_shape, args.raw)

        if args.codex:
            home = os.path.join(tmp, "codex-home")
            codex_home = os.path.join(home, ".codex")
            os.makedirs(codex_home)
            server = serve("codex")
            with open(os.path.join(codex_home, "config.toml"), "w") as f:
                f.write(f'model = "{args.codex_model}"\nmodel_provider = "fake"\n'
                        f'[model_providers.fake]\nname = "fake"\n'
                        f'base_url = "http://127.0.0.1:{server.server_port}/v1"\n'
                        f'env_key = "FAKE_OPENAI_KEY"\nwire_api = "responses"\n')
            # The npm launcher is a node script: its own folder holds node too.
            path = os.path.dirname(os.path.realpath(args.codex)) + ":" + os.path.dirname(args.codex)
            env = {**base_env, "PATH": path + ":" + base_env["PATH"], "HOME": home,
                   "CODEX_HOME": codex_home, "FAKE_OPENAI_KEY": FAKE_KEY}
            run("codex", [args.codex, "exec", "--skip-git-repo-check", "say ok"], env, work, args.timeout)
            server.shutdown()
            write(args.out, "codex", version(args.codex), CAPTURED["codex"], responses_shape, args.raw)


def write(out, name, cli_version, body, shape, raw=None):
    if raw:
        os.makedirs(raw, exist_ok=True)
        with open(os.path.join(raw, f"{name}.request.json"), "w") as f:
            json.dump(body, f, indent=1)
    rows = shape(body)
    doc = {"cli": cli_version, "model": body.get("model"), "tools": rows,
           "deferred_by_name": deferred_names(body)}
    with open(os.path.join(out, f"{name}.json"), "w") as f:
        json.dump(doc, f, indent=2, sort_keys=True)
        f.write("\n")
    print(f"{name}: {cli_version}, {len(rows)} tools, {sum(not r['deferred'] for r in rows)} on the wire")


if __name__ == "__main__":
    main()
