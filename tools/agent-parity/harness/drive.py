#!/usr/bin/env python3
"""Drive one subject (zo | claude) through one axis against the scripted mock."""
import argparse, json, os, pty, re, select, shutil, signal, subprocess, sys, time

ANSI = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][B0]|\x1b[=>]")


def plain(buf):
    return ANSI.sub(b"", bytes(buf))

ROOT = os.environ.get("PARITY_ROOT", "/tmp/zo-agent-parity-20260907")
# Whoever runs this owns the binaries; the environment names the machine,
# not a path written here.
ZO = os.environ.get("PARITY_ZO_BIN", os.path.expanduser("~/.local/bin/zo"))
CLAUDE = os.environ.get("PARITY_CLAUDE_BIN", os.path.expanduser("~/.local/bin/claude"))


def hermetic_env(sandbox, base_url, extra=None):
    env = {}
    keep = ("PATH", "SHELL", "LANG", "LC_ALL", "TERM", "TERMINFO", "TZ")
    for k in keep:
        if k in os.environ:
            env[k] = os.environ[k]
    env.update({
        "HOME": sandbox + "/home", "USERPROFILE": sandbox + "/home",
        "ZO_CONFIG_HOME": sandbox + "/home", "TMPDIR": sandbox + "/state",
        "ZO_DISABLE_MODEL_DISCOVERY": "1",
        "CODEX_HOME": sandbox + "/home/codex", "ZO_CODEX_HOME": sandbox + "/home/codex",
        "CLAUDE_CONFIG_DIR": sandbox + "/home/claude",
        "ZO_SESSION_ROOT": sandbox + "/sessions", "ZO_STATE_DIR": sandbox + "/state",
        "ANTHROPIC_BASE_URL": base_url, "ANTHROPIC_API_KEY": "test-dummy-key",
        "ZO_DISABLE_KEYCHAIN": "1", "ZO_DISABLE_EXTERNAL_CREDENTIALS": "1",
        "TERM": "xterm-256color", "RUST_BACKTRACE": "1",
                "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1",
        "DISABLE_AUTOUPDATER": "1", "DISABLE_BUG_COMMAND": "1",
        "DISABLE_NON_ESSENTIAL_MODEL_CALLS": "1",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    })
    if extra:
        env.update(extra)
    return env


def run_pty(argv, env, cwd, steps, done, timeout, capture_path, rows=40, cols=120):
    """Spawn argv on a pty and walk `steps`, then idle until done() or timeout.

    steps: ("wait", needle, secs) | ("send", bytes) | ("sleep", secs)
    """
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(cwd)
        os.environ.clear()
        os.environ.update(env)
        try:
            os.execv(argv[0], argv)
        finally:
            os._exit(127)
    import fcntl, struct, termios
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    t0 = time.time()
    buf = bytearray()
    alive = [True]

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], min(0.05, max(0.0, end - time.time())))
            if r:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    alive[0] = False
                    return
                if not chunk:
                    alive[0] = False
                    return
                buf.extend(chunk)

    result = "timeout"
    log = []
    for step in steps:
        if not alive[0] or time.time() - t0 > timeout:
            break
        if step[0] == "wait":
            needle, secs = step[1].encode(), step[2]
            end = time.time() + secs
            while time.time() < end and needle not in plain(buf):
                pump(0.05)
                if not alive[0]:
                    break
            log.append({"wait": step[1], "found": needle in plain(buf),
                        "at": round(time.time() - t0, 3)})
        elif step[0] == "ready":
            needle, secs = step[1].encode(), step[2]
            end = time.time() + secs
            escs = 0
            while time.time() < end:
                pump(0.4)
                tail = plain(buf)[-6000:]
                if b"Enter to confirm" in tail or b"Esc to cancel" in tail:
                    os.write(fd, b"\x1b")
                    escs += 1
                    pump(0.6)
                    continue
                if needle in tail:
                    break
            log.append({"ready": step[1], "found": needle in plain(buf)[-6000:],
                        "escapes": escs, "at": round(time.time() - t0, 3)})
        elif step[0] == "send":
            os.write(fd, step[1])
            log.append({"send": step[1].decode("utf-8", "replace")[:80],
                        "at": round(time.time() - t0, 3)})
        elif step[0] == "waitfile":
            path, secs = step[1], step[2]
            end = time.time() + secs
            while time.time() < end and not os.path.exists(path):
                pump(0.2)
            log.append({"waitfile": os.path.basename(path),
                        "found": os.path.exists(path),
                        "at": round(time.time() - t0, 3)})
        elif step[0] == "sleep":
            pump(step[1])
    while alive[0] and time.time() - t0 < timeout:
        if done():
            result = "done"
            break
        pump(0.1)
    if not alive[0] and result == "timeout":
        result = "exited"
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.time() + 6
    while time.time() < deadline:
        try:
            wpid, _ = os.waitpid(pid, os.WNOHANG)
            if wpid:
                break
        except ChildProcessError:
            break
        pump(0.1)
    else:
        try:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        except Exception:
            pass
    try:
        os.close(fd)
    except OSError:
        pass
    with open(capture_path, "wb") as f:
        f.write(bytes(buf))
    return result, time.time() - t0, log


def seed_claude(sandbox, cwd):
    """Skip Claude Code's first-run onboarding so the pty run reaches a prompt."""
    cfgdir = os.path.join(sandbox, "home", "claude")
    os.makedirs(cfgdir, exist_ok=True)
    key = "test-dummy-key"
    cfg = {
        "numStartups": 20, "installMethod": "native", "autoUpdates": False,
        "theme": "dark", "hasCompletedOnboarding": True, "lastOnboardingVersion": "2.1.263",
        "firstStartTime": "2026-01-01T00:00:00.000Z",
        "hasSeenTasksHint": True, "bypassPermissionsModeAccepted": True,
        "fullscreenUpsellSeenCount": 99, "fullscreenDownsellSeenCount": 99,
        "fullscreenBootStrikes": 0, "fullscreenBootPending": False,
        "hasUsedBackgroundTask": True, "hasAcknowledgedCostThreshold": True,
        "subscriptionNoticeCount": 99, "hasAvailableSubscription": False,
        "customApiKeyResponses": {"approved": [key[-20:], key], "rejected": []},
        "projects": {},
    }
    entry = {"allowedTools": [], "hasTrustDialogAccepted": True,
             "hasCompletedProjectOnboarding": True,
             "projectOnboardingSeenCount": 3, "dontCrawlDirectory": True,
             "hasClaudeMdExternalIncludesApproved": True,
             "hasClaudeMdExternalIncludesWarningShown": True,
             "history": []}
    for path in {cwd, os.path.realpath(cwd), os.path.abspath(cwd),
                 sandbox, os.path.realpath(sandbox)}:
        cfg["projects"][path] = dict(entry)
    with open(os.path.join(cfgdir, ".claude.json"), "w") as f:
        json.dump(cfg, f)
    # Some builds still read $HOME/.claude.json; write both.
    with open(os.path.join(sandbox, "home", ".claude.json"), "w") as f:
        json.dump(cfg, f)
    with open(os.path.join(cfgdir, "settings.json"), "w") as f:
        json.dump({"includeCoAuthoredBy": False,
                   "worktree": {"baseRef": "head"}}, f)


# ───────────────────────────────────────────────────────────── the pane lane
# The window's own evidence. `runtime::subagent_panes::TeamEvidence::can_split`
# wants a multiplexer, a pane, a team and a capability together; strip any of
# them and the mode falls back to inline, so a "panes" measurement would
# silently measure threads instead. The rest are what the window's `tmux`
# shim itself reads to reach the events channel.
TEAM_VARS = ("TMUX", "TMUX_PANE", "ZEROCODE_AGENT_TEAM_ID", "ZEROCODE_AGENT_TEAM_PANE",
             "ZEROCODE_AGENT_TEAM_LEADER", "ZEROCODE_AGENT_TEAM_TOKEN",
             "ZEROCODE_AGENT_TEAM_TOKEN_FILE", "ZEROCODE_HOOK_PORT",
             "ZEROCODE_HOOK_TOKEN", "ZEROCODE_HOOK_ENDPOINT", "ZEROCODE_HOOK_VERSION",
             "ZEROCODE_PANE_KEY", "ZEROCODE_MIRROR_DIR")


def pane_lane(run_dir, sandbox, base_url):
    """Env additions that make sub-agents real panes of this window.

    Two halves. The window's team evidence travels through untouched, so the
    parent's `tmux` is the window's shim and the pane is cut by the same events
    channel a person's fan-out uses. But a pane child does NOT inherit the
    parent's environment — the window hands every pane its own (measured: a
    variable exported beside the shim never reaches the pane) — so the child
    would read the real HOME and call the real provider. A wrapper in front of
    `tmux` rewrites `split-window … -- <program> …` into
    `… -- /usr/bin/env <this run's vars> <program> …`, which is the only place
    the child's environment can be stated from out here.
    """
    missing = [name for name in ("TMUX", "TMUX_PANE", "ZEROCODE_AGENT_TEAM_ID")
               if not os.environ.get(name)]
    if missing:
        print("panes lane needs the window's evidence; missing: %s" % ", ".join(missing),
              file=sys.stderr)
        sys.exit(3)
    real = shutil.which("tmux")
    if not real:
        print("panes lane needs the window's `tmux` shim on PATH", file=sys.stderr)
        sys.exit(3)
    child_env = {
        "HOME": sandbox + "/home", "USERPROFILE": sandbox + "/home",
        "ZO_CONFIG_HOME": sandbox + "/home", "TMPDIR": sandbox + "/state",
        "CODEX_HOME": sandbox + "/home/codex", "ZO_CODEX_HOME": sandbox + "/home/codex",
        "CLAUDE_CONFIG_DIR": sandbox + "/home/claude",
        "ZO_SESSION_ROOT": sandbox + "/sessions", "ZO_STATE_DIR": sandbox + "/state",
        "ANTHROPIC_BASE_URL": base_url, "ANTHROPIC_API_KEY": "test-dummy-key",
        "ZO_DISABLE_KEYCHAIN": "1", "ZO_DISABLE_EXTERNAL_CREDENTIALS": "1",
        "ZO_DISABLE_MODEL_DISCOVERY": "1", "RUST_BACKTRACE": "1",
    }
    bindir = os.path.join(run_dir, "bin")
    os.makedirs(bindir, exist_ok=True)
    wrapper = os.path.join(bindir, "tmux")
    with open(wrapper, "w") as f:
        f.write("#!/usr/bin/env python3\n"
                "# Generated by drive.py --panes. Only `split-window` is touched.\n"
                "import os, sys\n"
                "REAL = %r\n"
                "CHILD = %r\n"
                "argv = sys.argv[1:]\n"
                "if argv[:1] == ['split-window'] and '--' in argv:\n"
                "    cut = argv.index('--') + 1\n"
                "    argv = argv[:cut] + ['/usr/bin/env'] + \\\n"
                "        ['%%s=%%s' %% pair for pair in sorted(CHILD.items())] + argv[cut:]\n"
                "os.execv(REAL, [REAL] + argv)\n"
                % (real, child_env))
    os.chmod(wrapper, 0o755)
    passthrough = {name: os.environ[name] for name in TEAM_VARS if os.environ.get(name)}
    passthrough["PATH"] = bindir + os.pathsep + os.environ.get("PATH", "")
    passthrough["ZO_SUBAGENT_MODE"] = "panes"
    return passthrough


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--subject", required=True, choices=["zo", "claude"])
    ap.add_argument("--axis", required=True)
    ap.add_argument("--run", required=True)
    ap.add_argument("--prompt", required=True)
    ap.add_argument("--timeout", type=float, default=120)
    ap.add_argument("--mode", default="pty", choices=["pty", "headless"])
    ap.add_argument("--wire-agent", action="store_true")
    ap.add_argument("--panes", action="store_true",
                    help="run sub-agents as PANES of the installed ZeroCode window: "
                         "keep the window/team evidence so `tmux` reaches the events "
                         "channel, and put a wrapper in front of it that hands the "
                         "child this run's hermetic environment.")
    ap.add_argument("--repo", default="")
    ap.add_argument("--fixture", default="")
    ap.add_argument("--wt", default="")
    ap.add_argument("--env", action="append", default=[])
    ap.add_argument("--settle", type=float, default=3.0)
    ap.add_argument("--interrupt-on", default="")
    ap.add_argument("--followup", default="")
    args = ap.parse_args()

    run_dir = os.path.join(ROOT, "runs", args.run)
    if os.path.isdir(run_dir):
        shutil.rmtree(run_dir)
    os.makedirs(run_dir)
    sandbox = os.path.join(run_dir, "sandbox")
    for sub in ("home", "home/claude", "home/codex", "sessions", "state", "cwd"):
        os.makedirs(os.path.join(sandbox, sub), exist_ok=True)
    stamps = os.path.join(run_dir, "stamps")
    os.makedirs(stamps, exist_ok=True)

    mock_env = dict(os.environ)
    mock_env.update({"PARITY_ROOT": ROOT, "PARITY_RUN": args.run,
                     "PARITY_STAMPS": stamps, "PARITY_RAW": "1",
                     "PARITY_SUBJECT": args.subject})
    if args.repo:
        mock_env["PARITY_REPO"] = args.repo
    if args.fixture:
        mock_env["PARITY_FIXTURE"] = args.fixture
    if args.wt:
        mock_env["PARITY_WT"] = args.wt
    mock = subprocess.Popen([sys.executable, os.path.join(ROOT, "harness", "mock.py")],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            env=mock_env, text=True)
    line = mock.stdout.readline().strip()
    if not line.startswith("PORT="):
        print("mock failed:", line, mock.stdout.read())
        sys.exit(2)
    base = "http://127.0.0.1:" + line.split("=")[1]
    reqlog = os.path.join(run_dir, "requests.jsonl")
    open(reqlog, "a").close()

    marker = "PARENT DONE " + args.axis
    state = {"done_at": None}

    def done():
        try:
            with open(reqlog) as f:
                lines = f.readlines()
        except OSError:
            return False
        # the parent's terminal request is the one whose reply is the marker;
        # the mock logs before replying, so settle after the last request.
        for ln in lines:
            try:
                rec = json.loads(ln)
            except Exception:
                continue
            if rec.get("final_marker") == marker and state["done_at"] is None:
                state["done_at"] = time.time()
        if state["done_at"] and time.time() - state["done_at"] > args.settle:
            return True
        return False

    cwd = args.repo or os.path.join(sandbox, "cwd")
    extra = {"PARITY_RUN": args.run}
    for pair in args.env:
        k, _, v = pair.partition("=")
        extra[k] = v
    if args.wire_agent:
        extra["ZO_WIRE_TOOLS"] = "Agent,SpawnMultiAgent,SendMessage,StopAgent,ListAgents,EnterWorktree,ExitWorktree,Sleep,Monitor,ScheduleWakeup"
    env = hermetic_env(sandbox, base, extra)
    if args.panes:
        env.update(pane_lane(run_dir, sandbox, base))
    capture = os.path.join(run_dir, "%s.capture" % args.subject)

    if args.subject == "claude":
        seed_claude(sandbox, cwd)
    if args.subject == "zo":
        if args.mode == "headless":
            argv = [ZO, "--plain", "--json", "--permission-mode", "danger-full-access"]
        else:
            argv = [ZO, "--permission-mode", "danger-full-access"]
    else:
        env["CLAUDE_CONFIG_DIR"] = sandbox + "/home/claude"
        if args.mode == "headless":
            argv = [CLAUDE, "-p", "--output-format", "stream-json",
                    "--verbose", "--dangerously-skip-permissions", args.prompt]
        else:
            argv = [CLAUDE, "--dangerously-skip-permissions"]

    if args.mode == "pty":
        ready = "Ask zo to do anything" if args.subject == "zo" else "shift+tab to cycle"
        steps = [("ready", ready, 45.0), ("sleep", 1.5),
                 ("send", args.prompt.encode()), ("sleep", 0.8), ("send", b"\r")]
        if args.interrupt_on:
            steps += [("waitfile", os.path.join(stamps, args.interrupt_on), 90.0),
                      ("sleep", 2.0), ("send", b"\x1b"), ("sleep", 3.0)]
        if args.followup:
            steps += [("send", args.followup.encode()), ("sleep", 0.8),
                      ("send", b"\r")]
    elif args.subject == "zo":
        steps = [("send", args.prompt.encode() + b"\n"), ("sleep", 0.3), ("send", b"\x04")]
    else:
        steps = []

    t_start = time.time()
    outcome, elapsed, steplog = run_pty(argv, env, cwd, steps, done, args.timeout, capture)
    mock.send_signal(signal.SIGTERM)
    try:
        mock.wait(timeout=5)
    except subprocess.TimeoutExpired:
        mock.kill()
    meta = {"subject": args.subject, "axis": args.axis, "run": args.run,
            "argv": argv, "outcome": outcome, "elapsed_s": elapsed,
            "t_start": t_start, "mode": args.mode, "cwd": cwd,
            "prompt": args.prompt, "wire_agent": args.wire_agent,
            "panes": args.panes,
            "steps": steplog}
    with open(os.path.join(run_dir, "driver.json"), "w") as f:
        json.dump(meta, f, indent=2)
    print(json.dumps({"outcome": outcome, "elapsed_s": round(elapsed, 2),
                      "run_dir": run_dir}))


if __name__ == "__main__":
    main()
