#!/usr/bin/env python3
"""Gather the rings this window would have judged, and what the person did
in the minute after each, from the Claude transcripts on this machine — the
seed for the notify seat's replay (t-6043).

This script **only reads files and copies facts**. It judges nothing, asks
nothing and scores nothing: the question is `zerocode_core::notify_call::ask`,
today's rule is `Call::today`, the mark is `notify_call::agreed`, and the
replay that uses this seed hands each row to those functions as they ship
(`notify_call::tests::the_calls_this_machine_would_have_made`). A second copy
of any of that in Python would be a second rule.

What a seed row is: one moment the window's bell would have rung about a pane
that a Claude transcript recorded — a turn that ended (`finished`), a turn the
person stopped (`stopped`), an `AskUserQuestion` or `ExitPlanMode` the agent
blocked on (`needs input`) — with the facts the bell would have had AT THAT
MOMENT and nothing later:

  * the event's verb, the pane's name (the transcript's `cwd`, last segment)
    and the agent;
  * whether the person was present: their last typed prompt in ANY transcript
    strictly before the event is inside `ATTENDANCE_WINDOW_MS`;
  * one card line of the ring's words (the question, or the turn's last text);
  * the pane's last `RECENT_CAP` rings before this one, each with how long
    before and — only when its own minute had closed before this event —
    whether the person turned to it;
  * how many other panes stood at an open question at that moment.

And the label, which is the one thing read from AFTER the event: whether the
person's next typed prompt into that pane (or the answer to the question)
came inside `LABEL_WINDOW_MS`. The replay reads the label after it has asked,
never as input.

A prompt is the person's when the transcript says it was typed
(`promptSource: typed`) and it is not the orchestration ledger's own typing —
a summons or a mail pointer (`<pasted_content`, `orchestration message`).
Hook-injected notes (`isMeta`, `system`, `sdk`) are nobody's hand.

    tools/notify-replay/seed.py --out seed.json            # the window's projects and ~/.claude
    tools/notify-replay/seed.py --projects DIR --days 14
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

# `NOTIFY_LABEL_WINDOW_MS` in `crates/zerocode-core/src/jev.rs`: the minute
# after a ring inside which the person's hand is the ring's label.
LABEL_WINDOW_MS = 60 * 1_000

# `NOTIFY_ATTENDANCE_WINDOW_MS` there (the placement seat's window): how long
# since the person's last hand they still count as present.
ATTENDANCE_WINDOW_MS = 5 * 60 * 1_000

# `NOTIFY_RECENT_CAP` there: the pane's rings one question carries.
RECENT_CAP = 6

# `NOTIFY_WORDS_CHAR_CAP` there (one card line, `transcript::SUMMARY_CHARS`).
WORDS_CHAR_CAP = 240

# The verb table of `zerocode_core::notify` — the words the question reads.
VERB_ATTENTION = "needs input"
VERB_FINISHED = "finished"
VERB_STOPPED = "stopped"

# The tools a Claude agent blocks on for the person — `PreToolUse` on them is
# what the hook bridge reads as `needs-attention`.
ASKING_TOOLS = ("AskUserQuestion", "ExitPlanMode")

# What the orchestration ledger types into a pane: not a person's hand.
LEDGER_TYPED_MARKS = ("<pasted_content", "orchestration message")

# What Claude Code writes as the person's word when they stopped a turn.
INTERRUPT_MARK = "[Request interrupted by user"


def parse_ms(stamp: str | None) -> int | None:
    if not stamp:
        return None
    try:
        return int(datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return None


def read_rows(path: Path) -> list[dict]:
    """The transcript's records. A torn trailing line is skipped, never fatal."""
    rows = []
    with path.open(errors="replace") as held:
        for line in held:
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                rows.append(row)
    return rows


def text_of(row: dict) -> str | None:
    """The words of a user row that spoke, or `None` for a tool result."""
    content = (row.get("message") or {}).get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list) and content and content[0].get("type") == "text":
        return content[0].get("text") or ""
    return None


def is_persons_prompt(row: dict) -> bool:
    if row.get("type") != "user" or row.get("isMeta"):
        return False
    if row.get("promptSource") != "typed":
        return False
    text = text_of(row)
    if text is None or INTERRUPT_MARK in text:
        return False
    return not any(mark in text for mark in LEDGER_TYPED_MARKS)


def is_interrupt(row: dict) -> bool:
    text = text_of(row) if row.get("type") == "user" else None
    return text is not None and INTERRUPT_MARK in text


def last_text(row: dict) -> str:
    content = (row.get("message") or {}).get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        for block in reversed(content):
            if block.get("type") == "text" and block.get("text"):
                return block["text"]
    return ""


def card_line(text: str) -> str:
    """One card line: the first line that says anything, cut to the cap."""
    for line in text.splitlines():
        line = line.strip()
        if line:
            return line[:WORDS_CHAR_CAP]
    return ""


def asks_in(row: dict) -> list[tuple[str, str]]:
    """`(tool_use_id, question words)` for each asking tool a row calls."""
    content = (row.get("message") or {}).get("content")
    found = []
    if not isinstance(content, list):
        return found
    for block in content:
        if block.get("type") != "tool_use" or block.get("name") not in ASKING_TOOLS:
            continue
        held = block.get("input") or {}
        words = ""
        questions = held.get("questions")
        if isinstance(questions, list) and questions:
            words = str(questions[0].get("question") or "")
        elif block.get("name") == "ExitPlanMode":
            words = "plan ready for approval"
        found.append((block.get("id") or "", card_line(words)))
    return found


def tool_results_in(row: dict) -> set[str]:
    content = (row.get("message") or {}).get("content")
    if not isinstance(content, list):
        return set()
    return {block.get("tool_use_id") for block in content if block.get("type") == "tool_result"}


def events_of(path: Path) -> tuple[list[dict], list[int]]:
    """The rings one transcript would have rung, oldest first, and every
    moment the person typed into it — the hands other panes' attendance
    reads."""
    rows = read_rows(path)
    if not rows:
        return [], []
    agent = "claude"
    pane = ""
    for row in rows:
        if row.get("cwd"):
            pane = Path(row["cwd"]).name
            break
    hands = []
    events = []
    open_asks: dict[str, dict] = {}
    last_assistant: dict | None = None
    for row in rows:
        at = parse_ms(row.get("timestamp"))
        kind = row.get("type")
        if kind == "assistant" and at is not None:
            last_assistant = row
            for tool_use_id, words in asks_in(row):
                event = {
                    "at": at,
                    "verb": VERB_ATTENTION,
                    "interrupted": False,
                    "words": words,
                    "reactedAt": None,
                }
                events.append(event)
                open_asks[tool_use_id] = event
            continue
        if kind != "user" or at is None:
            continue
        for tool_use_id in tool_results_in(row):
            asked = open_asks.pop(tool_use_id, None)
            if asked is not None:
                asked["reactedAt"] = at
        if is_interrupt(row):
            events.append({
                "at": at,
                "verb": VERB_STOPPED,
                "interrupted": True,
                "words": card_line(last_text(last_assistant)) if last_assistant else "",
                "reactedAt": None,
            })
            last_assistant = None
            continue
        if not is_persons_prompt(row):
            continue
        hands.append(at)
        if last_assistant is not None:
            ended = parse_ms(last_assistant.get("timestamp"))
            if ended is not None:
                events.append({
                    "at": ended,
                    "verb": VERB_FINISHED,
                    "interrupted": False,
                    "words": card_line(last_text(last_assistant)),
                    "reactedAt": at,
                })
        # A stopped turn's reaction is the person's next word.
        for event in reversed(events):
            if event["verb"] == VERB_STOPPED and event["reactedAt"] is None:
                event["reactedAt"] = at
                break
        last_assistant = None
    # The turn the transcript ends on — a turn nobody typed after — rang too.
    if last_assistant is not None:
        ended = parse_ms(last_assistant.get("timestamp"))
        if ended is not None:
            events.append({
                "at": ended,
                "verb": VERB_FINISHED,
                "interrupted": False,
                "words": card_line(last_text(last_assistant)),
                "reactedAt": None,
            })
    events.sort(key=lambda event: event["at"])
    for event in events:
        event["pane"] = pane
        event["agent"] = agent
        event["transcript"] = str(path)
    return events, hands


def reacted(event: dict) -> bool:
    return event["reactedAt"] is not None and 0 <= event["reactedAt"] - event["at"] <= LABEL_WINDOW_MS


def attendance_at(at: int, hands: list[int]) -> str:
    """Present when a hand landed inside the window strictly before `at`."""
    import bisect

    before = bisect.bisect_left(hands, at)
    if before == 0:
        return "away"
    return "present" if at - hands[before - 1] <= ATTENDANCE_WINDOW_MS else "away"


def recent_before(event: dict, earlier: list[dict]) -> list[dict]:
    """The pane's last rings before `event`, as the bell would have known
    them: a ring whose own minute had not closed by then says nothing about
    its reaction."""
    held = []
    for one in earlier[-RECENT_CAP:]:
        closed = event["at"] - one["at"] > LABEL_WINDOW_MS
        held.append({
            "verb": one["verb"],
            "interrupted": one["interrupted"],
            "agoMs": event["at"] - one["at"],
            "reacted": reacted(one) if closed else None,
        })
    return held


def waiting_panes_at(at: int, transcript: str, asks: list[tuple[int, int | None, str]]) -> int:
    """How many OTHER panes stood at an open question at `at`: asked at or
    before, answered after or never."""
    return sum(
        1
        for asked, answered, owner in asks
        if owner != transcript and asked <= at and (answered is None or answered > at)
    )


def transcripts(roots: list[Path], days: int) -> list[Path]:
    since = time.time() - days * 86_400
    found = []
    for root in roots:
        for path in sorted(root.glob("*/*.jsonl")):
            if path.is_file() and path.stat().st_mtime >= since:
                found.append(path)
    return found


def build(roots: list[Path], days: int) -> dict:
    per_pane: dict[str, list[dict]] = {}
    hands: list[int] = []
    for path in transcripts(roots, days):
        events, typed = events_of(path)
        if events:
            per_pane[str(path)] = events
        hands.extend(typed)
    hands.sort()
    asks = [
        (event["at"], event["reactedAt"], transcript)
        for transcript, events in per_pane.items()
        for event in events
        if event["verb"] == VERB_ATTENTION
    ]
    rows = []
    for transcript, events in per_pane.items():
        earlier: list[dict] = []
        for event in events:
            rows.append({
                "at": event["at"],
                "transcript": transcript,
                "pane": event["pane"],
                "agent": event["agent"],
                "verb": event["verb"],
                "interrupted": event["interrupted"],
                "attendance": attendance_at(event["at"], hands),
                "words": event["words"],
                "sinceLastMs": event["at"] - earlier[-1]["at"] if earlier else None,
                "waitingPanes": waiting_panes_at(event["at"], transcript, asks),
                "recent": recent_before(event, earlier),
                "label": {
                    "reacted": reacted(event),
                    "afterMs": (event["reactedAt"] - event["at"]) if event["reactedAt"] is not None else None,
                },
            })
            earlier.append(event)
    rows.sort(key=lambda row: row["at"])
    return {
        "labelWindowMs": LABEL_WINDOW_MS,
        "attendanceWindowMs": ATTENDANCE_WINDOW_MS,
        "recentCap": RECENT_CAP,
        "wordsCharCap": WORDS_CHAR_CAP,
        "days": days,
        "transcripts": len(per_pane),
        "rows": rows,
    }


def default_roots() -> list[Path]:
    home = Path.home()
    held = [
        home / "Library" / "Application Support" / "dev.zerocode.app" / ".claude" / "projects",
        Path(os.environ.get("CLAUDE_CONFIG_DIR", home / ".claude")) / "projects",
    ]
    return [root for root in held if root.is_dir()]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--projects",
        type=Path,
        action="append",
        default=None,
        help="a Claude `projects` directory to read (default: the window's and ~/.claude's)",
    )
    parser.add_argument("--days", type=int, default=14, help="transcripts touched within this many days (default 14)")
    parser.add_argument("--out", type=Path, default=None, help="where to write the seed (default: stdout)")
    args = parser.parse_args()
    roots = [Path(root).expanduser() for root in (args.projects or default_roots())]
    seed = build(roots, args.days)
    rows = seed["rows"]
    by_verb = {}
    for row in rows:
        by_verb[row["verb"]] = by_verb.get(row["verb"], 0) + 1
    print(
        f"{len(rows)} rings from {seed['transcripts']} transcripts over {args.days} days; "
        f"{sum(1 for row in rows if row['label']['reacted'])} reacted inside the minute; "
        f"{sum(1 for row in rows if row['attendance'] == 'present')} present; "
        + ", ".join(f"{verb} {count}" for verb, count in sorted(by_verb.items())),
        file=sys.stderr,
    )
    text = json.dumps(seed, indent=1, ensure_ascii=False) + "\n"
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text)
        print(f"wrote {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
