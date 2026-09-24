#!/usr/bin/env python3
"""Choose recent zo transcripts containing successful file edits.

This seed picker records local transcript/workspace locators and counts edit
results. It does not copy prompts, tool output, or edited-file paths into the
seed. The Rust replay loads each transcript and asks the production file-pick
seat at each eligible user turn, then labels it with that turn's actual edits.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time
from pathlib import Path


def load_picker(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(name, module)
    spec.loader.exec_module(module)
    return module


_TOOLS = Path(__file__).resolve().parents[1]
picker = load_picker("file_pick_compaction_replay_seed", _TOOLS / "compaction-replay" / "seed.py")
patch_picker = load_picker("file_pick_patch_review_replay_seed", _TOOLS / "patch-review-replay" / "seed.py")
session_picker = load_picker("file_pick_mention_replay_seed", _TOOLS / "mention-rerank-replay" / "seed.py")

SEED_LOOKBACK_DAYS = 7
EDIT_TOOL = patch_picker.is_edit


def started_at_ms(path: Path) -> int | None:
    slug = path.name.removeprefix("session-").split("-", 1)[0]
    try:
        return int(slug)
    except ValueError:
        return None


def describe(path: Path, cwd: Path) -> dict | None:
    rows = picker.read_rows(path)
    edit_results = 0
    messages = 0
    model = None
    for row in rows:
        if row.get("type") != "message":
            continue
        messages += 1
        message = row.get("message") or {}
        model = model or message.get("model")
        for block in message.get("blocks") or []:
            if (
                block.get("type") == "tool_result"
                and not block.get("is_error")
                and EDIT_TOOL(block.get("tool_name") or "")
            ):
                edit_results += 1
    if edit_results == 0:
        return None
    return {
        "path": str(path),
        "messages": messages,
        "model": model,
        "edit_results": edit_results,
        "cwd": str(cwd),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--project",
        type=Path,
        action="append",
        help="a projects directory to read (default: $ZO_STATE_DIR/projects or ~/.zo/projects)",
    )
    parser.add_argument("--days", type=int, default=SEED_LOOKBACK_DAYS)
    parser.add_argument("--out", type=Path, help="write the seed here instead of stdout")
    args = parser.parse_args()

    state_root = Path(os.environ.get("ZO_STATE_DIR", Path.home() / ".zo")).expanduser()
    roots = args.project or [state_root / "projects"]
    cutoff_ms = int((time.time() - max(1, args.days) * 24 * 60 * 60) * 1_000)
    selected = []
    for path in picker.transcripts([root.expanduser() for root in roots if root.is_dir()]):
        started = started_at_ms(path)
        if started is None or started < cutoff_ms:
            continue
        cwd = session_picker.zo_cwd(path)
        if cwd is None or not cwd.is_dir():
            continue
        row = describe(path, cwd)
        if row is not None:
            selected.append(row)
    selected.sort(key=lambda row: row["path"])
    seed = {"lookbackDays": max(1, args.days), "transcripts": selected}
    rendered = json.dumps(seed, indent=1) + "\n"
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(rendered)
        print(f"{len(selected)} transcripts with successful edits -> {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
