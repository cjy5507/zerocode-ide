#!/usr/bin/env python3
"""Loopback-only pages for the built-in browser's popup test (t-90671).

Every value here is synthetic. A page reports what it can see to /report, so the
verdict is what a page really got rather than what the browser says it did.
"""

from __future__ import annotations

import argparse
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

POST_FIELD = "synthetic_order"
POST_VALUE = "fixture-order-7"
SLOW_PART = 8 * 1024 * 1024

# What a popped page does: says what it sees, and hands its case back through
# its opener when it has one.
TARGET = """<!doctype html><meta charset="utf-8"><title>popup target {case}</title>
<h1 id="proof">popup target {case}</h1>
<script>
const seen = new URLSearchParams({{
  case: "{case}", via: "target",
  opener: String(!!window.opener),
  guest: String(window.__zcGuest === 1),
  agent: navigator.userAgent.includes("ZeroCodeProbeAgent") ? "worn" : "bare",
  post: {post},
  header: {header},
}});
// A popup's first document can begin before its scripts arrive, so what stands a
// moment later is reported too.
setTimeout(() => fetch("/report?" + new URLSearchParams({{
  case: "{case}", via: "target-late", guest: String(window.__zcGuest === 1),
}})), 400);
fetch("/report?" + seen).finally(() => {{
  if (window.opener) window.opener.__receive("{case}");
  if ({close}) setTimeout(() => window.close(), 150);
}});
</script>"""

OPENERS = {
    "open-empty-location": """window.__fire = () => {
        const w = window.open("");
        report("handle", String(!!w));
        if (w) w.location = "/target?case=open-empty-location";
      };""",
    "open-blank-write": """window.__fire = () => {
        const w = window.open("about:blank", "zc_written");
        report("handle", String(!!w));
        if (!w) return;
        w.document.write('<!doctype html><title>written</title><h1 id="proof">written</h1>');
        w.document.close();
        if (w.document.title === "written") w.opener.__receive("open-blank-write");
      };""",
    "form-post-named": """window.__fire = () => {
        const w = window.open("", "zc_pay");
        report("handle", String(!!w));
        document.querySelector("#named").submit();
      };""",
    "form-post-blank": """window.__fire = () => document.querySelector("#blank").submit();""",
    "opener-callback": """window.__fire = () => {
        report("handle", String(!!window.open("/start?case=opener-callback")));
      };""",
    "link-blank": """window.__fire = () => document.querySelector("#link").click();""",
    "open-noopener": """window.__fire = () => {
        window.open("/target?case=open-noopener", "_blank", "noopener");
      };""",
    "form-noopener": """window.__fire = () => document.querySelector("#noopener").submit();""",
    "link-download": """window.__fire = () => {
        document.querySelector("#link").href = "/file?case=link-download";
        document.querySelector("#link").click();
      };""",
    "slow-download-closed": """window.__fire = () => {
        document.querySelector("#link").href = "/slow-file?case=slow-download-closed";
        document.querySelector("#link").click();
      };""",
    "refused-settle": """window.__fire = () => document.querySelector("#blank").submit();""",
    "refused-data": """window.__fire = () => {
        const w = window.open('data:text/html,<script>fetch("/report?case=refused-data&via=target")</script>');
        report("handle", String(!!w));
      };""",
    "refused-later": """window.__fire = () => {
        const w = window.open("");
        report("handle", String(!!w));
        if (w) w.location = "zc-probe-refused://nowhere/";
      };""",
    "no-gesture": """window.__fire = () => {
        report("handle", String(!!window.open("/target?case=no-gesture")));
      };""",
}


class Handler(BaseHTTPRequestHandler):
    log: Path

    def log_message(self, _format: str, *_args: object) -> None:
        pass

    def note(self, row: dict) -> None:
        with self.log.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(row, separators=(",", ":")) + "\n")

    def page(self, body: str, status: int = 200) -> None:
        raw = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(raw)

    def target(self, case: str, post: str = "", close: bool = False) -> None:
        self.page(
            TARGET.format(
                case=case,
                post=json.dumps(post),
                close="true" if close else "false",
                header=json.dumps(
                    "worn" if "ZeroCodeProbeAgent" in self.headers.get("User-Agent", "") else "bare"
                ),
            )
        )

    def do_GET(self) -> None:
        url = urlparse(self.path)
        query = {key: values[0] for key, values in parse_qs(url.query).items()}
        case = query.get("case", "")
        if url.path == "/report":
            self.note(query)
            self.page("ok")
        elif url.path == "/opener":
            fire = OPENERS.get(case, "window.__fire = () => {};")
            self.page(f"""<!doctype html><meta charset="utf-8"><title>opener {case}</title>
<h1 id="proof">opener {case}</h1>
<form id="named" method="post" target="zc_pay" action="/post?case={case}">
<input type="hidden" name="{POST_FIELD}" value="{POST_VALUE}"></form>
<form id="blank" method="post" target="_blank" action="/post?case={case}">
<input type="hidden" name="{POST_FIELD}" value="{POST_VALUE}"></form>
<form id="noopener" method="post" target="_blank" rel="noopener" action="/post?case={case}">
<input type="hidden" name="{POST_FIELD}" value="{POST_VALUE}"></form>
<a id="link" target="_blank" href="/target?case={case}">link</a>
<script>
const report = (via, value) => fetch("/report?" + new URLSearchParams({{case: "{case}", via, value}}));
window.__receive = (got) => report("opener-got", got);
{fire}
</script>""")
        elif url.path == "/file":
            raw = b"synthetic file\n"
            self.note({"case": case, "via": "file-served"})
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Disposition", 'attachment; filename="fixture.bin"')
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
        elif url.path == "/slow-file":
            # Half now, half after the probe has closed the window: did CEF keep
            # reading once its browser went?
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Disposition", 'attachment; filename="fixture-slow.bin"')
            self.send_header("Content-Length", str(2 * SLOW_PART))
            self.end_headers()
            try:
                self.wfile.write(b"a" * SLOW_PART)
                self.wfile.flush()
                time.sleep(1.2)
                self.wfile.write(b"b" * SLOW_PART)
                self.wfile.flush()
                time.sleep(0.2)
                self.note({"case": case, "via": "slow-file-sent"})
            except OSError:
                self.note({"case": case, "via": "slow-file-cut"})
        elif url.path == "/start":
            self.send_response(302)
            self.send_header("Location", f"/callback?case={case}")
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif url.path == "/callback":
            self.target(case, close=True)
        elif url.path == "/target":
            self.target(case)
        else:
            self.page("not here", 404)

    def do_POST(self) -> None:
        url = urlparse(self.path)
        case = parse_qs(url.query).get("case", [""])[0]
        length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(length).decode("utf-8", "replace")
        self.note({"case": case, "via": "post-arrived", "body": body})
        self.target(case, post=body)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file", required=True, type=Path)
    parser.add_argument("--log", required=True, type=Path)
    args = parser.parse_args()
    Handler.log = args.log
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    args.port_file.write_text(str(server.server_port), encoding="utf-8")
    server.serve_forever()


if __name__ == "__main__":
    main()
