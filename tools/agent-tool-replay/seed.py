#!/usr/bin/env python3
"""Build one golden seed for the agent tool seat's measurement (t-6040).

The seed is an EXTRACTION and never a calculation: every field it carries is
copied out of a vault page, and the arithmetic — agreement, Wilson bounds,
tokens and latency per arm — lives in one place only, the Rust harness
(`tools::smart_router::agent_tool::tests::jev_against_the_frontier_on_the_golden`),
which reads this file and asks both arms the same question.

One source, read-only: the second brain's `wiki/` pages. A page belongs to
the golden when its frontmatter `tags` carry exactly ONE of `TOPIC_TAGS` —
that tag is the label, the page's title and the head of its body are the
context, and the eleven tags with their one-line meanings are the options.
Nothing of a page beyond `TEXT_CHAR_CAP` characters leaves this script, which
is the cap the seat's door cuts a context to.

Usage:

    python3 tools/agent-tool-replay/seed.py \
        --vault "$ZEROCODE_SECOND_BRAIN" \
        --out /tmp/agent-tool-replay/seed.json
"""

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import sys

#: Characters of one context the seat's door lets through — the core table's
#: `AGENT_TOOL_TEXT_CHAR_CAP`, which is `ROUTING_TASK_CHAR_CAP`.
TEXT_CHAR_CAP = 2_000

#: The most options a `choose` may offer — the core table's
#: `AGENT_TOOL_OPTION_CAP` (the wire's own ceiling).
OPTION_CAP = 255

#: The one question both arms are asked, word for word.
QUESTION = "Which one topic is this wiki page mainly about?"

#: The closed set of topic tags, each with the meaning both arms are shown.
#: Order is the option order; a page is in the golden when it carries exactly
#: one of these in its frontmatter `tags`.
TOPIC_TAGS = (
    ("orchestration", "coordinating worker agents: the ledger, summons, tasks, dispatches, seats"),
    ("computer-use", "driving a desktop by hand and eye: clicks, OCR, marks, the Computer Use helper"),
    ("jev", "TypeSafe's Jev / System One judgments: seats, doors, promotion, hedges, rubrics"),
    ("terminal", "the terminal grid: PTY, painter, selection, copy, frames, throughput"),
    ("knowledge-graph", "the second-brain graph: nodes, edges, provenance, lenses, recall"),
    ("release", "shipping: the release lane, versions, bundles, updater feeds, signing"),
    ("emulator", "the mobile emulator / simulator: devices, streams, iOS and Android hands"),
    ("browser", "the built-in browser pane: tabs, walks, marks, navigation, observation"),
    ("ui", "the window's own screens: panes, boards, settings, styling, layout"),
    ("macos", "macOS itself: TCC permissions, keychain, launchd, signing, the OS APIs"),
    ("windows", "Windows itself: the MSVC build, Known Folders, COM, the Windows leg of CI"),
)

_FRONTMATTER = re.compile(r"\A---\n(.*?)\n---\n", re.S)
_TAGS = re.compile(r"^tags:\s*\[(.*?)\]\s*$", re.M)
_TITLE = re.compile(r'^title:\s*"?(.*?)"?\s*$', re.M)
_HEADING = re.compile(r"^#\s+(.*)$", re.M)


def tags_of(frontmatter: str) -> list[str]:
    """The inline `tags: [a, b]` of a page's frontmatter, quotes stripped."""
    found = _TAGS.search(frontmatter)
    if not found:
        return []
    return [tag.strip().strip("\"'") for tag in found.group(1).split(",") if tag.strip()]


def title_of(frontmatter: str, body: str) -> str:
    found = _TITLE.search(frontmatter)
    if found and found.group(1).strip():
        return found.group(1).strip()
    heading = _HEADING.search(body)
    return heading.group(1).strip() if heading else ""


def head_of(body: str) -> str:
    """The body with its first heading dropped, cut to `TEXT_CHAR_CAP`."""
    lines = [line for line in body.strip().splitlines() if not line.startswith("# ")]
    text = "\n".join(lines).strip()
    return text[:TEXT_CHAR_CAP]


def context_of(title: str, head: str) -> str:
    """What both arms read: the title, a blank line, the head — cut once
    more, so a long title cannot carry the context past the cap."""
    return f"{title}\n\n{head}"[:TEXT_CHAR_CAP]


def extract(vault: pathlib.Path) -> list[dict]:
    """Every golden item, in file-name order — one per page carrying exactly
    one topic tag."""
    topics = {tag for tag, _ in TOPIC_TAGS}
    items = []
    for page in sorted((vault / "wiki").glob("*.md")):
        text = page.read_text(encoding="utf-8")
        front = _FRONTMATTER.match(text)
        if not front:
            continue
        carried = [tag for tag in tags_of(front.group(1)) if tag in topics]
        if len(carried) != 1:
            continue
        body = text[front.end():]
        items.append({
            "file": page.name,
            "label": carried[0],
            "context": context_of(title_of(front.group(1), body), head_of(body)),
        })
    return items


def seed(vault: pathlib.Path) -> dict:
    if len(TOPIC_TAGS) > OPTION_CAP:
        raise SystemExit(f"{len(TOPIC_TAGS)} topics is more than a choose may offer ({OPTION_CAP})")
    return {
        "question": QUESTION,
        "options": [{"id": tag, "text": f"{tag} — {meaning}"} for tag, meaning in TOPIC_TAGS],
        "textCharCap": TEXT_CHAR_CAP,
        "extractedAt": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "items": extract(vault),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--vault", default=os.environ.get("ZEROCODE_SECOND_BRAIN", ""), help="the second-brain vault (its wiki/ is read)")
    parser.add_argument("--out", required=True, help="where to write seed.json")
    args = parser.parse_args()
    if not args.vault:
        parser.error("--vault or ZEROCODE_SECOND_BRAIN is required")
    vault = pathlib.Path(args.vault).expanduser()
    built = seed(vault)
    out = pathlib.Path(args.out).expanduser()
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(built, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    by_label: dict[str, int] = {}
    for item in built["items"]:
        by_label[item["label"]] = by_label.get(item["label"], 0) + 1
    print(f"{len(built['items'])} items over {len(built['options'])} options -> {out}", file=sys.stderr)
    for label, count in sorted(by_label.items(), key=lambda pair: -pair[1]):
        print(f"  {count:4d}  {label}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
