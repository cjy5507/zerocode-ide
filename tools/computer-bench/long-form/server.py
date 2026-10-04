#!/usr/bin/env python3
"""tools/computer-bench/long-form/server.py — serves the long-form bench page
on the loopback and records what it is sent (t-37883).

    server.py --out DIR [--port 0]

It prints one line, `listening http://127.0.0.1:<port>/`, once it answers.
The page reads `spec` (the form, without the card's answers: what an agent
fills must come from the card it was handed, never from the page) and posts
`submit`; every post lands as one line of DIR/submissions.jsonl, which the
oracle scores. Stdlib only, so the bench's own tests run it without a browser.
"""
import argparse
import hashlib
import http.server
import json
import pathlib
import threading
import time

HERE = pathlib.Path(__file__).resolve().parent
SPEC = HERE / "spec.json"
SUBMISSIONS = "submissions.jsonl"
# The files the page is made of, and the type each is served as.
PAGE_FILES = {"": ("form.html", "text/html; charset=utf-8"),
              "form.html": ("form.html", "text/html; charset=utf-8"),
              "form.js": ("form.js", "text/javascript; charset=utf-8")}
# A reference is the digest's first characters: short enough to read back.
REFERENCE_CHARS = 8
# The largest body a post may carry; the form's whole answer is a few KB.
MAX_BODY = 64 * 1024


def page_spec(spec):
    """The spec the page is served: the form, never the card."""
    return {key: value for key, value in spec.items() if key != "card"}


def reference_for(body):
    return "ZC-" + hashlib.sha256(body).hexdigest()[:REFERENCE_CHARS].upper()


class Handler(http.server.BaseHTTPRequestHandler):
    out: pathlib.Path
    spec: dict
    lock = threading.Lock()

    def log_message(self, *_args):
        return

    def reply(self, status, body, content_type):
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(body)))
        self.send_header("cache-control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        name = self.path.split("?", 1)[0].lstrip("/")
        if name == "spec":
            self.reply(200, json.dumps(page_spec(self.spec)).encode(), "application/json")
        elif name in PAGE_FILES:
            file_name, content_type = PAGE_FILES[name]
            self.reply(200, (HERE / file_name).read_bytes(), content_type)
        else:
            self.reply(404, b"not found", "text/plain")

    def do_POST(self):
        if self.path.split("?", 1)[0].lstrip("/") != "submit":
            self.reply(404, b"not found", "text/plain")
            return
        length = int(self.headers.get("content-length") or 0)
        if length <= 0 or length > MAX_BODY:
            self.reply(413, b"body too large or empty", "text/plain")
            return
        body = self.rfile.read(length)
        try:
            fields = json.loads(body)["fields"]
        except (ValueError, KeyError, TypeError):
            self.reply(400, b"not a form answer", "text/plain")
            return
        reference = reference_for(body)
        row = {"at_ms": int(time.time() * 1000), "reference": reference, "fields": fields}
        with self.lock, (self.out / SUBMISSIONS).open("a") as log:
            log.write(json.dumps(row) + "\n")
        self.reply(200, json.dumps({"reference": reference}).encode(), "application/json")


def serve(out, port=0):
    """A started server (on its own thread) and its base URL."""
    out = pathlib.Path(out)
    out.mkdir(parents=True, exist_ok=True)
    handler = type("BoundHandler", (Handler,), {"out": out, "spec": json.loads(SPEC.read_text())})
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}/"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True)
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    server, url = serve(args.out, args.port)
    print(f"listening {url}", flush=True)
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        server.shutdown()


if __name__ == "__main__":
    main()
