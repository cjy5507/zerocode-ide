#!/usr/bin/env python3
"""Loopback-only fixture for the embedded CEF smoke test."""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class FixtureHandler(BaseHTTPRequestHandler):
    request_log: Path

    def log_message(self, _format: str, *_args: object) -> None:
        pass

    def do_GET(self) -> None:
        cookie = self.headers.get("Cookie", "")
        record = {"path": self.path, "cookie": cookie}
        with self.request_log.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(record, separators=(",", ":")) + "\n")

        imported = "synthetic=first-profile" in cookie
        proof = "fixture: imported" if imported else "fixture: separate"
        body = f"""<!doctype html>
<html><head><meta charset=\"utf-8\"><title>CEF Smoke Fixture</title>
<style>body{{font:24px -apple-system,sans-serif;margin:48px;color:#14213d}}#proof{{padding:24px;background:#e5f4e3;border:2px solid #2a9d8f}}</style>
</head><body><h1>Embedded CEF smoke</h1><p id=\"proof\">{proof}</p></body></html>""".encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file", required=True, type=Path)
    parser.add_argument("--request-log", required=True, type=Path)
    args = parser.parse_args()

    FixtureHandler.request_log = args.request_log
    server = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    args.port_file.write_text(str(server.server_port), encoding="utf-8")
    server.serve_forever()


if __name__ == "__main__":
    main()
