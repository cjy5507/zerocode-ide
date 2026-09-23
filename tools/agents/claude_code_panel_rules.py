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
`--chat-*` tokens with what is written there. A few of the panel's measures
live in its script rather than its stylesheet — how tall a person's message
stands before "Show more", how tall a diff's box, what makes a tool's words
"long" — and those are read off `extension/webview/index.js` the same way,
each by a shape the minifier leaves alone (t-6323).

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
SCRIPT = "extension/webview/index.js"

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
    # What a row folds (t-6323): a tool body's row stops at its max-height
    # behind a mask; a diff's box and a person's message fade out at their foot.
    {
        "key": "toolBodyRowContent",
        "selector": ".toolBodyRowContent:not(.toolBodyRowContent_disableClipping)",
        "landmark": "toolBody",
    },
    {"key": "userMessage truncationGradient", "selector": ".truncationGradient", "landmark": "expandableContainer"},
    {"key": "diff truncationGradient", "selector": ".truncationGradient", "landmark": "diffEditorWrapper"},
    # A todo call's list (t-6323 A7): a done item's fade and the box's margin.
    # Three modules name a `.checkbox`; the list's is the one that draws a
    # mixed state (`✽` under way).
    {"key": "todo completed", "selector": ".completed", "landmark": "todoList"},
    {"key": "todo checkbox", "selector": ".checkbox", "landmark": "checkbox:indeterminate"},
    # An image a message carries (t-6323 A8): its pill and thumbnail, the
    # row they stand in above the person's words, and the preview a press
    # opens.
    {"key": "attachment pill", "selector": ".pill", "landmark": "thumbIcon"},
    {"key": "attachment thumbIcon", "selector": ".thumbIcon", "landmark": "thumbIcon"},
    {"key": "attachment meta", "selector": ".meta", "landmark": "thumbIcon"},
    {"key": "userMessageAttachments", "selector": ".userMessageAttachments", "landmark": "messagesContainer"},
    {"key": "previewOverlay", "selector": ".previewOverlay", "landmark": "previewOverlay"},
    {"key": "previewImage", "selector": ".previewImage", "landmark": "previewOverlay"},
    {"key": "previewCloseButton", "selector": ".previewCloseButton", "landmark": "previewOverlay"},
    {"key": "previewCloseIcon", "selector": ".previewCloseIcon", "landmark": "previewOverlay"},
    # Copying (t-6323 A9): the code block's copy over its corner, the shared
    # button's own box and icon — the shared button's module, not the login
    # screen's, which draws its own copy with the same class names.
    {"key": "code copyButton", "selector": ".copyButton", "landmark": "codeBlockWrapper"},
    {"key": "copyButton", "selector": ".copyButton", "landmark": "copyIcon", "without": "authUrlInput"},
    {"key": "copyButton:active", "selector": ".copyButton:active", "landmark": "copyIcon", "without": "authUrlInput"},
    {"key": "copyIcon", "selector": ".copyIcon", "landmark": "copyIcon", "without": "authUrlInput"},
]

# The panel's measures that live in its SCRIPT: each found by a shape the
# minifier keeps (class names, property names, the arithmetic around the
# number) with the numbers captured in order. A pattern that no longer
# matches exactly once refuses the snapshot, as a missing rule does.
_NAME = r"[A-Za-z0-9_$]+"
WANTED_CONSTANTS = [
    # A person's message stops at this height before "Show more"
    # (`F(EV0, {content, context, maxHeight: 60})` inside `userMessage`).
    {
        "keys": ["userMessageMaxHeight"],
        "pattern": rf"className:{_NAME}\.userMessage,children:\[{_NAME},{_NAME}\({_NAME},\{{[^{{}}]*?maxHeight:(\d+)\}}\)\]",
    },
    # A diff's box: `Math.min(200, contentHeight + 20)`, truncated past it.
    {
        "keys": ["diffMaxHeight", "diffHeightPad"],
        "pattern": rf"Math\.min\((\d+),{_NAME}\+(\d+)\);return\{{height:{_NAME},truncated:",
    },
    # A tool's words are long past 250 characters or 3 lines (`cN`): such a
    # row is clipped and opens its full text.
    {
        "keys": ["longTextChars", "longTextLines"],
        "pattern": rf"function {_NAME}\({_NAME}\)\{{return {_NAME}\.length>(\d+)\|\|{_NAME}\.split\(`\n`\)\.length>(\d+)\}}",
    },
    # The spinner's verb is picked again after these, then every the last
    # (`Ke`: `[2000,3000,5000]`, then 5000).
    {
        "keys": ["spinnerVerbAfter1", "spinnerVerbAfter2", "spinnerVerbAfter3", "spinnerVerbEvery"],
        "pattern": rf"let {_NAME}=\[(\d+),(\d+),(\d+)\];return {_NAME}<{_NAME}\.length\?{_NAME}\[{_NAME}\]:(\d+)",
    },
    # A new verb is revealed one step every this many milliseconds (`j75`).
    {
        "keys": ["spinnerRevealStep"],
        "pattern": rf"let {_NAME}=null,{_NAME}=0,{_NAME}=(\d+),{_NAME}=\({_NAME}\)=>\{{if\({_NAME}-{_NAME}<{_NAME}\)\{{{_NAME}=requestAnimationFrame",
    },
    # The spinner's glyph turns every this many milliseconds.
    {
        "keys": ["spinnerGlyphStep"],
        "pattern": rf"setInterval\(\(\)=>\{{{_NAME}\(\({_NAME}\)=>\({_NAME}\+1\)%{_NAME}\.length\)\}},(\d+)\)",
    },
    # The list stands at its foot within this many pixels (`TF`, right before
    # `dH`, the distance to the foot).
    {
        "keys": ["followSlack"],
        "pattern": rf"var {_NAME}=(\d+);function {_NAME}\({_NAME}\)\{{return {_NAME}\.scrollHeight-{_NAME}\.scrollTop-{_NAME}\.clientHeight\}}",
    },
    # A send's glide home keeps gliding while it is younger than this (`g25`,
    # right before the follow judge `lF1`).
    {
        "keys": ["followGlide"],
        "pattern": rf"var {_NAME}=(\d+);function {_NAME}\(\{{atBottom:",
    },
    # Helpers at work stand as this many rows before one sums the rest
    # (`sD1`, in the split `AU0` makes).
    {
        "keys": ["agentRowsShown"],
        "pattern": rf"var {_NAME}=(\d+);function {_NAME}\({_NAME}\)\{{if\({_NAME}\.length<={_NAME}\+1\)return\{{visible:{_NAME},overflow:\[\]\}}",
    },
    # A copy says it copied for this long (`gN`: the check, then the copy
    # icon again).
    {
        "keys": ["copiedFor"],
        "pattern": rf"navigator\.clipboard\.writeText\({_NAME}\)\)\.then\(\(\)=>\{{{_NAME}\(!0\),setTimeout\(\(\)=>{_NAME}\(!1\),(\d+)\)",
    },
    # A wheel, a touch or a key is the person's intent for this long (`c25`,
    # after the two key sets and before the controls that keep a Space).
    {
        "keys": ["followIntent"],
        "pattern": rf"{_NAME}=new Set\(\[[^\]]+\]\),{_NAME}=new Set\(\[[^\]]+\]\),{_NAME}=(\d+),{_NAME}='",
    },
]

# Words the panel keeps in its script — lists read whole, as JSON: the
# spinner's verbs stand right after its glyph cycle (`·✢*✶✻✽` and back).
WANTED_WORDS = [
    {
        "key": "spinnerVerbs",
        "pattern": r'\["·","✢","\*","✶","✻","✽"\],'
        + _NAME
        + r"=\[\.\.\."
        + _NAME
        + r",\.\.\.\[\.\.\."
        + _NAME
        + r"\]\.reverse\(\)\],"
        + _NAME
        + r"=\[([^\]]+)\]",
    },
    # The keys that scroll the list up and down (`u25`, `m25`), and what keeps
    # a Space for itself (`l25`) — one selector, kept whole.
    {
        "key": "followUpKeys",
        "pattern": _NAME + r"=new Set\(\[([^\]]+)\]\)," + _NAME + r"=new Set\(\[[^\]]+\]\)," + _NAME + r"=\d+," + _NAME + r"='",
    },
    {
        "key": "followDownKeys",
        "pattern": _NAME + r"=new Set\(\[[^\]]+\]\)," + _NAME + r"=new Set\(\[([^\]]+)\]\)," + _NAME + r"=\d+," + _NAME + r"='",
    },
    {
        "key": "followControls",
        "whole": True,
        "pattern": _NAME + r"=new Set\(\[[^\]]+\]\)," + _NAME + r"=new Set\(\[[^\]]+\]\)," + _NAME + r"=\d+," + _NAME + r"='([^']+)'",
    },
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


def member_of(package: bytes, name: str) -> str:
    """One file inside the vsix (a zip, gzip-wrapped on the wire)."""
    if package[:2] == b"\x1f\x8b":
        package = gzip.decompress(package)
    with zipfile.ZipFile(io.BytesIO(package)) as archive:
        return archive.read(name).decode("utf-8")


def stylesheet_of(package: bytes) -> str:
    """The webview stylesheet inside the vsix."""
    return member_of(package, STYLESHEET)


def constants_of(script: str) -> dict[str, int]:
    """The script's measures (`WANTED_CONSTANTS`), each pattern matching
    exactly once — two matches would be a guess, none a lost measure."""
    found: dict[str, int] = {}
    for want in WANTED_CONSTANTS:
        hits = re.findall(want["pattern"], script)
        if len(hits) != 1:
            continue
        numbers = hits[0] if isinstance(hits[0], tuple) else (hits[0],)
        for key, number in zip(want["keys"], numbers):
            found[key] = int(number)
    return found


def words_of(script: str) -> dict[str, list[str]]:
    """The script's word lists (`WANTED_WORDS`), each found exactly once; a
    `whole` entry is one string, kept as a list of one."""
    found: dict[str, list[str]] = {}
    for want in WANTED_WORDS:
        hits = re.findall(want["pattern"], script)
        if len(hits) == 1:
            found[want["key"]] = [hits[0]] if want.get("whole") else json.loads("[" + hits[0] + "]")
    return found


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
    """The hashes of the modules that define `landmark` as a class. A landmark
    may carry a qualifier after its class — an attribute (`container[data-
    permission-mode`) or a state (`checkbox:indeterminate`) — for a base name
    every module shares: then only a module that writes the class with that
    qualifier counts."""
    name, qualifier = re.match(r"([\w-]+)(.*)", landmark).groups()
    found = set()
    for selector, _ in rules:
        for match in re.finditer(r"\." + re.escape(name) + r"_([A-Za-z0-9-]{6})\b", selector):
            if qualifier and not selector[match.end():].startswith(qualifier):
                continue
            found.add(match.group(1))
    return found


def rule_named(rules: list[tuple[str, str]], selector: str, landmark: str, without: str | None = None) -> dict[str, str]:
    """One module's rule merged across its lines: the module the landmark names,
    less any that also defines `without` — for class names two modules share
    whole (the chat's copy button and the login screen's)."""
    modules = modules_with(rules, landmark) - (modules_with(rules, without) if without else set())
    held: dict[str, str] = {}
    for raw, body in rules:
        if not any(module in raw for module in modules):
            continue
        if any(HASH.sub("", part.strip()) == selector for part in raw.split(",")):
            held.update(declarations_of(body))
    return held


def snapshot(css: str, version: str, source: str, script: str = "") -> dict:
    rules = rules_of(css)
    found: dict[str, dict[str, str]] = {}
    for want in WANTED:
        found[want["key"]] = rule_named(rules, want["selector"], want["landmark"], want.get("without"))
    variables: dict[str, str] = {}
    for selector, body in rules:
        if selector == "html":
            for name, value in declarations_of(body).items():
                if name in WANTED_VARS:
                    variables[name] = value
    constants = constants_of(script)
    words = words_of(script)
    missing = (
        [key for key, rule in found.items() if not rule]
        + [name for name in WANTED_VARS if name not in variables]
        + [key for want in WANTED_CONSTANTS for key in want["keys"] if key not in constants]
        + [want["key"] for want in WANTED_WORDS if want["key"] not in words]
    )
    if missing:
        raise SystemExit(f"the panel's stylesheet no longer names: {', '.join(missing)}")
    return {
        "source": source,
        "version": version,
        "fetched_at": dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat(),
        "stylesheet": STYLESHEET,
        "script": SCRIPT,
        "vars": variables,
        "rules": found,
        "constants": constants,
        "words": words,
    }


def installed_version() -> str | None:
    try:
        said = subprocess.run(["claude", "--version"], capture_output=True, text=True, timeout=20, check=False).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    match = re.search(r"(\d+\.\d+\.\d+)", said)
    return match.group(1) if match else None


def same_measures(a: dict, b: dict) -> bool:
    keys = ("version", "vars", "rules", "constants", "words")
    return tuple(a.get(key) for key in keys) == tuple(b.get(key) for key in keys)


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
    package = fetch(source)
    written = snapshot(stylesheet_of(package), version, source, member_of(package, SCRIPT))
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
    print(
        f"wrote {args.out} — {len(written['rules'])} rules, {len(written['vars'])} variables, "
        f"{len(written['constants'])} script measures, {len(written['words'])} word lists, version {version}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
