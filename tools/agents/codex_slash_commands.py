#!/usr/bin/env python3
"""Refresh the Codex slash-command snapshot the window's composer palette
reads for Codex panes.

Codex keeps its slash catalog inside its TUI binary with no listing command,
so the window cannot ask the installed binary the way it asks Claude Code
(`--output-format stream-json` init) or zo (`zo commands --json`). The next
honest source is OpenAI's own reference page. This script fetches it, parses
the commands table, and writes the rows with their provenance — the page, the
fetch time, the Codex version installed when it was fetched — so the palette
can say "문서 기준 0.154.0" rather than pass the list off as the binary's.

    python3 tools/agents/codex_slash_commands.py [--out PATH] [--check]

`--check` fetches and exits 1 if the shipped snapshot differs (a gate for a
stale snapshot), writing nothing.
"""
from __future__ import annotations

import argparse
import datetime as dt
import html
import json
import re
import subprocess
import sys
import urllib.request
from html.parser import HTMLParser
from pathlib import Path

SOURCE = "https://learn.chatgpt.com/docs/developer-commands?surface=cli"
DEFAULT_OUT = Path(__file__).resolve().parents[2] / "crates/zerocode-shell/src/slash/codex-commands.json"


class Tables(HTMLParser):
    """Every <table> on the page as rows of cell texts."""

    def __init__(self) -> None:
        super().__init__()
        self.tables: list[list[list[str]]] = []
        self._row: list[str] | None = None
        self._cell: list[str] | None = None

    def handle_starttag(self, tag, attrs):
        if tag == "table":
            self.tables.append([])
        elif tag == "tr" and self.tables:
            self._row = []
        elif tag in ("td", "th") and self._row is not None:
            self._cell = []

    def handle_endtag(self, tag):
        if tag in ("td", "th") and self._cell is not None and self._row is not None:
            self._row.append(re.sub(r"\s+", " ", "".join(self._cell)).strip())
            self._cell = None
        elif tag == "tr" and self._row is not None and self.tables:
            if self._row:
                self.tables[-1].append(self._row)
            self._row = None

    def handle_data(self, data):
        if self._cell is not None:
            self._cell.append(html.unescape(data))


def fetch(url: str) -> str:
    request = urllib.request.Request(url, headers={"User-Agent": "zerocode-slash-snapshot/1"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read().decode("utf-8", "replace")


def commands_from(page: str) -> list[dict]:
    parser = Tables()
    parser.feed(page)
    rows: list[dict] = []
    for table in parser.tables:
        if not table:
            continue
        head = [cell.lower() for cell in table[0]]
        if not head or "command" not in head[0]:
            continue
        for cells in table[1:]:
            if not cells or not cells[0].startswith("/"):
                continue
            # "/model", "/model <arg>", or "/agent, /agents" (aliases): the first
            # slash word is the name, other slash words are aliases, and what
            # follows the name without a slash is the argument hint.
            words = re.findall(r"/[a-z][a-z0-9-]*", cells[0])
            if not words:
                continue
            name = words[0]
            aliases = words[1:]
            tail = cells[0].split(name, 1)[1]
            args = re.sub(r",?\s*/[a-z][a-z0-9-]*", "", tail).strip(" ,")
            about = cells[1] if len(cells) > 1 else ""
            row = {"name": name, "args": args, "about": about}
            if aliases:
                row["aliases"] = aliases
            rows.append(row)
    # One row per name, first wins, page order kept.
    seen: set[str] = set()
    unique = []
    for row in rows:
        if row["name"] in seen:
            continue
        seen.add(row["name"])
        unique.append(row)
    return unique


def codex_version() -> str | None:
    try:
        said = subprocess.run(["codex", "--version"], capture_output=True, text=True, timeout=20, check=False).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    match = re.search(r"(\d+\.\d+\.\d+)", said)
    return match.group(1) if match else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    page = fetch(SOURCE)
    commands = commands_from(page)
    if len(commands) < 10:
        print(f"parsed only {len(commands)} commands from {SOURCE}; the page shape changed", file=sys.stderr)
        return 2
    snapshot = {
        "agent": "codex",
        "source": SOURCE,
        "fetched_at": dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat(),
        "cli_version_observed": codex_version(),
        "commands": commands,
    }
    if args.check:
        held = json.loads(args.out.read_text()) if args.out.exists() else {}
        if held.get("commands") == commands:
            print(f"snapshot current: {len(commands)} commands")
            return 0
        print(f"snapshot stale: {len(held.get('commands', []))} shipped, {len(commands)} on the page", file=sys.stderr)
        return 1
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {len(commands)} commands to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
