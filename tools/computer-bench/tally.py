#!/usr/bin/env python3
"""Tally Computer Use runs from their evidence folders (docs/design/computer-use-bench.md §2).

Usage: tally.py <evidence-root> [--bench] [--json] [--markdown] [--stages] [--reflex]
                [--baseline FILE | --versus BASE:CONFIG] [--write-baseline FILE]

The root is walked for folders holding steps.jsonl. A bench run's own
bench-run.json names its scenario, lane and config and holds the oracle's
verdict; any other folder is scored from its files as before (qa-verdict.json,
state.json). --bench scores only folders that hold bench-run.json. --versus
compares one config with another that took turns with it in the same runs
(an A/B), per scenario and pooled over every scenario. A reflex run's own
reflex-run.json (fixture_reflex.py) is its row as the fixture's oracle judged
it; --reflex prints those rows' table. Nothing here
needs the window: files only. This module is also the bench's one reader of a
shim answer's envelope (first_json_line, refusal_code).
"""
import json
import math
import os
import re
import math
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))

# The product's own words (test_tally pins each to its Rust home).
STEPS = "steps.jsonl"
VERDICT = "qa-verdict.json"
STATE = "state.json"
BENCH_RUN = "bench-run.json"
# A reflex run's verdict and numbers, as fixture_reflex.py judged them.
REFLEX_RUN = "reflex-run.json"
# The walk report the one evidence writer leaves beside steps.jsonl (walk-NNN.json):
# per step the phases the run closure timed, per walk resolve/report.
WALK_PREFIX = "walk-"
WALK_SUFFIX = ".json"
STEP_STAGES = ("act", "settle", "verify", "pointer")
RUN_STAGES = ("resolve", "report")
HANDOFF_VERB = "handoff"
CONFIRMING_FLAG = "--confirming"
CONFIRM_CODES = {"confirmation_required", "confirmation_refused", "confirmation_timeout"}
STOP_CODES = {"stopped", "budget_exceeded"}
PERSON_ASKED_CODE = "person_asked"
PERSONS_STOP_REASONS = {"hotkey", "window"}
# The runner's name for a stop the model took itself (its own step log shows
# the verb): that run is judged, and failed. Every other stop — the person's,
# a signal to the runner, a chord that went deaf, a budget — is not the
# model's doing, and its run is not judged.
MODEL_STOP_PREFIX = "model:"
CODE_PATTERN = re.compile(r'"code"\s*:\s*"([a-z_]+)"')


def table(path=None):
    """bench.json's values, by name."""
    with open(path or os.path.join(HERE, "bench.json"), encoding="utf-8") as handle:
        raw = json.load(handle)
    return {name: entry["value"] for name, entry in raw.items() if isinstance(entry, dict) and "value" in entry}


def first_json_line(text):
    """The first line of a shim answer that is a JSON object, parsed."""
    for line in (text or "").splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict):
                return value
    return None


def refusal_code(row):
    """A step's refusal code: the row's own `code`, else the envelope in its
    error, else a code cut short at the window's length, else None."""
    if row.get("code"):
        return row["code"]
    error = str(row.get("error") or "")
    envelope = first_json_line(error)
    if envelope and isinstance(envelope.get("error"), dict) and envelope["error"].get("code"):
        return envelope["error"]["code"]
    match = CODE_PATTERN.search(error)
    return match.group(1) if match else None


def read_steps(folder):
    return read_lines(folder, STEPS)


def read_lines(folder, name):
    """A folder's JSON-lines file, one object a line; a broken line is skipped
    and a missing file is empty."""
    rows = []
    try:
        with open(os.path.join(folder, name), encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(row, dict):
                    rows.append(row)
    except OSError:
        return []
    return rows


def read_json(folder, name):
    try:
        with open(os.path.join(folder, name), encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError):
        return None


def read_walks(folder):
    """Every walk-NNN.json in the folder, in name order; a broken one is skipped."""
    out = []
    try:
        names = sorted(name for name in os.listdir(folder) if name.startswith(WALK_PREFIX) and name.endswith(WALK_SUFFIX))
    except OSError:
        return out
    for name in names:
        try:
            with open(os.path.join(folder, name), encoding="utf-8") as handle:
                value = json.load(handle)
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(value, dict):
            out.append(value)
    return out


def stages(folder):
    """Pooled stage timings of a folder's walks: per step the run closure's
    phases and the walk's own ms ("step"), per walk resolve/report."""
    out = {name: [] for name in STEP_STAGES + RUN_STAGES + ("step",)}
    for walk in read_walks(folder):
        for name in RUN_STAGES:
            value = (walk.get("phases") or {}).get(name)
            if isinstance(value, (int, float)):
                out[name].append(value)
        for ran in walk.get("ran") or []:
            if isinstance(ran.get("ms"), (int, float)):
                out["step"].append(ran["ms"])
            phases = ran.get("phases") or {}
            for name in STEP_STAGES:
                value = phases.get(name)
                if isinstance(value, (int, float)):
                    out[name].append(value)
    return out


def measure(folder):
    """One run's row, from its files alone."""
    reflex = read_json(folder, REFLEX_RUN)
    if isinstance(reflex, dict):
        return reflex_row(folder, reflex)
    steps = read_steps(folder)
    verdict = read_json(folder, VERDICT) or {}
    state = read_json(folder, STATE) or {}
    bench = read_json(folder, BENCH_RUN)
    ats = [row.get("at_epoch_ms") for row in steps if isinstance(row.get("at_epoch_ms"), int)]
    codes = {}
    confirms = stopped = 0
    for row in steps:
        code = None if row.get("ok", True) else refusal_code(row)
        if code:
            codes[code] = codes.get(code, 0) + 1
        # One step counts once: a declared last step, or the window's question.
        if CONFIRMING_FLAG in (row.get("argv") or []) or code in CONFIRM_CODES:
            confirms += 1
        if code in STOP_CODES:
            stopped += 1
    handoffs = sum(1 for row in steps if row.get("verb") == HANDOFF_VERB)
    out = {
        "folder": folder,
        "success": bool(verdict.get("pass")) if "pass" in verdict else None,
        "steps": len(steps),
        "acts": sum(1 for row in steps if row.get("acts")),
        "failed_steps": sum(1 for row in steps if not row.get("ok", True)),
        "handoffs": handoffs,
        "confirms": confirms,
        "stopped": stopped + (1 if state.get("stuck") else 0),
        "wall_ms": (max(ats) - min(ats)) if len(ats) >= 2 else 0,
        "codes": codes,
        "stages": stages(folder),
        "lane": "session",
        "config": "-",
    }
    if isinstance(bench, dict):
        out.update(bench_fields(bench, steps, handoffs, confirms))
    return out


def invalid_reason(bench):
    """Why a run that ran says nothing about the model, or None: a live run
    without its scenario's hands proof, zo ending on its own with an error (a
    provider, a quota, a crash), or no model answer at all."""
    if (bench.get("lane") or "live") != "live" or not (bench.get("setup") or {}).get("ok", True):
        return None
    exit_ = bench.get("exit") or {}
    if bench.get("hands_proved") is not True:
        return "no hands proof"
    if exit_.get("code") not in (0, None) and not exit_.get("killed"):
        return f"zo exited {exit_['code']}"
    if bench.get("tokens_total") is None and not bench.get("stopped_by"):
        return "no model answer"
    return None


def bench_fields(bench, steps, handoffs, confirms):
    """What bench-run.json adds: the oracle's verdict (none for a run that was
    cut by a stop not the model's, never set up, or invalid), the lane and
    config, and the run's costs."""
    oracle = bench.get("oracle") or {}
    setup = bench.get("setup") or {}
    exit_ = bench.get("exit") or {}
    stopped_by = bench.get("stopped_by")
    model_stop = str(stopped_by or "").startswith(MODEL_STOP_PREFIX)
    aborted = bool(stopped_by) and not model_stop
    invalid = invalid_reason(bench)
    judged = not aborted and setup.get("ok", True) and not invalid
    config = bench.get("config") or {}
    started = bench.get("started_at_ms")
    first_act = next((row.get("at_epoch_ms") for row in steps if row.get("acts") and isinstance(row.get("at_epoch_ms"), int)), None)
    success = None
    if judged:
        success = (bool(oracle.get("pass")) and not model_stop and not exit_.get("timed_out")
                   and not bench.get("off_road") and not bench.get("forbidden_verbs"))
    claimed = bench.get("claimed")
    return {
        "scenario": bench.get("scenario"),
        "lane": bench.get("lane") or "live",
        "config": str(config.get("id") or "-"),
        "wire_model": config.get("wire_model"),
        "success": success,
        "aborted": aborted,
        "invalid": 1 if invalid else 0,
        "invalid_reason": invalid,
        "setup_failed": not setup.get("ok", True),
        "tool_calls": sum((bench.get("tool_calls") or {}).values()),
        "off_road": len(bench.get("off_road") or []),
        "forbidden": len(bench.get("forbidden_verbs") or []),
        "tokens_total": bench.get("tokens_total"),
        "run_wall_ms": bench.get("run_wall_ms"),
        "first_act_ms": (first_act - started) if first_act is not None and isinstance(started, int) else None,
        "interventions": handoffs + confirms + (1 if stopped_by in PERSONS_STOP_REASONS else 0),
        "claimed": claimed,
        "claim_mismatch": 1 if claimed == "PASS" and oracle.get("pass") is False else 0,
        "step_latency_ms": bench.get("step_latency_ms") or [],
    }


def scenario_of(folder):
    """bench-run.json's scenario, else `bench:<id>` from the run row beside
    the folder, else the folder name."""
    bench = read_json(folder, BENCH_RUN)
    if isinstance(bench, dict) and bench.get("scenario"):
        return bench["scenario"]
    parent = os.path.dirname(folder)
    for name in ("run.json", "automation.json"):
        row = read_json(parent, name) or read_json(folder, name)
        if isinstance(row, dict):
            label = str(row.get("automation_name") or row.get("name") or "")
            if label.startswith("bench:"):
                return label[len("bench:"):]
    return os.path.basename(folder.rstrip(os.sep))


def walk(root, bench_only=False):
    """Every run folder under root: a bench run by its bench-run.json (a run
    that never acted has no steps), a reflex run by its reflex-run.json, any
    other by its step log."""
    for dirpath, _dirs, files in os.walk(root):
        if BENCH_RUN in files or REFLEX_RUN in files or (STEPS in files and not bench_only):
            yield dirpath


def reflex_row(folder, judged):
    """A reflex run's row: the oracle's verdict and success (None when a
    person, a deaf monitor or the runner stopped it: not judged), and its
    numbers — every count from the fixture's own record."""
    measured = judged.get("measure") or {}
    verdict = judged.get("verdict") or {}
    reaction = measured.get("appear_to_press_ms") or {}
    piloted = measured.get("autopilot") or {}
    asked = measured.get("l1") or {}
    cost = measured.get("cost") or {}
    return {
        "folder": folder,
        "scenario": judged.get("scenario"),
        "lane": judged.get("lane") or "reflex",
        "config": str(judged.get("config") or "-"),
        "success": judged.get("success"),
        "verdict": verdict.get("verdict"),
        "failed_checks": [row["check"] for row in verdict.get("checks") or [] if not row.get("passed")],
        "seed": judged.get("seed"),
        "hits": measured.get("hits"),
        "apm": measured.get("apm"),
        "apm_steady": measured.get("apm_steady"),
        "verified_apm": measured.get("verified_apm"),
        "wrong_inputs": measured.get("wrong_inputs"),
        "oracle": measured.get("oracle"),
        "oracle_goal": measured.get("oracle_goal"),
        "decisions": measured.get("decisions"),
        "decisions_due": measured.get("decisions_due"),
        "wall_s": measured.get("wall_s"),
        "preparation_s": measured.get("preparation_s"),
        "reaction_ms": {name: reaction.get(name) for name in ("n", "p50", "p95", "p99")},
        "roads": measured.get("roads") or {},
        "floors": judged.get("floors") or {},
        # The autopilot's own columns (t-10223 R9): None for a person's plan.
        "goal_to_first_press_ms": measured.get("goal_to_first_press_ms"),
        "replan_gap_ms": measured.get("replan_gap_ms") or {},
        "applied": piloted.get("applied"),
        "not_carried_out": piloted.get("invalid"),
        "unanswered": piloted.get("unanswered"),
        "ended": piloted.get("ended"),
        "plans": piloted.get("plans"),
        "sources": piloted.get("sources"),
        "roads_add_up": piloted.get("roads_add_up"),
        "l1_asked": asked.get("asked"),
        "l1_rtt_ms": asked.get("rtt_ms"),
        "tokens": cost.get("tokens"),
        "usd": cost.get("usd"),
    }


REFLEX_MEDIANS = ("apm", "apm_steady", "oracle", "oracle_goal", "decisions", "preparation_s", "wall_s",
                  "goal_to_first_press_ms", "plans")
# The autopilot's decisions carried out, in the order a column prints them.
REFLEX_DECISIONS = ("continue", "pause", "replan")


def summarize_reflex(rows, values=None):
    """Per (scenario, config) of the reflex rows: runs, judged and passed runs,
    the medians of the numbers, the worst of the ones with a floor (lowest APM,
    oracle and decisions; wrong inputs summed), and the pooled road counts."""
    values = values or table()
    groups = {}
    for row in rows:
        if row.get("lane") != "reflex":
            continue
        key = "|".join((row.get("scenario") or "?", row.get("config") or "-"))
        groups.setdefault(key, []).append(row)
    out = {}
    for key, group in sorted(groups.items()):
        judged = [row for row in group if row.get("success") is not None]
        entry = {"scenario": group[0].get("scenario"), "config": group[0].get("config"), "runs": len(group),
                 "judged": len(judged), "passed": sum(1 for row in judged if row["success"]),
                 "wrong_inputs": sum(int(row.get("wrong_inputs") or 0) for row in judged),
                 "apm_floor": 60_000 / values["human_step_ms"]}
        for name in REFLEX_MEDIANS:
            seen = [row[name] for row in judged if isinstance(row.get(name), (int, float))]
            entry["median_" + name] = statistics.median(seen) if seen else None
            entry["min_" + name] = min(seen) if seen else None
        for name in ("p50", "p95", "p99"):
            seen = [row["reaction_ms"][name] for row in judged if isinstance((row.get("reaction_ms") or {}).get(name), (int, float))]
            entry["reaction_" + name] = statistics.median(seen) if seen else None
        roads = {}
        for row in judged:
            for road, count in (row.get("roads") or {}).items():
                roads[road] = roads.get(road, 0) + int((count or {}).get("n") or 0)
        entry["roads"] = roads
        for name, column in (("replan_gap", "replan_gap_ms"), ("l1_rtt", "l1_rtt_ms")):
            for share in ("p50", "p95"):
                seen = [row[column][share] for row in judged
                        if isinstance((row.get(column) or {}).get(share), (int, float))]
                entry[f"median_{name}_{share}"] = statistics.median(seen) if seen else None
        piloted = [row for row in judged if row.get("applied") is not None]
        entry["applied"] = {word: sum(int(row["applied"].get(word) or 0) for row in piloted)
                            for word in REFLEX_DECISIONS} if piloted else None
        entry["roads_add_up"] = all(row.get("roads_add_up") for row in piloted) if piloted else None
        billed = [row for row in piloted if row.get("tokens")]
        entry["tokens"] = {side: sum(int(row["tokens"].get(side) or 0) for row in billed)
                           for side in ("input", "output")} if billed else None
        entry["usd"] = (None if any(row.get("usd") is None for row in billed)
                        else sum(row["usd"] for row in billed)) if billed else None
        out[key] = entry
    return out


def render_reflex(summary):
    lines = ["| scenario | config | runs | pass | APM (goal) | APM (steady) | wrong | oracle | decisions | "
             "appear→press p50/p95/p99 ms | roads | goal→press ms | plans | applied c/p/r | re-plan gap ms | "
             "L1 RTT p50/p95 ms | tokens in/out | $ |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for entry in summary.values():
        roads = ", ".join(f"{road} {count}" for road, count in entry["roads"].items()) or "-"
        reaction = "/".join(fmt_n(entry["reaction_" + name]) for name in ("p50", "p95", "p99"))
        applied = "/".join(str(entry["applied"][word]) for word in REFLEX_DECISIONS) if entry["applied"] else "-"
        if entry["roads_add_up"] is False:
            applied += " (roads differ)"
        rtt = "/".join(fmt_n(entry[f"median_l1_rtt_{share}"]) for share in ("p50", "p95"))
        tokens = f"{entry['tokens']['input']}/{entry['tokens']['output']}" if entry["tokens"] else "-"
        usd = f"{entry['usd']:.4f}" if isinstance(entry["usd"], float) else "-"
        lines.append(
            f"| {entry['scenario']} | {entry['config']} | {entry['runs']} | {entry['passed']}/{entry['judged']} | "
            f"{fmt_n(entry['min_apm'] and round(entry['min_apm'], 1))} (≥{entry['apm_floor']:g}) | "
            f"{fmt_n(entry['min_apm_steady'] and round(entry['min_apm_steady'], 1))} | {entry['wrong_inputs']} | "
            f"{fmt_n(entry['min_oracle'] and round(entry['min_oracle'], 4))} | {fmt_n(entry['min_decisions'])} | "
            f"{reaction} | {roads} | {fmt_n(entry['median_goal_to_first_press_ms'])} | "
            f"{fmt_n(entry['median_plans'])} | {applied} | {fmt_n(entry['median_replan_gap_p50'])} | {rtt} | "
            f"{tokens} | {usd} |")
    return "\n".join(lines)


def wilson(passed, judged, z):
    """The Wilson score interval of passed/judged at z."""
    if judged == 0:
        return None
    p = passed / judged
    denominator = 1 + z * z / judged
    centre = (p + z * z / (2 * judged)) / denominator
    half = z * math.sqrt(p * (1 - p) / judged + z * z / (4 * judged * judged)) / denominator
    return [round(max(0.0, centre - half), 4), round(min(1.0, centre + half), 4)]


MEDIANS = ("tool_calls", "first_act_ms", "run_wall_ms", "tokens_total", "interventions", "steps")
COUNTS = ("off_road", "claim_mismatch", "aborted", "setup_failed", "invalid")


def summarize(rows, values=None):
    """Per (scenario, lane, config): runs, success over judged runs with its
    Wilson interval, medians and counts, and the wire models its runs got. A
    group with no judged run has no rate (invalid), never 0."""
    values = values or table()
    z = values["wilson_z"]
    groups = {}
    for row in rows:
        key = "|".join((row.get("scenario") or "?", row.get("lane") or "session", row.get("config") or "-"))
        group = groups.setdefault(key, {"scenario": row.get("scenario"), "lane": row.get("lane"), "config": row.get("config"), "rows": []})
        group["rows"].append(row)
    out = {}
    for key, group in sorted(groups.items()):
        rows_ = group["rows"]
        judged = [row for row in rows_ if row.get("success") is not None]
        passed = sum(1 for row in judged if row["success"])
        entry = {
            "scenario": group["scenario"], "lane": group["lane"], "config": group["config"],
            "wire_models": sorted({row["wire_model"] for row in rows_ if row.get("wire_model")}),
            "runs": len(rows_), "judged": len(judged), "passed": passed,
            "success_rate": round(passed / len(judged), 4) if judged else None,
            "wilson": wilson(passed, len(judged), z),
        }
        for name in MEDIANS:
            seen = [row[name] for row in rows_ if isinstance(row.get(name), (int, float))]
            entry["median_" + name] = statistics.median(seen) if seen else None
            entry["n_" + name] = len(seen)
        for name in COUNTS:
            entry[name] = sum(int(row.get(name) or 0) for row in rows_)
        codes = {}
        for row in rows_:
            for code, n in (row.get("codes") or {}).items():
                codes[code] = codes.get(code, 0) + n
        entry["codes"] = dict(sorted(codes.items(), key=lambda item: (-item[1], item[0])))
        out[key] = entry
    return out


def percentile(values, share):
    """Nearest-rank percentile of a non-empty list."""
    ordered = sorted(values)
    return ordered[max(1, math.ceil(share * len(ordered))) - 1]


def summarize_stages(rows, values=None):
    """Every stage pooled over the rows' walks: n, p50, p95, its share of the
    step stages' timed ms; and the median step against the human floor
    (bench.json human_step_ms, the 200 APM floor) as x_human — flygym's
    "x realtime" column: above 1 is faster than a person."""
    values = values or table()
    pooled = {name: [] for name in STEP_STAGES + RUN_STAGES + ("step",)}
    for row in rows:
        for name, seen in (row.get("stages") or {}).items():
            pooled.setdefault(name, []).extend(seen)
    timed = sum(sum(pooled[name]) for name in STEP_STAGES)
    out = {}
    for name, seen in pooled.items():
        entry = {"n": len(seen), "p50": statistics.median(seen) if seen else None, "p95": percentile(seen, 0.95) if seen else None}
        if name in STEP_STAGES:
            entry["share_pct"] = round(100 * sum(seen) / timed, 1) if timed else None
        if name == "step":
            entry["x_human"] = round(values["human_step_ms"] / entry["p50"], 2) if entry["p50"] else None
        out[name] = entry
    return out


def render_stages(summary):
    lines = ["| stage | n | p50 ms | p95 ms | share % | × human |", "|---|---|---|---|---|---|"]
    for name in STEP_STAGES + ("step",) + RUN_STAGES:
        entry = summary.get(name) or {}
        lines.append(f"| {name} | {entry.get('n', 0)} | {fmt_n(entry.get('p50'))} | {fmt_n(entry.get('p95'))} | "
                     f"{fmt_n(entry.get('share_pct'))} | {fmt_n(entry.get('x_human'))} |")
    return "\n".join(lines)


def compare(summary, baseline, values=None):
    """Each group against the baseline's: a rate is up or down only when the
    intervals part; a median moved by at least median_change_pct, with at
    least median_min_n runs on both sides, is flagged. A group whose runs got
    another wire model than the baseline's is not compared at all."""
    values = values or table()
    bar, least = values["median_change_pct"], values["median_min_n"]
    verdicts = {}
    for key, new in summary.items():
        base = baseline.get(key)
        if not base:
            verdicts[key] = {"rate": "new"}
            continue
        if new.get("wire_models") and base.get("wire_models") and new["wire_models"] != base["wire_models"]:
            verdicts[key] = {"rate": "model changed", "moved": {}}
            continue
        rate = "within noise"
        if new.get("wilson") and base.get("wilson"):
            if new["wilson"][0] > base["wilson"][1]:
                rate = "up"
            elif new["wilson"][1] < base["wilson"][0]:
                rate = "down"
        moved = {}
        for name in MEDIANS:
            now, then = new.get("median_" + name), base.get("median_" + name)
            if now is None or not then or new.get("n_" + name, 0) < least or base.get("n_" + name, 0) < least:
                continue
            change = 100.0 * (now - then) / then
            if abs(change) >= bar:
                moved[name] = round(change, 1)
        verdicts[key] = {"rate": rate, "moved": moved}
    return verdicts


# The scenario an A/B's pooled groups are filed under.
POOLED = "*"


def versus(rows, base_config, config, values=None):
    """One config against another that took turns with it in the same runs
    (an A/B): `config`'s groups compared as against a baseline, the base
    config's groups standing in for it — per scenario, and pooled over every
    scenario (scenario `*`), where the runs are many enough to part two
    intervals."""
    sides = [row for row in rows if row.get("config") in (base_config, config)]
    summary = summarize(sides + [dict(row, scenario=POOLED) for row in sides], values)
    rekey = lambda key: key.rsplit("|", 1)[0] + "|" + config
    base = {rekey(key): entry for key, entry in summary.items() if entry["config"] == base_config}
    return compare({key: entry for key, entry in summary.items() if entry["config"] == config}, base, values)


def fmt_rate(entry):
    if entry["success_rate"] is None:
        return "—"
    low, high = entry["wilson"]
    return f"{entry['passed']}/{entry['judged']} [{low:.2f}, {high:.2f}]"


def fmt_ms(value):
    return "—" if value is None else f"{value / 1000:.1f}"


def fmt_n(value):
    return "—" if value is None else f"{value:g}"


def render(summary):
    lines = ["| scenario | lane | config | model | runs | success [95%] | tool calls | first act s | wall s | tokens | interventions | off-road | claim≠oracle | invalid |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for entry in summary.values():
        lines.append(
            f"| {entry['scenario']} | {entry['lane']} | {entry['config']} | {', '.join(entry.get('wire_models') or []) or '-'} | {entry['runs']} | {fmt_rate(entry)} | "
            f"{fmt_n(entry['median_tool_calls'])} | {fmt_ms(entry['median_first_act_ms'])} | {fmt_ms(entry['median_run_wall_ms'])} | "
            f"{fmt_n(entry['median_tokens_total'])} | {fmt_n(entry['median_interventions'])} | {entry['off_road']} | {entry['claim_mismatch']} | {entry.get('invalid', 0)} |")
    return "\n".join(lines)


def markdown(summary, versions=None, verdicts=None):
    """A dated section to append verbatim to docs/analysis/computer-bench.md."""
    lines = [f"## {time.strftime('%Y-%m-%d %H:%M')} — computer bench", ""]
    if versions:
        lines.append("- versions: " + ", ".join(f"{name} {value}" for name, value in versions.items()))
        lines.append("")
    lines.append(render(summary))
    if verdicts:
        lines.append("")
        for key, verdict in verdicts.items():
            moved = ", ".join(f"{name} {change:+}%" for name, change in (verdict.get("moved") or {}).items())
            lines.append(f"- {key}: {verdict['rate']}" + (f"; {moved}" if moved else ""))
    return "\n".join(lines) + "\n"


def collect(root, bench_only=False):
    rows = []
    for folder in walk(root, bench_only):
        row = measure(folder)
        if not row.get("scenario"):
            row["scenario"] = scenario_of(folder)
        rows.append(row)
    return rows


def main(argv):
    args = argv[1:]
    if not args or args[0].startswith("--"):
        print(__doc__.strip(), file=sys.stderr)
        return 2
    root = args[0]
    flag = lambda name: args[args.index(name) + 1] if name in args and args.index(name) + 1 < len(args) else None
    values = table()
    rows = collect(root, "--bench" in args)
    summary = summarize(rows, values)
    verdicts = None
    if flag("--baseline") and flag("--versus"):
        print("tally: --baseline or --versus, not both", file=sys.stderr)
        return 2
    if flag("--versus"):
        base_config, _, config = flag("--versus").partition(":")
        verdicts = versus(rows, base_config, config, values)
    if flag("--baseline"):
        with open(flag("--baseline"), encoding="utf-8") as handle:
            verdicts = compare(summary, json.load(handle), values)
    if flag("--write-baseline"):
        with open(flag("--write-baseline"), "w", encoding="utf-8") as handle:
            json.dump(summary, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
    stage_summary = summarize_stages(rows, values) if "--stages" in args or "--json" in args else None
    reflex_summary = summarize_reflex(rows, values) if "--reflex" in args or "--json" in args else None
    if "--json" in args:
        print(json.dumps({"runs": rows, "summary": summary, "compare": verdicts, "stages": stage_summary,
                          "reflex": reflex_summary}, ensure_ascii=False, indent=2))
    elif "--reflex" in args:
        print(render_reflex(reflex_summary))
    elif "--markdown" in args:
        print(markdown(summary, verdicts=verdicts), end="")
    else:
        print(render(summary))
        if stage_summary is not None:
            print()
            print(render_stages(stage_summary))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
