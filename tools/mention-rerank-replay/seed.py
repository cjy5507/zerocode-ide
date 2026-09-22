#!/usr/bin/env python3
"""Pick the prompts on this machine in which a person named a file of their
own working tree, and say where each one is.

This script **only reads files and picks**. It builds no page, runs no fuzzy
search, cuts no sentence and asks nothing: the page is
`runtime::file_search::run` and the question is
`smart_router::mention_rerank::judge`, and the replay that uses this seed
hands each prompt to those functions as they ship
(`mention_rerank::tests::the_pages_this_machine_would_have_reranked`). A second
copy of any of that in Python would be a second rule, and the one under test
would stop being the one that ships.

What a seed row carries is a transcript's path, the line of the prompt in
it, the working directory the prompt was made in, and the relative path the
person named. No words: nothing a person typed is read into the seed. The
replay reads the prompt's text itself, at that line, and derives the intent
(the prompt without the named path) and the token typed (the head of the
file's name — its length is the replay's own knob, printed with the table).

Two kinds of transcript are read, because both hold prompts a person wrote
about a file of a tree they were working in:

- zo's, `~/.zo/projects/*/sessions/session-*.jsonl`, whose working directory
  is the `.cwd` sidecar beside the transcript;
- Claude Code's, `~/.claude/projects/*/*.jsonl`, whose `user` rows carry
  `cwd` themselves.

A prompt counts when it holds a relative path — a word with a separator and
an extension, not a URL, not absolute — that names a file which exists under
the prompt's working directory now. A prompt the harness wrote (a
continuation summary, a tool result quoted back) is not a person's.

    tools/mention-rerank-replay/seed.py                  # both stores under $HOME
    tools/mention-rerank-replay/seed.py --out seed.json
    tools/mention-rerank-replay/seed.py --zo ~/.zo/projects --claude ~/.claude/projects
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

# `MENTION_CANDIDATE_CAP` in `crates/zerocode-core/src/jev.rs` — one page.
PAGE_ROWS = 8

# Sidecars that share zo's transcript extension and are not transcripts.
SIDECAR_MARKS = (".vault.jsonl", ".rot-", ".todos.json", ".prefs.json")

# A prompt the harness wrote rather than the person.
NOT_A_PERSONS = ("This session is being continued", "<", "[Request interrupted")

# A word that names a relative file: a separator, an extension, no scheme.
PATH_WORD = re.compile(r"^[A-Za-z0-9_.@-][A-Za-z0-9_./@-]*/[A-Za-z0-9_./@-]+\.[A-Za-z0-9]{1,8}$")


def is_zo_transcript(path: Path) -> bool:
    name = path.name
    return name.startswith("session-") and name.endswith(".jsonl") and not any(mark in name for mark in SIDECAR_MARKS)


def read_rows(path: Path):
    """The transcript's records with their line numbers. A torn line is skipped, never fatal."""
    with path.open(errors="replace") as handle:
        for line_no, line in enumerate(handle):
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                yield line_no, row


def zo_cwd(path: Path) -> Path | None:
    sidecar = path.with_suffix(".cwd")
    try:
        raw = json.loads(sidecar.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    cwd = raw.get("cwd") if isinstance(raw, dict) else None
    return Path(cwd) if isinstance(cwd, str) and cwd.strip() else None


def zo_prompts(path: Path):
    """`(line, text)` of every prompt a person typed into a zo session."""
    for line_no, row in read_rows(path):
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "user":
            continue
        for block in message.get("blocks") or []:
            if block.get("type") == "text" and isinstance(block.get("text"), str):
                yield line_no, block["text"]


def claude_prompts(path: Path):
    """`(line, cwd, text)` of every prompt a person typed into a Claude Code session."""
    for line_no, row in read_rows(path):
        if row.get("type") != "user":
            continue
        cwd = row.get("cwd")
        if not isinstance(cwd, str):
            continue
        content = (row.get("message") or {}).get("content")
        texts = []
        if isinstance(content, str):
            texts = [content]
        elif isinstance(content, list):
            texts = [block.get("text") for block in content if isinstance(block, dict) and block.get("type") == "text"]
        for text in texts:
            if isinstance(text, str):
                yield line_no, Path(cwd), text


def a_persons(text: str) -> bool:
    head = text.lstrip()
    return bool(head) and not any(head.startswith(mark) for mark in NOT_A_PERSONS)


def named_files(text: str, cwd: Path) -> list[str]:
    """The relative paths in `text`, as typed, that name a file under `cwd` now."""
    found = []
    for word in text.split():
        word = word.strip("`\"'(),:;<>[]").lstrip("@")
        if not PATH_WORD.match(word) or word.startswith(("http", "./", "../")):
            continue
        if word in found:
            continue
        if (cwd / word).is_file():
            found.append(word)
    return found


def pick(zo_roots: list[Path], claude_roots: list[Path]) -> list[dict]:
    rows = []
    seen = set()
    for root in zo_roots:
        for path in sorted(root.glob("*/sessions/session-*.jsonl")):
            if not is_zo_transcript(path) or path.resolve() in seen:
                continue
            seen.add(path.resolve())
            cwd = zo_cwd(path)
            if cwd is None or not cwd.is_dir():
                continue
            for line_no, text in zo_prompts(path):
                if not a_persons(text):
                    continue
                for mention in named_files(text, cwd):
                    rows.append({"store": "zo", "path": str(path), "line": line_no, "cwd": str(cwd), "mention": mention})
    for root in claude_roots:
        for path in sorted(root.glob("*/*.jsonl")):
            if path.resolve() in seen:
                continue
            seen.add(path.resolve())
            for line_no, cwd, text in claude_prompts(path):
                if not cwd.is_dir() or not a_persons(text):
                    continue
                for mention in named_files(text, cwd):
                    rows.append({"store": "claude", "path": str(path), "line": line_no, "cwd": str(cwd), "mention": mention})
    return rows


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    home = Path.home()
    parser.add_argument("--zo", action="append", type=Path, help="a zo projects root (default ~/.zo/projects)")
    parser.add_argument("--claude", action="append", type=Path, help="a Claude Code projects root (default ~/.claude/projects)")
    parser.add_argument("--out", type=Path, help="write the seed here instead of stdout")
    args = parser.parse_args(argv)
    zo_roots = args.zo if args.zo is not None else [home / ".zo" / "projects"]
    claude_roots = args.claude if args.claude is not None else [home / ".claude" / "projects"]
    rows = pick([root for root in zo_roots if root.is_dir()], [root for root in claude_roots if root.is_dir()])
    seed = {"pageRows": PAGE_ROWS, "prompts": rows}
    rendered = json.dumps(seed, indent=1, ensure_ascii=False) + "\n"
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(rendered)
        print(f"{len(rows)} prompts naming a file of their own tree -> {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(rendered)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
