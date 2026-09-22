#!/usr/bin/env python3
"""Pick the transcripts on this machine whose patches the review seat can be
replayed on, and count how many patches each one wrote.

This script **only reads files and counts**. It builds no question, reads no
patch and grades nothing: what a review is asked (the person's words, the
hunks, the evidence's tail) is `runtime::patch_review::ask_for`, what became
of a patch is `runtime::patch_review::hindsight_of_turn`, and the replay that
uses this seed hands each transcript to those functions as they ship
(`smart_router::patch_review::tests::the_patches_this_machine_wrote_reviewed_in_hindsight`).
A second copy of either rule in Python would be a second rule, and the one
under test would stop being the one that ships.

What a seed row carries is a path, a message count, the model the transcript
ran on, how many successful edit results it holds and whether a vault sits
beside it. No words: nothing a person typed or a tool printed is read into
the seed.

The replay asks each review from the messages before the patch's own result
and reads the turns after it only as the label — the same lines edited again
inside `PATCH_REVIEW_REGRET_TURNS` turns is regret, a check green after the
turn's last edit is a patch that stood — so a replay row never sees what its
own clock had not reached. The seed carries that window so a replay made for
another one is refused.

Which files are transcripts is compaction-replay's reading (`is_transcript`,
`transcripts`, `read_rows`), loaded from there rather than copied.

    tools/patch-review-replay/seed.py                      # every project under ~/.zo
    tools/patch-review-replay/seed.py --out seed.json
    tools/patch-review-replay/seed.py --project ~/.zo/projects
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
from pathlib import Path

_PICKER_PATH = Path(__file__).resolve().parents[1] / "compaction-replay" / "seed.py"
_spec = importlib.util.spec_from_file_location("compaction_replay_seed", _PICKER_PATH)
picker = importlib.util.module_from_spec(_spec)
sys.modules.setdefault(_spec.name, picker)
_spec.loader.exec_module(picker)

# `PATCH_REVIEW_REGRET_TURNS` in `crates/zerocode-core/src/jev.rs` — the
# window the label reads after a patch.
REGRET_TURNS = 5

# `EDIT_RESULT_TOOL_NAMES` in `zo-ide/crates/runtime/src/compact/mod.rs`: the
# tools whose result records a file mutation.
EDIT_RESULT_TOOL_NAMES = ("Edit", "MultiEdit", "Write", "NotebookEdit", "edit_file", "write_file")

# `EDIT_RESULT_TOOL_LEAF_VERBS` in the same file: a namespaced tool
# (`mcp__<server>__write_file`) whose leaf is one of these is a mutation too.
EDIT_RESULT_TOOL_LEAF_VERBS = ("write_file", "edit_file")


def is_edit(tool_name: str) -> bool:
    """The runtime's `is_edit_result_tool`, as a count needs it."""
    if tool_name in EDIT_RESULT_TOOL_NAMES:
        return True
    leaf = tool_name.rsplit("__", 1)[-1]
    return leaf != tool_name and leaf in EDIT_RESULT_TOOL_LEAF_VERBS


def edits(rows: list[dict]) -> int:
    """Successful results of a mutation tool — the patches a review would have
    been asked about, at most (the replay asks only the ones that wrote a
    patch, after the person had said something)."""
    count = 0
    for row in rows:
        if row.get("type") != "message":
            continue
        for block in (row.get("message") or {}).get("blocks") or []:
            if block.get("type") == "tool_result" and not block.get("is_error") and is_edit(block.get("tool_name") or ""):
                count += 1
    return count


def describe(path: Path) -> dict | None:
    rows = picker.read_rows(path)
    written = edits(rows)
    if written == 0:
        return None
    model = None
    for row in rows:
        if row.get("type") == "message":
            model = (row.get("message") or {}).get("model")
            if model:
                break
    return {
        "path": str(path),
        "messages": sum(1 for row in rows if row.get("type") == "message"),
        "model": model,
        "edits": written,
        "vault": path.with_name(path.name[: -len(".jsonl")] + ".vault.jsonl").is_file(),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--project",
        type=Path,
        action="append",
        default=None,
        help="a projects directory to read (default: $ZO_STATE_DIR or ~/.zo/projects)",
    )
    parser.add_argument("--out", type=Path, default=None, help="where to write the seed (default: stdout)")
    args = parser.parse_args()

    roots = args.project or [Path(os.environ.get("ZO_STATE_DIR", Path.home() / ".zo")) / "projects"]
    held = []
    for path in picker.transcripts([Path(root).expanduser() for root in roots]):
        row = describe(path)
        if row is not None:
            held.append(row)
    held.sort(key=lambda row: (-row["edits"], row["path"]))
    print(
        f"{len(held)} transcripts with an edit from {len(roots)} root(s); "
        f"{sum(row['edits'] for row in held)} edit results, {sum(1 for row in held if row['vault'])} with a vault",
        file=sys.stderr,
    )
    seed = {"regretTurns": REGRET_TURNS, "transcripts": held}
    text = json.dumps(seed, indent=1) + "\n"
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text)
        print(f"wrote {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
