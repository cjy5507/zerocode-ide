#!/usr/bin/env python3
"""Turn one run directory into axis metrics + one baseline-schema runs.jsonl row.

Fields follow docs/design/agent-workflow-quality-baseline.md §2.
"""
import json, os, platform, re, statistics, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from panes_on_disk import pane_children  # noqa: E402

ROOT = os.environ.get("PARITY_ROOT", "/tmp/zo-agent-parity-20260907")


def load(run_dir):
    reqs = []
    p = os.path.join(run_dir, "requests.jsonl")
    if os.path.exists(p):
        for ln in open(p):
            try:
                reqs.append(json.loads(ln))
            except Exception:
                pass
    drv = {}
    d = os.path.join(run_dir, "driver.json")
    if os.path.exists(d):
        drv = json.load(open(d))
    stamps = {}
    sd = os.path.join(run_dir, "stamps")
    if os.path.isdir(sd):
        for name in os.listdir(sd):
            try:
                stamps[name] = float(open(os.path.join(sd, name)).read().strip().split()[0])
            except Exception:
                stamps[name] = None
    return reqs, drv, stamps


def user_blob(rec):
    """Only what the MODEL was told: every non-assistant-tool_use payload.

    Deliberately excludes assistant `tool_use` inputs, which echo the very
    command strings the markers live in — counting those would score a spawn
    as a delivery.
    """
    out = list(rec.get("tool_results", []))
    raw = rec.get("raw")
    if not raw:
        return "\n".join(out)
    try:
        body = json.loads(raw)
    except Exception:
        return "\n".join(out)
    sysm = body.get("system")
    if isinstance(sysm, str):
        out.append(sysm)
    elif isinstance(sysm, list):
        for b in sysm:
            if isinstance(b, dict) and b.get("type") == "text":
                out.append(b.get("text", ""))
    for m in body.get("messages", []):
        c = m.get("content")
        if isinstance(c, str):
            out.append(c)
        elif isinstance(c, list):
            for b in c:
                t = b.get("type")
                if t == "text" and m.get("role") != "assistant":
                    out.append(b.get("text", ""))
                elif t == "tool_result":
                    out.append(json.dumps(b.get("content")))
    return "\n".join(out)


def parents(reqs):
    return [r for r in reqs if r.get("kind") == "parent"]


def children(reqs):
    return [r for r in reqs if r.get("kind") == "child"]


def first_where(rows, pred):
    for r in rows:
        if pred(r):
            return r
    return None


# ─────────────────────────────────────────────────────────────── per-axis math
def axis_A(reqs, drv, stamps, run_dir=None):
    ps = parents(reqs)
    spawn = first_where(ps, lambda r: r.get("final_marker") is None and not r["tool_uses"])
    if spawn is None:
        return {"error": "no parent spawn request"}
    t_flush = spawn["t_out"]
    cs = children(reqs)
    if not cs:
        return {"error": "no child request", "t_flush": t_flush}
    t_childreq = min(c["t_in"] for c in cs)
    tool = stamps.get("A_a1.first_tool")
    out = {"spawn_to_child_request_ms": round((t_childreq - t_flush) * 1000, 1),
           "child_requests": len(cs)}
    if tool:
        out["spawn_to_first_tool_ms"] = round((tool - t_flush) * 1000, 1)
    if drv.get("panes") and run_dir:
        kids = pane_children(run_dir)
        out["pane_children"] = len(kids)
        if kids:
            child_dir = kids[0]
            for label, key in (("brief", "brief_at"), ("channel", "channel_at"),
                               ("result", "result_at"), ("final", "final_at")):
                stamp = child_dir.get(key)
                out["spawn_to_%s_ms" % label] = (round((stamp - t_flush) * 1000, 1)
                                                 if stamp else None)
            out["pane_final_reason"] = child_dir.get("final_reason")
    return out


def axis_B(reqs, drv, stamps):
    starts = {k: v for k, v in stamps.items() if k.startswith("B_") and v}
    cs = children(reqs)
    firsts = {}
    for c in cs:
        tag = c.get("child")
        if tag and tag not in firsts:
            firsts[tag] = c["t_in"]
    span = (max(firsts.values()) - min(firsts.values())) if len(firsts) > 1 else None
    correct, waits = 0, 0
    for r in parents(reqs):
        correct = max(correct, r.get("b_seen", 0))
    waits = sum(1 for r in parents(reqs) if r.get("b_seen") is not None) - 1
    return {"children_started": len(firsts), "parent_polls": max(0, waits),
            "first_request_span_s": round(span, 3) if span is not None else None,
            "tool_start_span_s": (round(max(starts.values()) - min(starts.values()), 3)
                                  if len(starts) > 1 else None),
            "answers_matched": correct, "answers_expected": 4,
            "parallel": (span is not None and span < 0.9)}


def axis_C(reqs, drv, stamps, n=20):
    ps = parents(reqs)
    seen = 0
    tags = []
    for r in ps:
        if "c_seen" in r:
            seen = max(seen, r["c_seen"])
            tags = r.get("c_tags", tags)
    spawned = len(set(c.get("child") for c in children(reqs) if c.get("child")))
    # per-completion delay: child's own final flush → first parent request carrying it
    child_done = {}
    for c in children(reqs):
        tag = c.get("child")
        if tag:
            child_done[tag] = max(child_done.get(tag, 0), c["t_out"])
    arrival = {}
    for r in ps:
        for tag in set(re.findall(r"CDONE (c\d\d)", user_blob(r))):
            arrival.setdefault(tag, r["t_in"])
    delays = [arrival[t] - child_done[t] for t in arrival if t in child_done]
    return {"spawned": spawned, "delivered": seen, "expected": n,
            "missing": n - seen, "tags": tags,
            "delivery_delay_median_s": round(statistics.median(delays), 3) if delays else None,
            "delivery_delay_max_s": round(max(delays), 3) if delays else None}


def axis_D(reqs, drv, stamps):
    ps = parents(reqs)
    last = ps[-1] if ps else None
    receipts, roster = {}, None
    if last and last.get("raw"):
        body = json.loads(last["raw"])
        uses = {}
        for m in body["messages"]:
            if isinstance(m.get("content"), list):
                for b in m["content"]:
                    if b.get("type") == "tool_use":
                        uses[b["id"]] = (b["name"], b.get("input"))
        for m in body["messages"]:
            if isinstance(m.get("content"), list):
                for b in m["content"]:
                    if b.get("type") != "tool_result":
                        continue
                    name, inp = uses.get(b["tool_use_id"], ("?", {}))
                    txt = json.dumps(b.get("content"))
                    if name == "SendMessage":
                        m2 = re.search(r'\\"receipt\\":\s*\\"(\w+)', txt)
                        receipts[(inp or {}).get("to", b["tool_use_id"])] = (
                            m2.group(1) if m2 else "none")
                    if name == "ListAgents":
                        roster = txt
    kinds = set(receipts.values())
    last_receipts = re.findall(r'lastReceipt\\":\s*\\"(\w+)', roster or "")
    kinds |= set(last_receipts)
    steer_seen = any(c.get("d_child_saw_steer") for c in children(reqs))
    return {"receipts_by_target": receipts, "roster_last_receipts": last_receipts,
            "child_saw_steer": steer_seen,
            "kinds_seen": sorted(k for k in kinds if k in
                                 ("consumed", "queued", "rejected")),
            "kinds_expected": 3}


def axis_F(reqs, drv, stamps, n=20):
    ps = parents(reqs)
    subject = drv.get("subject")
    exits = {k.split("_")[1].split(".")[0]: v for k, v in stamps.items()
             if k.startswith("F_") and v}
    arrival = {}
    for r in ps:
        blob = user_blob(r)
        if subject == "claude":
            for block in re.findall(r"<task-notification>(.*?)</task-notification>",
                                    blob, re.S):
                if "<status>completed</status>" not in block:
                    continue
                m = re.search(r"<tool-use-id>t_f_(\d\d)</tool-use-id>", block)
                if m:
                    arrival.setdefault("f" + m.group(1), r["t_in"])
        else:
            for tag in set(re.findall(r"FDONE (f\d\d)", blob)):
                arrival.setdefault(tag, r["t_in"])
    delays = [arrival[t] - exits[t] for t in arrival if t in exits]
    inlined = 0
    for r in ps:
        inlined = max(inlined, r.get("f_seen", 0))
    return {"started": len(exits), "notified": len(arrival), "expected": n,
            "missing": n - len(arrival), "output_inlined": inlined,
            "delay_median_s": round(statistics.median(delays), 3) if delays else None,
            "delay_max_s": round(max(delays), 3) if delays else None}


def axis_I(reqs, drv, stamps, run_dir=None):
    sd = os.path.join(run_dir or "", "stamps")

    def read(name):
        p = os.path.join(sd, name)
        return open(p).read().strip() if os.path.exists(p) else None
    cwd, commit, branch = read("I.cwd"), read("I.commit"), read("I.branch")
    main_status = read("I.main_status")
    repo = drv.get("cwd", "")
    moved = bool(cwd and os.path.realpath(cwd) != os.path.realpath(repo))
    clean = bool(main_status is not None and main_status.splitlines()
                 and main_status.splitlines()[0].strip() == "")
    lines = (main_status or "").splitlines()
    return {"entered_worktree": moved, "worktree_cwd": cwd, "commit": commit,
            "branch": branch, "main_status_raw": main_status,
            "main_clean": (main_status is not None and
                           not any(ln and not ln.startswith(("main", "parity")) and
                                   re.match(r"^[ MADRCU?!]{2} ", ln) for ln in lines)),
            "pass": bool(moved and commit and branch and branch != "main")}


def axis_J(reqs, drv, stamps):
    ps, cs = parents(reqs), children(reqs)
    fork = [c for c in cs if c.get("child") == "jfork"]
    plain = [c for c in cs if c.get("child") == "jplain"]
    got = {}
    for c in fork:
        if "j_fork_answer" in c:
            got = {"answer": c["j_fork_answer"], "tools_before": c.get("j_fork_tools")}
    want = {"SECRET": "ossifrage", "COUNT": "8317", "COLOR": "vermilion"}
    ans = got.get("answer", "")
    correct = sum(1 for k, v in want.items() if "%s=%s" % (k, v) in ans)
    blind = None
    for c in plain:
        if "j_plain_blind" in c:
            blind = c["j_plain_blind"]
    return {"fork_requests": len(fork), "fork_answer": ans,
            "fork_correct": correct, "fork_expected": 3,
            "fork_tool_calls": max(0, len(fork) - 1),
            "plain_blind_answer": blind, "plain_requests": len(plain)}


def axis_G(reqs, drv, stamps):
    """Phase agents that ran before vs after the resume call."""
    ps = parents(reqs)
    wf_times = []
    for r in ps:
        n = r.get("g_workflow_calls")
        if n is not None:
            wf_times.append((n, r["t_out"]))
    # the moment the RESUME call went out = t_out of the request whose reply
    # carried the second Workflow tool_use
    resume_at = None
    for n, t in wf_times:
        if n >= 1 and resume_at is None and any(x[0] == 0 for x in wf_times):
            resume_at = t
            break
    before, after = {}, {}
    for c in children(reqs):
        tag = c.get("child")
        if not tag:
            continue
        bucket = after if (resume_at and c["t_in"] > resume_at) else before
        bucket[tag] = bucket.get(tag, 0) + 1
    resumed_note = None
    for r in ps:
        for res in r.get("tool_results", []):
            m = re.search(r"resumed (\d+) phase\(s\) from cache", res)
            if m:
                resumed_note = int(m.group(1))
            if "agents_spawned" in res:
                m2 = re.findall(r'"agents_spawned":\s*(\d+)', res)
                if m2:
                    resumed_note = resumed_note if resumed_note is not None else None
    stops = sum(1 for r in ps if r.get("g_task_stop"))
    completed_before = sorted(k for k in before if k in ("g1", "g2"))
    reran = sorted(k for k in completed_before if after.get(k))
    return {"phase_requests_before_resume": before,
            "phase_requests_after_resume": after,
            "completed_steps_rerun": len(reran), "rerun_tags": reran,
            "resumed_from_cache_note": resumed_note,
            "extra_calls_needed": stops,
            "run_id": next((r.get("g_run_id") for r in reversed(ps)
                            if r.get("g_run_id")), None)}


def axis_H(reqs, drv, stamps):
    ps = parents(reqs)
    target = next((r["h_cron_target"] for r in ps if r.get("h_cron_target")), None)
    fires = [(r["t_in"], r.get("h_kinds")) for r in ps if r.get("h_fires")]
    kinds = sorted({k for _, ks in fires for k in (ks or [])})
    # The error belongs to the CRON's own arrival. Two schedules are armed in
    # one window, and reading `fires[0]` scored a wakeup that landed on time as
    # the cron's error even when no cron ever fired (H-zo, 2026-09-07).
    cron_at = next((t for t, ks in fires if "cron" in (ks or [])), None)
    wake_at = next((t for t, ks in fires if "wakeup" in (ks or [])), None)
    # When the wakeup was ASKED for, and for how long: both products clamp the
    # request to 60 s, so "fired on time" has to be read against the clamp and
    # against what the receipt itself announced, not against the 45 s asked.
    asked = next((r.get("h_wake_at") for r in ps if r.get("h_wake_at")), None)
    armed = (asked - 45) if asked else None
    return {"cron_target_epoch": target,
            "cron_expr": next((r.get("h_cron_expr") for r in ps if r.get("h_cron_expr")), None),
            "cron_zone": next((r.get("h_cron_zone") for r in ps if r.get("h_cron_zone")), None),
            "cron_fired": "cron" in kinds,
            "cron_error_s": (round(cron_at - target, 2)
                             if (target and cron_at) else None),
            "wakeup_fired": "wakeup" in kinds,
            "wakeup_delay_s": (round(wake_at - armed, 2)
                               if (wake_at and armed) else None),
            "wakeup_error_vs_asked_s": (round(wake_at - asked, 2)
                                        if (wake_at and asked) else None),
            "window_s": round(drv.get("elapsed_s", 0), 1),
            "fires": len(fires),
            # The run always ends at the window's edge (`--timeout`), so the
            # driver's outcome says nothing. This does.
            "pass": bool(cron_at and wake_at and target
                         and abs(cron_at - target) <= 5)}


def axis_L(reqs, drv, stamps):
    ps = [r for r in parents(reqs) if r.get("l_ticks_submitted")]
    times = [r["t_in"] for r in ps]
    seen, uniq = set(), []
    for r in ps:
        n = r["l_ticks_submitted"]
        if n not in seen:
            seen.add(n)
            uniq.append((n, r["t_in"]))
    gaps = [round(uniq[i][1] - uniq[i - 1][1], 2) for i in range(1, len(uniq))]
    errs = [round(g - 60, 2) for g in gaps]
    steady = errs[1:] if len(errs) > 1 else errs
    return {"ticks": len(uniq), "expected": 5, "interval_s": gaps,
            "interval_error_s": errs,
            "window_s": round(drv.get("elapsed_s", 0), 1),
            # The first gap is the loop's phase, not its interval (see
            # `table.cell_H3`); only the steady ones are judged.
            "pass": bool(len(uniq) >= 5 and steady
                         and max(abs(e) for e in steady) <= 5)}


def axis_K(reqs, drv, stamps):
    ps = parents(reqs)
    cs = children(reqs)
    if not ps:
        return {"error": "no parent request"}
    t_first = ps[0]["t_in"]
    turns = len(ps)
    adv = next((r.get("k_agent_advertised") for r in ps
                if r.get("k_agent_advertised") is not None), None)
    wire = next((r.get("k_wire_tools") for r in ps
                 if r.get("k_wire_tools") is not None), None)
    spawn_req = None
    for r in ps:
        if r.get("final_marker"):
            break
        spawn_req = r
    out = {"agent_advertised_on_wire": adv, "wire_tool_count": wire,
           "parent_turns_before_child": None, "first_prompt_to_child_ms": None}
    if cs:
        t_child = min(c["t_in"] for c in cs)
        out["first_prompt_to_child_ms"] = round((t_child - t_first) * 1000, 1)
        out["parent_turns_before_child"] = sum(1 for r in ps
                                               if r["t_in"] < t_child)
    tool = stamps.get("K_k1.first_tool")
    if tool:
        out["first_prompt_to_first_tool_ms"] = round((tool - t_first) * 1000, 1)
    return out


# ───────────────────────── the pane children (see panes_on_disk.py)
def axis_M(reqs, drv, stamps, run_dir=None):
    """Lane release (995711d9): the four lanes leave, the lone teammate stays."""
    snap = os.path.join(run_dir or "", "stamps", "M.snapshot.json")
    if os.path.exists(snap):
        with open(snap) as f:
            kids = json.load(f)
    else:
        kids = pane_children(run_dir or "")
    lanes = [c for c in kids if re.search(r"PARITYCHILD:M\.m[0-9]", c["prompt"])]
    lone = [c for c in kids if "PARITYCHILD:M.mlone" in c["prompt"]]
    answered = [c["result_at"] for c in lanes if c["result_at"]]
    left = [c["final_at"] for c in lanes if c["final_at"]]
    release_s = (round(max(left) - max(answered), 3)
                 if answered and len(left) == len(lanes) and lanes else None)
    reasons = sorted({c["final_reason"] for c in lanes})
    stops = max((r.get("m_stop_calls") or 0) for r in parents(reqs)) \
        if any("m_stop_calls" in r for r in parents(reqs)) else None
    return {
        "lanes": len(lanes), "lanes_answered": len(answered), "lanes_left": len(left),
        "release_s": release_s,
        "per_lane_release_s": sorted(round(c["final_at"] - c["result_at"], 3)
                                     for c in lanes
                                     if c["final_at"] and c["result_at"]),
        "lane_reasons": reasons,
        "all_lane_done": bool(lanes) and reasons == ["lane_done"],
        "stop_agent_calls": stops,
        "control_children": len(lone),
        "control_answered": bool(lone and lone[0]["result_at"]),
        "control_still_seated": bool(lone and lone[0]["final_at"] is None),
        "control_has_channel": bool(lone and lone[0]["channel_at"]),
        "pass": bool(lanes and reasons == ["lane_done"] and release_s is not None
                     and release_s <= 3.0 and stops == 0
                     and lone and lone[0]["final_at"] is None),
    }


AXES = {"A": axis_A, "M": axis_M, "G": axis_G, "H": axis_H, "L": axis_L, "K": axis_K, "B": axis_B, "C": axis_C, "D": axis_D, "F": axis_F,
        "I": axis_I, "J": axis_J}


def analyse(run_dir):
    reqs, drv, stamps = load(run_dir)
    axis = drv.get("axis") or os.path.basename(run_dir).split("-")[0]
    fn = AXES.get(axis)
    if fn is None:
        metrics = {"error": "no analyser for axis %s" % axis}
    elif axis in ("I", "M") or (axis == "A" and drv.get("panes")):
        metrics = fn(reqs, drv, stamps, run_dir=run_dir)
    else:
        metrics = fn(reqs, drv, stamps)
    return axis, drv, reqs, metrics


def main():
    rows = []
    for run in sorted(sys.argv[1:]):
        run_dir = run if os.path.isabs(run) else os.path.join(ROOT, "runs", run)
        axis, drv, reqs, metrics = analyse(run_dir)
        rows.append({"run": os.path.basename(run_dir), "axis": axis,
                     "subject": drv.get("subject"), "outcome": drv.get("outcome"),
                     "elapsed_s": round(drv.get("elapsed_s", 0), 2),
                     "requests": len(reqs), "metrics": metrics,
                     "evidence": os.path.join(run_dir, "requests.jsonl")})
    print(json.dumps(rows, indent=2))


if __name__ == "__main__":
    main()
