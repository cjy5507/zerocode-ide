#!/usr/bin/env python3
"""Refresh the snapshot of the Claude Code VS Code panel's own stylesheet
that the window's conversation view is measured against.

The conversation view is drawn to look like the panel of the Claude Code
extension (docs/design/agent-conversation-claude-code-grammar-20260915.md).
Twice that likeness was read off screenshots and twice it was wrong; the
one honest source is the extension package itself — `extension/webview/
index.css` inside the marketplace vsix, the same version as the installed
CLI. This script fetches that package, reads the rules the view mirrors,
and writes them with their provenance (the URL, the version, the fetch time)
to `crates/zerocode-shell/src/chat/claude-code-panel.json`; the unit test
`the_conversation_wears_the_extensions_own_measures` compares the window's
`--chat-*` tokens with what is written there.

    python3 tools/agents/claude_code_panel_rules.py [--version 2.1.278] [--out PATH] [--check]

The version defaults to the installed `claude --version`. `--check` fetches
and exits 1 if the shipped snapshot differs (a gate for a stale snapshot),
writing nothing. The package is "© Anthropic PBC. All rights reserved": the
snapshot holds measures read off it for comparison, never its code.
"""
from __future__ import annotations

import argparse
import datetime as dt
import gzip
import io
import json
import re
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

SOURCE = (
    "https://marketplace.visualstudio.com/_apis/public/gallery/publishers/anthropic/"
    "vsextensions/claude-code/{version}/vspackage"
)
DEFAULT_OUT = Path(__file__).resolve().parents[2] / "crates/zerocode-shell/src/chat/claude-code-panel.json"
STYLESHEET = "extension/webview/index.css"

# The rules the view mirrors, each named for what it is on the panel. A rule
# is found by its CSS-module class names with the per-build hash stripped
# (`.timelineMessage_07S1Yg` → `.timelineMessage`); where several modules
# share a base name, `landmark` names a class that only the wanted module
# defines, so `.icon` is the spinner's and not a menu's.
WANTED = [
    {"key": "message", "selector": ".message", "landmark": "messagesContainer"},
    {"key": "timelineMessage", "selector": ".timelineMessage", "landmark": "messagesContainer"},
    {"key": "timelineMessage:before", "selector": ".timelineMessage:before", "landmark": "messagesContainer"},
    {"key": "timelineMessage:after", "selector": ".timelineMessage:after", "landmark": "messagesContainer"},
    {"key": "timelineMessage.dotSuccess:before", "selector": ".timelineMessage.dotSuccess:before", "landmark": "messagesContainer"},
    {"key": "timelineMessage.dotFailure:before", "selector": ".timelineMessage.dotFailure:before", "landmark": "messagesContainer"},
    {"key": "messagesContainer", "selector": ".messagesContainer", "landmark": "messagesContainer"},
    {"key": "messagesContainer.stickyMode:before", "selector": ".messagesContainer.stickyMode:before", "landmark": "messagesContainer"},
    {"key": "message.stickyHeader", "selector": ".message.stickyHeader", "landmark": "messagesContainer"},
    {"key": "userMessageContainer", "selector": ".userMessageContainer", "landmark": "messagesContainer"},
    {"key": "userMessage", "selector": ".userMessage", "landmark": "messagesContainer"},
    {"key": "metaMessage", "selector": ".metaMessage", "landmark": "messagesContainer"},
    {"key": "spinnerRow", "selector": ".spinnerRow", "landmark": "messagesContainer"},
    {"key": "messageGradient", "selector": ".messageGradient", "landmark": "messagesContainer"},
    {"key": "inputContainer", "selector": ".inputContainer", "landmark": "messagesContainer"},
    {"key": "assistantActions", "selector": ".assistantActions", "landmark": "messagesContainer"},
    {"key": "assistantActions copyResponseButton", "selector": ".assistantActions .copyResponseButton", "landmark": "messagesContainer"},
    {"key": "composer", "selector": ".inputContainer", "landmark": "messageInput"},
    {"key": "composer:focus-within", "selector": ".inputContainer:focus-within", "landmark": "messageInput"},
    {"key": "messageInput", "selector": ".messageInput", "landmark": "messageInput"},
    {"key": "inputFooter", "selector": ".inputFooter", "landmark": "inputFooterV2"},
    {"key": "sendButton", "selector": ".sendButton", "landmark": "inputFooterV2"},
    {"key": "modelPill", "selector": ".modelPill", "landmark": "inputFooterV2"},
    {"key": "spinner icon", "selector": ".icon", "landmark": "container[data-permission-mode"},
    {"key": "spinner text", "selector": ".text", "landmark": "container[data-permission-mode"},
    {"key": "toolNameText", "selector": ".toolNameText", "landmark": "toolBody"},
    {"key": "toolNameTextSecondary", "selector": ".toolNameTextSecondary", "landmark": "toolBody"},
    {"key": "toolBody", "selector": ".toolBody", "landmark": "toolBody"},
    {"key": "secondaryLine", "selector": ".secondaryLine", "landmark": "secondaryLine"},
    {"key": "thinkingV2 thinkingToggle", "selector": ".thinkingV2 .thinkingToggle", "landmark": "thinkingSummary"},
]
# The panel's own variables, read off its `html` rule.
WANTED_VARS = [
    "--app-claude-orange",
    "--app-claude-clay-button-orange",
    "--app-claude-ivory",
    "--corner-radius-small",
    "--corner-radius-medium",
    "--corner-radius-large",
    "--app-pill-min-height",
    "--app-spacing-small",
    "--app-spacing-medium",
    "--app-spacing-large",
]
HASH = re.compile(r"_[A-Za-z0-9-]{6}\b")


def fetch(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as response:  # noqa: S310 — a fixed marketplace URL
        return response.read()


def stylesheet_of(package: bytes) -> str:
    """The webview stylesheet inside the vsix (a zip, gzip-wrapped on the wire)."""
    if package[:2] == b"\x1f\x8b":
        package = gzip.decompress(package)
    with zipfile.ZipFile(io.BytesIO(package)) as archive:
        return archive.read(STYLESHEET).decode("utf-8")


def rules_of(css: str) -> list[tuple[str, str]]:
    """Every innermost `selector { body }` pair, selectors trimmed."""
    return [(selector.strip(), body) for selector, body in re.findall(r"([^{}]+)\{([^{}]*)\}", css)]


def declarations_of(body: str) -> dict[str, str]:
    held: dict[str, str] = {}
    for declaration in body.split(";"):
        if ":" not in declaration:
            continue
        name, value = declaration.split(":", 1)
        held[name.strip()] = " ".join(value.split())
    return held


def modules_with(rules: list[tuple[str, str]], landmark: str) -> set[str]:
    """The hashes of the modules that define `landmark` as a class."""
    found = set()
    for selector, _ in rules:
        for match in re.finditer(r"\." + re.escape(landmark.split("[", 1)[0]) + r"_([A-Za-z0-9-]{6})\b", selector):
            if "[" in landmark and landmark.split("[", 1)[1] not in selector:
                continue
            found.add(match.group(1))
    return found


def rule_named(rules: list[tuple[str, str]], selector: str, landmark: str) -> dict[str, str]:
    modules = modules_with(rules, landmark)
    held: dict[str, str] = {}
    for raw, body in rules:
        if not any(module in raw for module in modules):
            continue
        if any(HASH.sub("", part.strip()) == selector for part in raw.split(",")):
            held.update(declarations_of(body))
    return held


def snapshot(css: str, version: str, source: str) -> dict:
    rules = rules_of(css)
    found: dict[str, dict[str, str]] = {}
    for want in WANTED:
        found[want["key"]] = rule_named(rules, want["selector"], want["landmark"])
    variables: dict[str, str] = {}
    for selector, body in rules:
        if selector == "html":
            for name, value in declarations_of(body).items():
                if name in WANTED_VARS:
                    variables[name] = value
    missing = [key for key, rule in found.items() if not rule] + [name for name in WANTED_VARS if name not in variables]
    if missing:
        raise SystemExit(f"the panel's stylesheet no longer names: {', '.join(missing)}")
    return {
        "source": source,
        "version": version,
        "fetched_at": dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat(),
        "stylesheet": STYLESHEET,
        "vars": variables,
        "rules": found,
    }


def installed_version() -> str | None:
    try:
        said = subprocess.run(["claude", "--version"], capture_output=True, text=True, timeout=20, check=False).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    match = re.search(r"(\d+\.\d+\.\d+)", said)
    return match.group(1) if match else None


def same_measures(a: dict, b: dict) -> bool:
    return (a.get("version"), a.get("vars"), a.get("rules")) == (b.get("version"), b.get("vars"), b.get("rules"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--version", default=None, help="the extension version to read (default: the installed CLI's)")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    version = args.version or installed_version()
    if not version:
        print("no version given and no `claude --version` to read one from", file=sys.stderr)
        return 2
    source = SOURCE.format(version=version)
    written = snapshot(stylesheet_of(fetch(source)), version, source)
    if args.check:
        try:
            shipped = json.loads(args.out.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            shipped = {}
        if same_measures(shipped, written):
            print(f"{args.out} matches the panel's stylesheet at {version}")
            return 0
        print(f"{args.out} differs from the panel's stylesheet at {version}", file=sys.stderr)
        return 1
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(written, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {args.out} — {len(written['rules'])} rules, {len(written['vars'])} variables, version {version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
