"""Per-axis scripted responses. One script, two subjects (see adapt.py)."""
import calendar, os, re, time
from mock import (script, tools_sse, text_sse, tool_results,
                  assistant_tool_uses, bump, peek, user_prompt_text)
from adapt import call, calls, SUBJECT
from panes_on_disk import snapshot

STAMPS = os.environ.get("PARITY_STAMPS", "/tmp/zo-agent-parity-20260907/stamps")
os.makedirs(STAMPS, exist_ok=True)


def stamp(name):
    return "perl -MTime::HiRes=time -e 'printf \"%%.6f\\n\", time' > %s/%s" % (STAMPS, name)


def used(body, *names):
    want = set(names)
    return sum(1 for u in assistant_tool_uses(body) if u[0] in want)


def results_of(body, *names):
    want = set(names)
    by_id = {u[1]: u[0] for u in assistant_tool_uses(body)}
    return [r for r in tool_results(body) if by_id.get(r[0]) in want]


def child(tid, tag, prompt, axis, **kw):
    return call(tid, "agent", description="parity child %s" % tag,
                prompt="PARITYCHILD:%s.%s %s" % (axis, tag, prompt), **kw)


BASH = ("bash", "Bash")
READ = ("read_file", "Read")
AGENT = ("Agent",)
SLEEPT = ("Sleep",) if SUBJECT == "zo" else ("Bash",)
SEND = ("SendMessage",)
ROSTER = ("ListAgents",)
WTIN = ("EnterWorktree",)
WTOUT = ("ExitWorktree",)


# ───────────────────────────────────────────────────────────── A spawn latency
@script("A")
def axis_a(body, text, tag, meta):
    if tag:
        if used(body, *BASH) == 0:
            return tools_sse("m_a_child", calls(call(
                "t_a_bash", "bash", command=stamp("A_%s.first_tool" % tag),
                timeout=10000, description="stamp")))
        return text_sse("m_a_child_done", "CHILD ANSWER %s ready\n" % tag)
    if used(body, *AGENT) == 0:
        return tools_sse("m_a_parent", calls(child(
            "t_a_agent", "a1", "run your stamp tool once, then answer ready.",
            "A", background=False)))
    return text_sse("m_a_parent_done", "PARENT DONE A\n")


# ───────────────────────────────────────────────────────── B fan-out accuracy
B_Q = {"b1": ("7*6", "42"), "b2": ("11*11", "121"),
       "b3": ("15+27", "42"), "b4": ("100-37", "63")}


@script("B")
def axis_b(body, text, tag, meta):
    if tag:
        if used(body, *BASH) == 0:
            return tools_sse("m_b_%s" % tag, calls(call(
                "t_b_%s" % tag, "bash",
                command="%s ; sleep 1" % stamp("B_%s.start" % tag),
                timeout=30000, description="work")))
        return text_sse("m_b_%s_done" % tag,
                        "CHILD ANSWER %s equals %s\n" % (tag, B_Q[tag][1]))
    if used(body, *AGENT) == 0:
        return tools_sse("m_b_parent", calls(*[child(
            "t_b_%s" % k, k, "compute %s and answer with the number." % B_Q[k][0],
            "B", background=False) for k in ("b1", "b2", "b3", "b4")]))
    seen = sorted(set("%s=%s" % (k, v) for k, v in B_Q.items()
                      if re.search(r"%s equals %s\b" % (k, v[1]), text)))
    meta["b_seen"] = len(seen)
    meta["b_tags"] = seen
    waits = used(body, *SLEEPT)
    if len(seen) >= 4 or waits >= 25:
        return text_sse("m_b_done", "PARENT DONE B seen=%d\n" % len(seen))
    return tools_sse("m_b_wait_%d" % waits,
                     calls(call("t_b_sleep_%d" % waits, "sleep", ms=800)))


# ─────────────────────────────────────────────────────────── C completion loss
C_N = int(os.environ.get("PARITY_C_N", "20"))


@script("C")
def axis_c(body, text, tag, meta):
    if tag:
        return text_sse("m_c_%s" % tag, "CDONE %s\n" % tag)
    if used(body, *AGENT) == 0:
        return tools_sse("m_c_parent", calls(*[child(
            "t_c_%02d" % i, "c%02d" % i, "answer immediately with your tag.",
            "C", background=True) for i in range(C_N)]))
    seen = sorted(set(re.findall(r"CDONE (c\d\d)", text)))
    meta["c_seen"] = len(seen)
    meta["c_tags"] = seen
    waits = used(body, *SLEEPT)
    if len(seen) >= C_N or waits >= 30:
        return text_sse("m_c_done", "PARENT DONE C seen=%d\n" % len(seen))
    return tools_sse("m_c_wait_%d" % waits,
                     calls(call("t_c_sleep_%d" % waits, "sleep", ms=800)))


# ────────────────────────────────────────────────────────── D message receipts
@script("D")
def axis_d(body, text, tag, meta):
    if tag:
        if tag == "dbusy":
            steered = "PARITY steer one" in text
            meta["d_child_saw_steer"] = steered
            if used(body, *BASH) == 0:
                return tools_sse("m_d_busy", calls(call(
                    "t_d_busy", "bash", command="sleep 4", timeout=40000,
                    description="park")))
            return text_sse("m_d_busy_done",
                            "CHILD ANSWER dbusy parked steer=%s\n" % steered)
        return text_sse("m_d_%s" % tag, "CHILD ANSWER %s\n" % tag)
    n_agent, n_send = used(body, *AGENT), used(body, *SEND)
    if n_agent == 0:
        return tools_sse("m_d_spawn_busy", calls(child(
            "t_d_busy_agent", "dbusy", "park on your tool, then answer.",
            "D", background=True, name="parityBusy")))
    if n_agent == 1:
        return tools_sse("m_d_spawn_short", calls(child(
            "t_d_short_agent", "dshort", "answer immediately.",
            "D", background=False, name="parityShort")))
    if n_send == 0:
        return tools_sse("m_d_send_running", calls(call(
            "t_d_send1", "send", to="parityBusy", message="PARITY steer one")))
    if n_send == 1:
        return tools_sse("m_d_send_dead", calls(call(
            "t_d_send2", "send", to="parityGhostNobody",
            message="PARITY steer two")))
    if n_send == 2:
        return tools_sse("m_d_send_finished", calls(call(
            "t_d_send3", "send", to="parityShort", message="PARITY steer three")))
    if used(body, *SLEEPT) < 2:
        n = used(body, *SLEEPT)
        return tools_sse("m_d_settle_%d" % n,
                         calls(call("t_d_settle_%d" % n, "sleep", ms=4000)))
    if used(body, *ROSTER) == 0:
        return tools_sse("m_d_roster", calls(call("t_d_roster", "roster")))
    meta["d_steer_seen"] = "steer=True" in text
    return text_sse("m_d_done", "PARENT DONE D\n")


# ───────────────────────────────────────────────── F background notifications
F_N = int(os.environ.get("PARITY_F_N", "20"))


@script("F")
def axis_f(body, text, tag, meta):
    bg = used(body, *BASH)
    if bg == 0:
        return tools_sse("m_f_spawn", calls(*[call(
            "t_f_%02d" % i, "bash",
            command="sleep 0.4; %s; echo FDONE f%02d" % (stamp("F_f%02d.exit" % i), i),
            background=True, timeout=60000, description="bg %d" % i)
            for i in range(F_N)]))
    seen = sorted(set(re.findall(r"FDONE (f\d\d)", text)))
    meta["f_seen"] = len(seen)
    meta["f_tags"] = seen
    waits = used(body, *SLEEPT) - (F_N if SUBJECT != "zo" else 0)
    waits = max(0, waits)
    if len(seen) >= F_N or waits >= 30:
        return text_sse("m_f_done", "PARENT DONE F seen=%d\n" % len(seen))
    return tools_sse("m_f_wait_%d" % waits,
                     calls(call("t_f_sleep_%d" % waits, "sleep", ms=800)))


# ─────────────────────────────────────────────────────── I worktree isolation
@script("I")
def axis_i(body, text, tag, meta):
    repo = os.environ["PARITY_REPO"]
    wt = os.environ.get("PARITY_WT", repo + "-wt/parity-i")
    if used(body, *WTIN) == 0:
        if SUBJECT == "zo":
            inp = {"path": wt, "branch": "parity-i"}
        else:
            inp = {"name": "parity-i"}
        return tools_sse("m_i_enter", [("t_i_enter", "EnterWorktree", inp)])
    if used(body, *BASH) == 0:
        cmd = ("pwd > %s/I.cwd && printf 'parity edit\\n' >> parity.txt && "
               "git add parity.txt && "
               "git -c user.email=p@x -c user.name=parity commit -q -m 'parity edit' && "
               "git rev-parse HEAD > %s/I.commit && "
               "git rev-parse --abbrev-ref HEAD > %s/I.branch"
               % (STAMPS, STAMPS, STAMPS))
        return tools_sse("m_i_edit", calls(call("t_i_edit", "bash", command=cmd,
                                                timeout=60000, description="edit+commit")))
    if used(body, *WTOUT) == 0:
        inp = {} if SUBJECT == "zo" else {"action": "keep"}
        return tools_sse("m_i_exit", [("t_i_exit", "ExitWorktree", inp)])
    if used(body, *BASH) == 1:
        cmd = ("cd %s && git status --porcelain > %s/I.main_status; "
               "git rev-parse --abbrev-ref HEAD >> %s/I.main_status; "
               "git log --oneline -1 >> %s/I.main_status; "
               "git log --all --oneline >> %s/I.main_status"
               % (repo, STAMPS, STAMPS, STAMPS, STAMPS))
        return tools_sse("m_i_check", calls(call("t_i_check", "bash", command=cmd,
                                                 timeout=60000, description="check main")))
    return text_sse("m_i_done", "PARENT DONE I\n")


# ───────────────────────────────────────────────────────────── J fork fidelity
J_QS = "what is the SECRET word; what is the COUNT number; what is the COLOR"


@script("J")
def axis_j(body, text, tag, meta):
    fixture = os.environ["PARITY_FIXTURE"]
    if tag == "jfork":
        meta["j_fork_tools"] = used(body, *BASH, *READ)
        meta["j_fork_answer"] = fork_answer(text)
        return text_sse("m_j_fork", "CHILD ANSWER jfork %s\n" % fork_answer(text))
    if tag == "jplain":
        if used(body, *READ) == 0:
            meta["j_plain_blind"] = fork_answer(text)
            return tools_sse("m_j_plain_read", calls(call(
                "t_j_read", "read", path=fixture)))
        return text_sse("m_j_plain", "CHILD ANSWER jplain %s\n" % fork_answer(text))
    if used(body, *READ) == 0:
        return tools_sse("m_j_pread", calls(call("t_j_pread", "read", path=fixture)))
    if used(body, *AGENT) == 0:
        return tools_sse("m_j_spawn", calls(
            child("t_j_fork", "jfork", "Answer from context only, no tools: " + J_QS,
                  "J", subagent_type="fork", background=False),
            child("t_j_plain", "jplain", "Answer these: " + J_QS,
                  "J", subagent_type="general-purpose", background=False)))
    return text_sse("m_j_done", "PARENT DONE J\n")


def fork_answer(text):
    out = []
    for key in ("SECRET", "COUNT", "COLOR"):
        m = re.search(key + r"=([A-Za-z0-9_\-]+)", text)
        out.append("%s=%s" % (key, m.group(1) if m else "UNKNOWN"))
    return " ".join(out)


# ───────────────────────────────────────────────────────── G workflow resume
SENTINEL = os.environ.get("PARITY_SENTINEL", STAMPS)


def touch(name):
    open(os.path.join(SENTINEL, name), "a").close()


def zo_spec():
    return {"name": "parity-g", "description": "three phases",
            "mode": "phases",
            "phases": [
                {"id": "g1", "prompt": "PARITYCHILD:G.g1 answer ok",
                 "model": "claude-opus-5"},
                {"id": "g2", "prompt": "PARITYCHILD:G.g2 answer ok",
                 "model": "claude-opus-5"},
                {"id": "g3", "prompt": "PARITYCHILD:G.g3 answer ok",
                 "model": "claude-opus-5"},
            ]}


CC_SCRIPT = """export const meta = {
  name: 'parity-g',
  description: 'three phases for the t-2877 resume axis',
  phases: [{ title: 'One' }, { title: 'Two' }, { title: 'Three' }],
}
const a = await agent('PARITYCHILD:G.g1 answer ok', {label: 'g1', phase: 'One'})
const b = await agent('PARITYCHILD:G.g2 answer ok', {label: 'g2', phase: 'Two'})
const c = await agent('PARITYCHILD:G.g3 answer ok', {label: 'g3', phase: 'Three'})
return { a, b, c }
"""


@script("G")
def axis_g(body, text, tag, meta):
    if tag:
        n = int(tag[1])
        if n == 3:
            if used(body, *BASH) == 0:
                touch("G_g3_started")
                return tools_sse("m_g_g3", calls(call(
                    "t_g_g3", "bash", command="sleep 40", timeout=120000,
                    description="long phase")))
            return text_sse("m_g_g3_done", "PHASE ok g3\n")
        return text_sse("m_g_%s" % tag, "PHASE ok %s\n" % tag)
    runs = used(body, "Workflow")
    wf_results = [r[1] for r in results_of(body, "Workflow")]
    last_wf = wf_results[-1] if wf_results else ""
    run_id = None
    for _, txt, _ in tool_results(body):
        if SUBJECT == "claude":
            m = re.search(r"(wf_[a-z0-9][a-z0-9\-]{5,})", txt)
        else:
            m = re.search(r'run_?[iI]d"?\s*[:=]\s*"?([A-Za-z0-9_\-]{6,})', txt)
        if m:
            run_id = m.group(1)
    meta["g_run_id"] = run_id
    meta["g_workflow_calls"] = runs
    spec = {"spec": zo_spec()} if SUBJECT == "zo" else {"script": CC_SCRIPT}
    if runs == 0:
        return tools_sse("m_g_run1", [("t_g_run1", "Workflow", dict(spec))])
    if SUBJECT == "zo" and run_id is None and used(body, "WorkflowRuns") == 0:
        return tools_sse("m_g_runs", [("t_g_runs", "WorkflowRuns", {"limit": 5})])
    stop = re.search(r'TaskStop\(\{\s*taskId:\s*\\?"(\w+)', last_wf)
    if stop and used(body, "TaskStop") == 0:
        meta["g_task_stop"] = stop.group(1)
        return tools_sse("m_g_stop", [("t_g_stop", "TaskStop",
                                       {"task_id": stop.group(1)})])
    blocked = "tool_use_error" in last_wf or "still running" in last_wf
    if runs < 3 and (runs == 1 or blocked):
        inp = dict(spec)
        if run_id:
            inp["resumeFromRunId"] = run_id
        meta["g_resume_used"] = bool(run_id)
        return tools_sse("m_g_run%d" % (runs + 1),
                         [("t_g_run%d" % (runs + 1), "Workflow", inp)])
    return text_sse("m_g_done", "PARENT DONE G\n")


# ─────────────────────────────────────────────────────────────── H scheduling
H_MARK = "PARITYFIRE"


@script("H")
def axis_h(body, text, tag, meta):
    submitted = user_prompt_text(body)
    fires = len(re.findall(H_MARK + r"-(\w+)", submitted))
    meta["h_fires"] = fires
    if fires:
        meta["h_kinds"] = sorted(set(re.findall(H_MARK + r"-(\w+)", submitted)))
        return text_sse("m_h_fire", "FIRE SEEN\n")
    n_cron = used(body, "CronCreate")
    n_wake = used(body, "ScheduleWakeup")
    if n_cron == 0:
        # WHICH clock the expression is written in is itself a measurement.
        # A person writes local time (`PARITY_H_CRON_TZ=local`, the 09-06 run);
        # `utc` writes the same instant the way zo currently judges it, which
        # separates "the scheduler does not fire" from "the timezone is off".
        wall = time.time() + 75
        zone = os.environ.get("PARITY_H_CRON_TZ", "local")
        if zone == "utc":
            broken = time.gmtime(wall)
            target = calendar.timegm((broken.tm_year, broken.tm_mon, broken.tm_mday,
                                      broken.tm_hour, broken.tm_min, 0, 0, 0, 0))
        else:
            broken = time.localtime(wall)
            target = time.mktime((broken.tm_year, broken.tm_mon, broken.tm_mday,
                                  broken.tm_hour, broken.tm_min, 0, 0, 0, -1))
        cronexpr = "%d %d %d %d *" % (broken.tm_min, broken.tm_hour,
                                      broken.tm_mday, broken.tm_mon)
        meta["h_cron_expr"] = cronexpr
        meta["h_cron_zone"] = zone
        meta["h_cron_target"] = target
        if SUBJECT == "zo":
            inp = {"schedule": cronexpr, "prompt": H_MARK + "-cron say fired",
                   "description": "parity cron"}
        else:
            inp = {"cron": cronexpr, "prompt": H_MARK + "-cron say fired",
                   "recurring": False}
        return tools_sse("m_h_cron", [("t_h_cron", "CronCreate", inp)])
    if n_wake == 0:
        meta["h_wake_at"] = time.time() + 45
        inp = {"delaySeconds": 45, "reason": "parity wakeup probe",
               "prompt": H_MARK + "-wakeup say fired"}
        if SUBJECT != "zo":
            inp["noop"] = False
        return tools_sse("m_h_wake", [("t_h_wake", "ScheduleWakeup", inp)])
    return text_sse("m_h_armed", "ARMED H\n")


# ───────────────────────────────────────────── H-loop: fixed interval firing
@script("L")
def axis_l(body, text, tag, meta):
    """A fixed-interval loop: zo's own `/loop every 1m` engine vs Claude
    Code's `/loop <interval>`, which schedules through CronCreate."""
    submitted = user_prompt_text(body)
    ticks = len(re.findall(r"PARITYAXIS:L", submitted))
    meta["l_ticks_submitted"] = ticks
    if SUBJECT != "zo" and used(body, "CronCreate") == 0:
        return tools_sse("m_l_cron", [("t_l_cron", "CronCreate", {
            "cron": "* * * * *", "prompt": "PARITYAXIS:L loop tick",
            "recurring": True})])
    return text_sse("m_l_tick_%d" % ticks, "LOOP TICK %d\n" % ticks)


# ─────────────────────────── K first-delegation cost (deferred vs advertised)
@script("K")
def axis_k(body, text, tag, meta):
    """What it costs to reach the FIRST Agent call in a fresh session.

    The model's path follows the wire it is shown, so one script measures
    both products and both zo builds: when `Agent` is advertised the first
    call is direct (Claude Code, and zo after t-2903); when it is deferred
    (zo r49..310f200a) the model must ToolSearch the schema and then call
    through CapabilityInvoke.  PARITY_K_MODE=blind is the probe for the
    runtime half of option (b): CapabilityInvoke(Agent) with no ToolSearch
    first — it shows whether the second turn is the runtime's or the model's.
    """
    if tag:
        if used(body, *BASH) == 0:
            return tools_sse("m_k_child", calls(call(
                "t_k_bash", "bash", command=stamp("K_%s.first_tool" % tag),
                timeout=10000, description="stamp")))
        return text_sse("m_k_child_done", "CHILD ANSWER %s ready\n" % tag)
    advertised = [t for t in (body.get("tools") or [])]
    names = [t.get("name") for t in advertised]
    meta["k_agent_advertised"] = "Agent" in names
    meta["k_wire_tools"] = len(names)
    meta["k_mode"] = os.environ.get("PARITY_K_MODE", "model")
    if "Agent" not in names:
        blind = meta["k_mode"] == "blind"
        if not blind and used(body, "ToolSearch") == 0:
            return tools_sse("m_k_search", [("t_k_search", "ToolSearch", {
                "query": "select:Agent", "max_results": 3})])
        if used(body, "CapabilityInvoke") == 0:
            return tools_sse("m_k_invoke", [("t_k_invoke", "CapabilityInvoke", {
                "name": "Agent", "input": {
                    "description": "parity child k1",
                    "subagent_type": "general-purpose", "background": False,
                    "prompt": "PARITYCHILD:K.k1 run your stamp tool, then answer ready."}})])
        return text_sse("m_k_done", "PARENT DONE K\n")
    if used(body, *AGENT) == 0:
        return tools_sse("m_k_agent", calls(child(
            "t_k_agent", "k1", "run your stamp tool once, then answer ready.",
            "K", background=False)))
    return text_sse("m_k_done", "PARENT DONE K\n")


# ───────────────────────────── M lane release (995711d9): a fan-out lane leaves
# One `SpawnMultiAgent` of four pane lanes, then ONE lone `Agent` as the
# control. The parent NEVER calls `StopAgent`: whether the four panes are
# released is the product's answer, not the script's.
M_LANES = int(os.environ.get("PARITY_M_LANES", "4"))
M_SETTLE = int(os.environ.get("PARITY_M_SETTLE", "4"))


@script("M")
def axis_m(body, text, tag, meta):
    if tag:
        return text_sse("m_m_%s" % tag, "CHILD ANSWER %s\n" % tag)
    if used(body, "SpawnMultiAgent") == 0:
        members = [{"prompt": "PARITYCHILD:M.m%d answer immediately with your tag." % i,
                    "description": "parity lane m%d" % i,
                    "subagent_type": "general-purpose",
                    "name": "parityLane%d" % i}
                   for i in range(1, M_LANES + 1)]
        return tools_sse("m_m_fan", [("t_m_fan", "SpawnMultiAgent",
                                      {"agents": members})])
    if used(body, *AGENT) == 0:
        return tools_sse("m_m_lone", calls(child(
            "t_m_lone", "mlone", "answer immediately with your tag.", "M",
            background=False, name="parityLone")))
    waits = used(body, *SLEEPT)
    if waits < M_SETTLE:
        # Each settle asks for a DIFFERENT duration: zo refuses a third
        # identical tool call in one turn ("Repeating the same call will not
        # produce a different result"), which ended the parent's turn before
        # the snapshot and timed the run out (M-zo, first attempt).
        return tools_sse("m_m_settle_%d" % waits,
                         calls(call("t_m_settle_%d" % waits, "sleep",
                                    ms=2000 + 300 * waits)))
    meta["m_stop_calls"] = used(body, "StopAgent")
    kids = snapshot(os.path.dirname(STAMPS), os.path.join(STAMPS, "M.snapshot.json"))
    meta["m_pane_children"] = len(kids)
    meta["m_lanes_answered"] = sorted(set(re.findall(r"CHILD ANSWER (m\w+)", text)))
    return text_sse("m_m_done", "PARENT DONE M\n")
