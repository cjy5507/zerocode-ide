"""What a run's pane children look like on disk — the one reader.

A pane child's directory is named for its agent id and sits beside the
manifests (`tools::misc_tools::agent_tools::panes::child_directory`). Its
`brief.json` says which lane it is, `result.json` when it answered, and
`result-final.json` whether — and why — it left. Both the scripted lane (which
snapshots the store while the parent is still alive) and the analyser (which
reads it afterwards) ask the same question, so they ask it in one place.
"""
import json, os


def pane_children(run_dir):
    found = []
    sandbox = os.path.join(run_dir, "sandbox")
    for base, dirs, files in os.walk(sandbox):
        if "brief.json" not in files:
            continue
        dirs[:] = []
        try:
            with open(os.path.join(base, "brief.json")) as f:
                brief = json.load(f)
        except Exception:
            continue

        def when(name):
            path = os.path.join(base, name)
            return os.stat(path).st_mtime if os.path.exists(path) else None

        final = None
        fp = os.path.join(base, "result-final.json")
        if os.path.exists(fp):
            try:
                with open(fp) as f:
                    final = json.load(f)
            except Exception:
                final = {}
        found.append({
            "dir": base, "agent_id": brief.get("agentId"),
            "prompt": brief.get("prompt", ""), "name": brief.get("name"),
            "wave_index": brief.get("waveIndex"),
            "result_at": when("result.json"), "final_at": when("result-final.json"),
            "channel_at": when("channel.addr"), "brief_at": when("brief.json"),
            "final_reason": (final or {}).get("reason"),
            "final_exit": (final or {}).get("exit"),
        })
    return sorted(found, key=lambda c: c["brief_at"] or 0)


def snapshot(run_dir, path):
    """Freeze the answer while the parent is still there.

    A child that outlives its turn closes itself when its parent goes away
    (`parent_lost`), and the parent goes away when the driver ends the run — so
    a control read AFTER the run would show every teammate as having left. The
    scripted parent takes this snapshot on its last turn instead.
    """
    kids = pane_children(run_dir)
    with open(path, "w") as f:
        json.dump(kids, f, indent=2)
    return kids
