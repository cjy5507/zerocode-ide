"""Canonical tool calls → the subject CLI's own tool names and argument shapes.

The SAME scripted intent has to reach two different agents. Everything the
scripts emit is canonical; this table is the only place either product's
vocabulary appears.
"""
import os

SUBJECT = os.environ.get("PARITY_SUBJECT", "zo")

NAMES = {
    "zo": {"bash": "bash", "read": "read_file", "agent": "Agent", "sleep": "Sleep",
           "send": "SendMessage", "roster": "ListAgents", "stop": "StopAgent",
           "wt_enter": "EnterWorktree", "wt_exit": "ExitWorktree",
           "fanout": "SpawnMultiAgent"},
    "claude": {"bash": "Bash", "read": "Read", "agent": "Agent", "sleep": "Bash",
               "send": "SendMessage", "roster": "ListAgents", "stop": None,
               "wt_enter": "EnterWorktree", "wt_exit": "ExitWorktree",
               "fanout": None},
}


def call(tid, kind, **kw):
    """Return (tool_id, name, input) for this subject, or None if unsupported."""
    name = NAMES[SUBJECT].get(kind)
    if name is None:
        return None
    if kind == "bash":
        inp = {"command": kw["command"], "description": kw.get("description", "step")}
        if kw.get("timeout"):
            inp["timeout"] = kw["timeout"]
        if kw.get("background"):
            inp["run_in_background"] = True
        return (tid, name, inp)
    if kind == "read":
        key = "path" if SUBJECT == "zo" else "file_path"
        return (tid, name, {key: kw["path"]})
    if kind == "sleep":
        if SUBJECT == "zo":
            return (tid, name, {"duration_ms": kw["ms"]})
        return (tid, name, {"command": "sleep %.2f" % (kw["ms"] / 1000.0),
                            "description": "wait", "timeout": 60000})
    if kind == "agent":
        inp = {"description": kw.get("description", "parity child"),
               "prompt": kw["prompt"],
               "subagent_type": kw.get("subagent_type", "general-purpose")}
        if kw.get("name"):
            inp["name"] = kw["name"]
        if SUBJECT == "zo" and "background" in kw:
            inp["background"] = kw["background"]
        return (tid, name, inp)
    if kind == "send":
        return (tid, name, {"to": kw["to"], "message": kw["message"]})
    if kind == "roster":
        return (tid, name, {"format": "json"} if SUBJECT == "zo" else {})
    if kind == "wt_enter":
        return (tid, name, {"name": kw["name"]} if SUBJECT == "zo"
                else {"branch": kw["name"]})
    if kind == "wt_exit":
        return (tid, name, {})
    raise KeyError(kind)


def calls(*items):
    return [c for c in items if c is not None]
