#!/usr/bin/env python3
"""The realtime bench's reflex round (t-6767, brief-realtime-R4): a fixture of
its own in a process and bundle of its own, an exogenous round drawn from a
seed, the helper's hand driven down the window's own reflex road, and an
oracle that reads only the fixture's record of what it drew and what it got.

prepare DIR                 builds ReflexFixture.swift into an app of its own.
rehearse DIR --seed N       plays a round with no hand at all: nothing is pressed.
run DIR --seed N --driver B announces, waits, re-checks that nobody is at the
                            machine and that the pointer rests on the fixture,
                            gives the goal (T0), and runs the window crate's
                            ignored driver test B (by its absolute path) for one
                            run of the table's length; the first input that is
                            not the hand's stops it for good.
    [--autopilot [WORDS]]   gives the driver a goal in place of a plan: the
    [--generator G] [--l1 W] window's reflex autopilot writes its plans (the
                            window's generator, or `stub`: this runner's plan
                            answered down the autopilot's road) and carries its
                            reflex decision out, forced as bench.json's
                            reflex_goal says, on ledgers in the run's own zo home.
judge RUN_DIR               judges a run's folder again from its files.

Every number is bench.json's (reflex_*) or the product's reflex table; the
APM wall starts at the goal and ends at the verdict, and only the fixture's own
hits are actions.
"""
import argparse
import collections
import getpass
import json
import math
import os
import pathlib
import plistlib
import random
import signal
import subprocess
import sys
import time
import uuid

import tally
from bench import Bench, Refused, Signals, Stopped
from fixture_apm import uptime_ns

HERE = pathlib.Path(__file__).resolve().parent
FIXTURES = HERE.parents[1] / "crates/zerocode-core/fixtures"
COLOURS = ("red", "blue")
SCENARIO = "reflex-fixture"
# The road this bench drives: the helper straight over its socket (pre-check (iii)).
CONFIG = "helper-direct"
# The generator that stands in for a model: this runner's plan, counted as its own.
STUB = "stub"
# Where a plan came from when a model wrote it — the autopilot's plans' word.
MODEL = "model"
# The helper's reason for a run it was asked to stop: the autopilot's pause and re-plan.
ON_REQUEST = "request"
# The autopilot's own end when its decision stopped answering usably.
ESCALATED = "escalated"
# The reflex decision's rows that were questions, and the one road that asks.
ASKED_OUTCOME = "answered"
JEV_ROAD = "jev"
DECISION_ROADS = ("memo", "surrogate", JEV_ROAD)


def limits():
    return json.loads((FIXTURES / "reflex-contract/limits.json").read_text())


def contract():
    return json.loads((FIXTURES / "reflex-contract/capability.json").read_text())["contract"]


def other(colour):
    return COLOURS[1 - COLOURS.index(colour)]


def field(values):
    """The field below the strip, in the window's content points (top-left origin)."""
    canvas, hud = values["reflex_canvas_pt"], values["reflex_hud_pt"]
    return {"x": 0, "y": hud, "width": canvas["width"], "height": canvas["height"] - hud}


def schedule(seed, values):
    """The round's stimulus, drawn from `seed` and the table alone: phases of
    one colour, one target at a time inside its phase with a clean gap after
    it, still decoys of the other colour inside a target's life, and curtains
    sweeping across the field. Nothing a run reads or does is an input: what
    a hit changes is the fixture's to record (the target goes), never the
    round."""
    draw = random.Random(seed)
    target, phase, decoy, curtain, rounds = (values[name] for name in (
        "reflex_target", "reflex_phase", "reflex_decoy", "reflex_curtain", "reflex_round"))
    length_ms = (values["reflex_safety"]["prep_s"] + rounds["run_s"]) * 1_000
    box = field(values)
    phases = []
    at, colour = 0, draw.choice(COLOURS)
    while at < length_ms:
        span = draw.randint(*phase["length_ms"])
        phases.append({"index": len(phases), "colour": colour, "startMs": at, "endMs": min(at + span, length_ms)})
        at, colour = at + span, other(colour)

    def phase_at(ms):
        return next(row for row in phases if row["startMs"] <= ms < row["endMs"])

    targets, decoys = [], []
    at = rounds["lead_ms"]
    while at < length_ms:
        gap, life = draw.randint(*target["gap_ms"]), draw.randint(*target["life_ms"])
        appear = at + gap
        if appear >= length_ms:
            break
        current = phase_at(appear)
        appear = max(appear, current["startMs"] + phase["settle_ms"])
        expire = appear + life
        if expire > current["endMs"]:
            # A target never outlives its phase's colour: the next phase's first one comes after its settle.
            at = current["endMs"]
            continue
        radius = draw.randint(*target["radius_pt"])
        speed = draw.uniform(*target["speed_pt_s"]) if draw.random() < target["moving_share"] else 0.0
        heading = draw.uniform(0, 2 * math.pi)
        vx, vy = round(speed * math.cos(heading), 2) or 0.0, round(speed * math.sin(heading), 2) or 0.0
        dx, dy = vx * life / 1_000, vy * life / 1_000
        inset = radius + target["edge_pt"]
        x = draw.uniform(box["x"] + inset - min(0, dx), box["x"] + box["width"] - inset - max(0, dx))
        y = draw.uniform(box["y"] + inset - min(0, dy), box["y"] + box["height"] - inset - max(0, dy))
        row = {"id": f"t{len(targets) + 1}", "phase": current["index"], "colour": current["colour"],
               "appearMs": appear, "expireMs": expire, "x": round(x, 2), "y": round(y, 2), "vx": vx, "vy": vy,
               "radius": radius}
        targets.append(row)
        if draw.random() < decoy["share"]:
            placed = decoy_beside(row, draw, values)
            if placed:
                decoys.append({"id": f"d{len(decoys) + 1}", "colour": other(current["colour"]),
                               "appearMs": appear + draw.randint(0, life // 2), "expireMs": expire, **placed})
        at = expire
    curtains = []
    at = draw.randint(*curtain["every_ms"])
    while at < length_ms:
        sweep, width = draw.randint(*curtain["sweep_ms"]), draw.randint(*curtain["width_pt"])
        rightward = draw.random() < 0.5
        speed = round((box["width"] + width) * 1_000 / sweep, 2)
        start = box["x"] - width if rightward else box["x"] + box["width"]
        curtains.append({"id": f"c{len(curtains) + 1}", "appearMs": at, "expireMs": at + sweep, "x": start,
                         "y": box["y"], "width": width, "height": box["height"], "vx": speed if rightward else -speed})
        at += sweep + draw.randint(*curtain["every_ms"])
    return {"seed": seed, "lengthMs": length_ms, "field": box, "phases": phases, "targets": targets,
            "decoys": decoys, "curtains": curtains}


def decoy_beside(target, draw, values):
    """A still decoy clear of the target's whole drift: one of the field's
    lattice points a decoy's width apart, drawn among those clear of it."""
    clearance = values["reflex_decoy"]["clearance_pt"]
    edge = values["reflex_target"]["edge_pt"]
    radius = draw.randint(*values["reflex_target"]["radius_pt"])
    box = field(values)
    life_s = (target["expireMs"] - target["appearMs"]) / 1_000
    start = (target["x"], target["y"])
    end = (target["x"] + target["vx"] * life_s, target["y"] + target["vy"] * life_s)
    pitch = 2 * radius + clearance
    inset = radius + edge
    candidates = [(x, y)
                  for x in range(box["x"] + inset, box["x"] + box["width"] - inset + 1, pitch)
                  for y in range(box["y"] + inset, box["y"] + box["height"] - inset + 1, pitch)
                  if apart((x, y), start, end) >= target["radius"] + radius + clearance]
    if not candidates:
        return None
    x, y = draw.choice(candidates)
    return {"x": x, "y": y, "radius": radius}


def apart(point, start, end):
    """The distance from `point` to the segment start-end."""
    (px, py), (ax, ay), (bx, by) = point, start, end
    dx, dy = bx - ax, by - ay
    span = dx * dx + dy * dy
    share = 0.0 if span == 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / span))
    return math.hypot(px - (ax + share * dx), py - (ay + share * dy))


def colour_class(entry, tolerance=True):
    words = ("r", "g", "b", "tolerance") if tolerance else ("r", "g", "b")
    return {word: entry[word] for word in words}


def plan(geometry, values, bundle, contract_version, rules=None, table_limits=None):
    """The plan the goal asks for, read off the window the window server named
    (`listWindows`) on the display it sits on (`displays`) and the table —
    never off the round: its reference extent is the display in points, its
    field the window below the strip, each colour's detector gated by anchors
    on the strip showing that colour, a rule per colour firing on exactly one
    blob of it, and a move-then-click macro at it. Unhashed: the driver hashes
    it with the core (`reflex::plan_hash`), so no second hash is written here."""
    display, window = geometry["display"], geometry["window"]
    hud, chosen, palette = values["reflex_hud_pt"], values["reflex_plan"], values["reflex_palette"]
    x, y = window["x"] - int(display["x"]), window["y"] - int(display["y"])
    # The field less its edge on every side: every target and decoy sits
    # inside it (the round insets them by more), and no sample lands on the
    # window's border, which the eye's scaled capture blends with whatever is
    # beside the window.
    edge = values["reflex_target"]["edge_pt"]
    roi = {"height": window["height"] - hud - 2 * edge, "space": "pixel", "width": window["width"] - 2 * edge,
           "x": x + edge, "y": y + hud + edge}
    classes = [colour_class(palette[name]) for name in COLOURS]
    detectors = []
    for number, colour in enumerate(COLOURS, start=1):
        detectors.append({
            "color": {
                "anchors": [{"class": number, "x": x + int(share * window["width"]), "y": y + hud // 2}
                            for share in chosen["anchors"]],
                "classes": classes,
                "confirm": chosen["confirm"],
                "ground": colour_class(palette["ground"]),
                "layout": {"class": number, "gate": chosen["gate_pt"], "kind": "blobs",
                           "max_blobs": chosen["max_blobs"], "min_samples": chosen["min_samples"],
                           "step": chosen["step_pt"]},
                "reference_height": int(display["height"]),
                "reference_width": int(display["width"]),
                "space": "srgb",
            },
            "id": colour, "kind": "color", "patches": 1, "roi": roi,
            "scale": {"denominator": 1, "numerator": 1},
        })
    # One rule a colour, or several: a rule that fired is spent until its
    # colour reads false again, so a rule beside it — armed, lower in priority —
    # tries the same target again while it stands when the first try ended
    # without a press. Every rule's quota keeps the plan inside the table's
    # expanded-action budget (two leaves a fire).
    many = rules or chosen["rules_per_colour"]
    leaves = 2
    budget = (table_limits or limits())["max_expanded_actions"]
    quota = min(chosen["quota"], budget // (len(COLOURS) * many * leaves))
    rules = [{"cooldown_ms": 0, "detector": colour, "id": f"hit_{colour}" + (f"_{try_}" if try_ > 1 else ""),
              "macro_id": f"tap_{colour}", "max_fires": quota, "predicate": {"op": "eq", "value": 1},
              "priority": many - try_ + 1}
             for colour in COLOURS for try_ in range(1, many + 1)]
    macros = [{"actions": [{"id": f"move_{colour}", "kind": "move", "target": colour},
                           {"id": f"click_{colour}", "kind": "click", "target": colour}],
               "id": f"tap_{colour}", "repeat": 1}
              for colour in COLOURS]
    return {"detectors": detectors, "macros": macros, "plan_hash": "",
            "pointer": {"curve": "cosine", "duration_ms": chosen["pointer_ms"], "instant": False},
            "rules": rules, "scope": {"surface": "macos_desktop", "target": bundle}, "version": contract_version}


# ------------------------------------------------------------- the oracle --
# Every check below reads the fixture's own record of what it drew and what
# it received, and the round; the driver's receipts and statuses are claims
# the record must confirm, never evidence of their own.

FOREIGN = "person"


def at_ns(record, ms):
    """A round's millisecond as the host clock's nanosecond (the goal is 0)."""
    return record["run"]["t0Ns"] + ms * 1_000_000


def acting(record):
    """The window the run could act in: the helper's acceptance to its deadline."""
    return record["started"]["acceptedNs"], record["started"]["deadlineNs"]


def phase_of(record, when_ns):
    """The round's phase showing at `when_ns`, or None outside the round."""
    ms = (when_ns - record["run"]["t0Ns"]) / 1_000_000
    return next((phase for phase in record["schedule"]["phases"] if phase["startMs"] <= ms < phase["endMs"]), None)


def presses(record):
    return [event for event in record["events"] if event["kind"] == "down"]


def hits(record):
    """Every press the fixture judged a hit, with the target it named."""
    return [event for event in presses(record) if "hit" in (event.get("judged") or {})]


def wrong(record):
    """Every input the round did not ask for, by kind, counted once each: a
    press the fixture judged a miss (by why), a press of another button, a
    release with no press before it, a drag, a scroll, a key press (its
    repeats are that same press). A waypoint is no input to judge."""
    counts = {}
    held = False

    def count(kind):
        counts[kind] = counts.get(kind, 0) + 1

    for event in record["events"]:
        kind = event["kind"]
        if kind == "down":
            judged = event.get("judged") or {}
            if event.get("button", 0) != 0:
                count("button")
            elif "hit" not in judged:
                count(judged.get("miss") or "unjudged")
            held = True
        elif kind == "up":
            if not held:
                count("stray_up")
            held = False
        elif kind == "key":
            if not event.get("repeat"):
                count("key")
        elif kind != "move":
            count(kind)
    return counts


def due(record, values, limits):
    """The round's targets a run is held to — the stimulus's facts alone:
    those whose whole life lies in the acting window (`run`), and those from
    the goal to the deadline (`goal`)."""
    start, deadline = acting(record)
    t0 = record["run"]["t0Ns"]
    in_run, in_goal = [], []
    for target in record["schedule"]["targets"]:
        appear, expire = at_ns(record, target["appearMs"]), at_ns(record, target["expireMs"])
        if appear >= start and expire <= deadline:
            in_run.append(target["id"])
        if appear >= t0 and expire <= deadline:
            in_goal.append(target["id"])
    transitions = [phase["index"] for phase in record["schedule"]["phases"][1:]
                   if start < at_ns(record, phase["startMs"]) <= deadline]
    return {"run": in_run, "goal": in_goal, "transitions": transitions}


def claims(record, limits):
    """Each click the runtime says it finished, held to a press the fixture
    judged a hit within one frame of it (the table's frame rate): one claim, one hit."""
    window = 1_000_000_000 // limits["frames_per_second"]
    free = sorted(event["evNs"] for event in hits(record))
    done = [receipt for receipt in record["receipts"] if receipt["outcome"] == "done" and receipt.get("downHostNs")]
    confirmed = 0
    for receipt in sorted(done, key=lambda receipt: receipt["downHostNs"]):
        match = next((at for at in free if abs(at - receipt["downHostNs"]) <= window), None)
        if match is not None:
            free.remove(match)
            confirmed += 1
    return {"done": len(done), "confirmed": confirmed, "unconfirmed": len(done) - confirmed}


def account(record):
    """The autopilot's account of a run it carried (its runs, plans, roads
    and end), or None for a person's plan."""
    return (record.get("ended") or {}).get("autopilot")


def traces(record):
    """Each run's report, status and the receipts on disk for it: one for a
    person's plan, one a run the autopilot started."""
    ended = record.get("ended") or {}
    runs = ended.get("runs")
    if runs is None:
        return [(ended.get("report") or {}, ended.get("status") or {}, len(record["receipts"]))]
    return [(run.get("report") or {}, run.get("status") or {},
             sum(1 for receipt in record["receipts"] if receipt.get("run") == run["runId"])) for run in runs]


def aborted(record):
    """Why a run says nothing about the reflex, or None: it was stopped by a
    person, a monitor that could not hear, or the runner, or a press or a move
    reached the fixture from anyone but the run's hand. An autopilot that
    stopped its own last run (a pause, an escalation) is judged, not excused."""
    if record["run"].get("stoppedBy"):
        return record["run"]["stoppedBy"]
    if not record.get("started") or not record.get("ended"):
        refused = (record.get("ended") or {}).get("refused") or {}
        return f"never started: {refused.get('code') or 'no answer from the driver'}"
    helper = record["geometry"]["helperPid"]
    if any(event.get("sourcePid") != helper for event in record["events"]):
        return FOREIGN
    reason = (record["ended"].get("status") or {}).get("reason")
    if reason == "deadline" or (account(record) is not None and reason == ON_REQUEST):
        return None
    return f"ended by {reason}"


def verdict(record, values, limits):
    """Whether the run did what a run must, every check exact."""
    why = aborted(record)
    if why:
        return {"verdict": "aborted", "reason": why, "checks": []}
    run, ready, fixture = record["run"], record["ready"], record["fixture"]
    schedule, started, ended = record["schedule"], record["started"], record["ended"]
    start, deadline = acting(record)
    frame_age = limits["max_frame_age_ns"]
    targets = {target["id"]: target for target in schedule["targets"]}
    judged_hits = hits(record)
    mistakes = wrong(record)
    claimed = claims(record, limits)
    runs = traces(record)
    whole = sum(1 for report, status, kept in runs
                if report.get("verified") is True and report.get("through") == status.get("receiptsIssued") == kept)
    ups = sum(1 for event in record["events"] if event["kind"] == "up")

    def the_rounds(event):
        target = targets.get(event["judged"]["hit"])
        if target is None:
            return False
        phase = phase_of(record, event["evNs"])
        return (phase is not None and phase["colour"] == target["colour"]
                and at_ns(record, target["appearMs"]) <= event["evNs"] <= at_ns(record, target["expireMs"]) + frame_age)

    checks = [
        ("starts clean", ready.get("events") == 0 and ready.get("hits") == 0 and ready.get("misses") == 0
         and fixture.get("owner") == run["owner"] == ready.get("owner") and fixture.get("t0Ns") == run["t0Ns"]
         and fixture.get("seed") == run["seed"] == schedule["seed"],
         f"before the goal: {ready.get('events')} events, {ready.get('hits')} hits; owner {fixture.get('owner')}"),
        ("the round played through the run", fixture.get("frames", 0) > 0 and fixture.get("firstFrameNs", start) <= start
         and fixture.get("lastFrameNs", 0) >= deadline,
         f"{fixture.get('frames', 0)} frames drawn"),
        ("the run acted", len(judged_hits) > 0, f"{len(judged_hits)} hits"),
        ("no wrong input", not mistakes, f"wrong inputs {mistakes or 0}"),
        ("every hit is the round's", all(the_rounds(event) for event in judged_hits),
         "each named target shows in its own phase and life"),
        ("buttons balanced", len(presses(record)) == ups == fixture.get("downs") == fixture.get("ups")
         and not fixture.get("held"),
         f"{len(presses(record))} presses, {ups} releases, held {fixture.get('held')}"),
        ("every done click is a fixture hit", claimed["unconfirmed"] == 0,
         f"{claimed['done']} done clicks, {claimed['confirmed']} confirmed"),
        ("the trace is whole", whole == len(runs),
         f"{whole} of {len(runs)} runs verified with every receipt on disk: "
         + "; ".join(f"{report.get('through')} of {status.get('receiptsIssued')}, {kept} kept"
                     for report, status, kept in runs)),
        ("the wall starts at the goal", started["requestNs"] >= run["t0Ns"] and start >= run["t0Ns"]
         and run["verdictNs"] > ended["endedNs"]
         and run["verdictNs"] - run["t0Ns"] >= values["reflex_floor"]["wall_s"] * 1_000_000_000,
         f"{(run['verdictNs'] - run['t0Ns']) / 1e9:.3f} s from the goal to the verdict"),
    ]
    piloted = account(record)
    if piloted is not None:
        sources = collections.Counter(plan.get("source") for plan in piloted.get("plans") or [])
        checks.append(("every plan is the model's", bool(sources) and set(sources) == {MODEL},
                       f"{sum(sources.values())} plans by source {dict(sources)}"))
    rendered = [{"check": name, "passed": bool(passed), "detail": detail} for name, passed, detail in checks]
    return {"verdict": "pass" if all(row["passed"] for row in rendered) else "fail", "checks": rendered}


def spread(values_ms):
    """n and nearest-rank p50/p95/p99 of a list of milliseconds."""
    if not values_ms:
        return {"n": 0, "p50": None, "p95": None, "p99": None}
    return {"n": len(values_ms), **{f"p{share}": round(tally.percentile(values_ms, share / 100), 3)
                                    for share in (50, 95, 99)}}


def measure(record, values, limits):
    """The run's numbers, every one from the fixture's record and the round;
    the runtime's receipts give only their own timings and the claims."""
    run = record["run"]
    start, deadline = acting(record)
    judged_hits = hits(record)
    targets = {target["id"]: target for target in record["schedule"]["targets"]}
    facts = due(record, values, limits)
    hit_ids = {event["judged"]["hit"] for event in judged_hits}
    wall_ns = run["verdictNs"] - run["t0Ns"]
    in_run = [event for event in judged_hits if start <= event["evNs"] <= deadline]
    apm = len(judged_hits) / (wall_ns / 60e9) if wall_ns > 0 else None
    apm_steady = len(in_run) / ((deadline - start) / 60e9) if deadline > start else None
    shown = {}
    for frame in record.get("frames") or []:
        for name in frame.get("shown") or []:
            shown.setdefault(name, frame["ns"])
    reaction = [(event["evNs"] - shown.get(event["judged"]["hit"], at_ns(record, targets[event["judged"]["hit"]]["appearMs"])))
                / 1e6 for event in judged_hits if event["judged"]["hit"] in targets]
    decided = 0
    for index in facts["transitions"]:
        phase = record["schedule"]["phases"][index]
        begin, end = at_ns(record, phase["startMs"]), at_ns(record, phase["endMs"])
        first = next((event for event in presses(record) if begin <= event["evNs"] < end), None)
        if first is not None and "hit" in (first.get("judged") or {}) and \
                targets.get(first["judged"]["hit"], {}).get("phase") == index:
            decided += 1
    leaves = {}
    for receipt in record["receipts"]:
        kind = "click" if receipt["actionId"].startswith("click") else "move"
        row = leaves.setdefault(kind, {"outcomes": {}, "decision_to_first_event": [], "frame_to_first_event": [],
                                       "capture_wait": [], "decision_to_admission": []})
        row["outcomes"][receipt["outcome"]] = row["outcomes"].get(receipt["outcome"], 0) + 1
        if receipt["outcome"] != "done":
            continue
        for name, later, earlier in (("decision_to_first_event", "firstEventHostNs", "decidedHostNs"),
                                     ("frame_to_first_event", "firstEventHostNs", "firstEventFrameHostNs"),
                                     ("decision_to_admission", "admittedHostNs", "decidedHostNs")):
            if receipt.get(later) and receipt.get(earlier):
                row[name].append((receipt[later] - receipt[earlier]) / 1e6)
        row["capture_wait"].append(receipt.get("captureWaitNs", 0) / 1e6)
    for row in leaves.values():
        for name in ("decision_to_first_event", "frame_to_first_event", "capture_wait", "decision_to_admission"):
            row[name] = spread(row[name])
    mistakes = wrong(record)
    moves, glides, seen = [], 0, set()
    for event in record["events"]:
        if event["kind"] == "move":
            seen.add((event["x"], event["y"]))
        elif event["kind"] == "down":
            moves.append(len(seen))
            glides += 1 if len(seen) >= 2 else 0
            seen = set()
    fires = sum(status.get("fires", 0) for _report, status, _kept in traces(record))
    passed = verdict(record, values, limits)["verdict"] == "pass"
    piloted = autopilot_numbers(record)
    downs = [event["evNs"] for event in presses(record)]
    return {
        "hits": len(judged_hits),
        "hits_in_run": len(in_run),
        "wall_s": round(wall_ns / 1e9, 3),
        "run_s": round((deadline - start) / 1e9, 3),
        "preparation_s": round((start - run["t0Ns"]) / 1e9, 3),
        "apm": apm,
        "apm_steady": apm_steady,
        "verified_apm": apm if passed else None,
        "wrong_inputs": sum(mistakes.values()),
        "wrong_by_kind": mistakes,
        "oracle": len(hit_ids & set(facts["run"])) / len(facts["run"]) if facts["run"] else None,
        "oracle_goal": len(hit_ids & set(facts["goal"])) / len(facts["goal"]) if facts["goal"] else None,
        "targets_due": {"run": len(facts["run"]), "goal": len(facts["goal"])},
        "decisions": decided,
        "decisions_due": len(facts["transitions"]),
        "claims": claims(record, limits),
        "appear_to_press_ms": spread(reaction),
        "leaves": leaves,
        "roads": roads(fires, piloted),
        "autopilot": piloted,
        "goal_to_first_press_ms": (min(downs) - run["t0Ns"]) / 1e6 if downs else None,
        "replan_gap_ms": spread(replan_gaps(record.get("calls") or [])),
        "l1": l1_numbers(record) if piloted is not None else None,
        "cost": plan_cost(record, values) if piloted is not None else None,
        "plan_rtt_ms": spread([row["rttMs"] for row in plan_rows(record) if isinstance(row.get("rttMs"), (int, float))]),
        "pointer": {"positions_per_press": spread(moves), "visible_glides": glides, "presses": len(moves)},
        "end": {"reason": (record["ended"].get("status") or {}).get("reason"), "held": record["fixture"].get("held")},
    }


def autopilot_numbers(record):
    """What the autopilot's account says it did (t-10223 §2.4), or None for a
    person's plan: the decisions carried out and not, the questions the door
    or the wire kept, its end, its plans by source, and whether its roads add
    up to the decisions it carried out."""
    piloted = account(record)
    if piloted is None:
        return None
    applied = dict(piloted.get("applied") or {})
    counted = piloted.get("roads") or {}
    return {
        "applied": applied,
        "invalid": dict(piloted.get("invalid") or {}),
        "unanswered": piloted.get("unanswered", 0),
        "door": dict(piloted.get("door") or {}),
        "wire": dict(piloted.get("wire") or {}),
        "ended": (piloted.get("ended") or {}).get("reason"),
        "plans": len(piloted.get("plans") or []),
        "sources": dict(collections.Counter(plan.get("source") for plan in piloted.get("plans") or [])),
        "road_counts": {road: counted.get(road, 0) for road in DECISION_ROADS},
        "roads_add_up": sum(counted.get(road, 0) for road in DECISION_ROADS) == sum(applied.values()),
    }


def roads(fires, piloted):
    """Every road's count: the hand's fires, and — for an autopilot — each
    decision road's share of the decisions carried out, and the runs that
    ended escalated; a person's plan asks no decision."""
    counts = (piloted or {}).get("road_counts") or {}
    carried = sum(((piloted or {}).get("applied") or {}).values())
    rows = {"l0": {"n": fires, "share": 1.0 if fires else None}}
    for road in DECISION_ROADS:
        rows[road] = {"n": counts.get(road, 0), "share": counts.get(road, 0) / carried if carried else 0.0}
    rows[ESCALATED] = {"n": int((piloted or {}).get("ended") == ESCALATED), "share": None}
    return rows


def replan_gaps(calls):
    """The milliseconds between one plan's hand letting go (its stop asked)
    and the next plan's hand taking over (its start answered)."""
    gaps, let_go = [], None
    started = False
    for call in sorted(calls, key=lambda call: call["askedNs"]):
        if call.get("refused"):
            continue
        if call["method"] == "reflexStop":
            let_go = call["askedNs"]
        elif call["method"] == "reflexStart":
            if started and let_go is not None:
                gaps.append((call["answeredNs"] - let_go) / 1e6)
            started, let_go = True, None
    return gaps


def question_rows(record):
    """The reflex decision's rows that were questions — its labels aside."""
    return [row for row in record.get("decisions") or [] if "decision" in row and "outcome" in row]


def l1_numbers(record):
    """The reflex decision's questions from its ledger in the bench's home:
    asked, answered, forced by the bench, the requests that went and their
    round trips."""
    rows = question_rows(record)
    return {
        "asked": len(rows),
        "answered": sum(1 for row in rows if row.get("outcome") == ASKED_OUTCOME),
        "forced": sum(1 for row in rows if (row.get("provenance") or {}).get("forced") is True),
        "requests": sum(int(row.get("attempts") or 0) for row in rows),
        "rtt_ms": spread([row["rttMs"] for row in rows if row.get("road") == JEV_ROAD
                          and isinstance(row.get("rttMs"), (int, float))]),
    }


def plan_rows(record):
    """The plans' ledger rows that were plans asked for — their labels aside."""
    return [row for row in record.get("plans") or [] if "outcome" in row]


def plan_cost(record, values):
    """What the autopilot's plans cost: tokens and requests summed over the
    plans' ledger, and dollars at bench.json's prices — None when a model
    that billed tokens has no price there."""
    prices = values["reflex_usd_per_million"]
    tokens = {"input": 0, "output": 0}
    usd = 0.0
    for row in plan_rows(record):
        billed = row.get("tokens") or {}
        for side in tokens:
            tokens[side] += int(billed.get(side) or 0)
        if not any(billed.values()):
            continue
        price = prices.get(row.get("model"))
        if price is None or usd is None:
            usd = None
            continue
        usd += sum(int(billed.get(side) or 0) * price[side] / 1e6 for side in tokens)
    return {"tokens": tokens, "plan_requests": sum(int(row.get("requests") or 0) for row in plan_rows(record)),
            "usd": usd}


def floors(measured, values):
    """The realtime gate's floors (t-6723 §7), each passed or not by the number."""
    floor = values["reflex_floor"]
    apm_floor = 60_000 / values["human_step_ms"]
    rows = {
        "apm": (measured["apm"] or 0) >= apm_floor,
        "wrong_inputs": measured["wrong_inputs"] == 0,
        "oracle": (measured["oracle"] or 0) >= floor["oracle"],
        "decisions": measured["decisions"] >= floor["decisions"],
        "wall": measured["wall_s"] >= floor["wall_s"],
    }
    piloted, asked = measured.get("autopilot"), measured.get("l1") or {}
    if piloted is not None:
        # The design's gate for a goal in place of a plan (t-10223 §5.1): no
        # plan but the model's, every carried-out decision on a road, and
        # every question the bench forced saying so.
        rows["model_plans"] = piloted["plans"] > 0 and set(piloted["sources"]) == {MODEL}
        rows["roads_add_up"] = piloted["roads_add_up"]
        rows["l1_forced"] = asked.get("asked", 0) > 0 and asked.get("forced") == asked.get("asked")
    return {"apm_floor": apm_floor, **{name: passed for name, passed in rows.items()}}


def judged(record, values, limits):
    """A run's row for tally.py (`reflex-run.json`): the verdict, the numbers
    and the floors, with the round's identity."""
    result = verdict(record, values, limits)
    if not record.get("started") or not record.get("ended"):
        # Nothing ran: no number to measure, the refusal is the record.
        return {"scenario": SCENARIO, "lane": "reflex", "config": record["run"].get("config", CONFIG),
                "seed": record["run"]["seed"], "owner": record["run"]["owner"], "verdict": result,
                "measure": None, "floors": None, "success": None,
                "refused": (record.get("ended") or {}).get("refused")}
    measured = measure(record, values, limits)
    gate = floors(measured, values)
    return {
        "scenario": SCENARIO,
        "lane": "reflex",
        "config": record["run"].get("config", CONFIG),
        "seed": record["run"]["seed"],
        "owner": record["run"]["owner"],
        "verdict": result,
        "measure": measured,
        "floors": gate,
        "success": None if result["verdict"] == "aborted" else (
            result["verdict"] == "pass" and all(value for name, value in gate.items() if name != "apm_floor")),
    }


# ---------------------------------------------------------------- safety --

def keychain(service, value=False):
    """One item of the person's keychain: whether it is there — or, asked for
    its value, the value, which goes into one driver's environment and
    nowhere else: never printed, never written, never an argument."""
    command = ["security", "find-generic-password", "-s", service, "-a", getpass.getuser()]
    done = subprocess.run(command + (["-w"] if value else []), capture_output=True, text=True)
    if done.returncode != 0:
        return None
    return (done.stdout.strip() or None) if value else True


def generator_key_name():
    """The name the value seat's chosen row keeps its key under, after the
    window's prefix — the key the window's generator asks with — or None for
    a row that keeps none there."""
    models = json.loads((FIXTURES / "type-value/models.json").read_text())
    row = next((row for row in models["rows"] if row["id"] == models.get("chosen")), None)
    return (row or {}).get("credentialKey")


def keys_for(values, generator, read):
    """The keys one autopilot driver is handed, by the name the window's key
    store gives them, read with `read` (the keychain): the Jev key its
    questions ask with, when there is one, and — for the window's own
    generator — that generator's key, without which nothing starts. A key
    that is not there is looked for and never read. Answers (env, None), or
    (None, why)."""
    keys = values["reflex_goal"]["keys"]
    env = {}
    if read(keys["prefix"] + keys["jev"]):
        env[keys["jev"]] = read(keys["prefix"] + keys["jev"], value=True)
    if generator != STUB:
        name = generator_key_name()
        if not name or not read(keys["prefix"] + name):
            return None, (f"the window's generator has no key ({keys['prefix']}{name}): it writes no plan, "
                          f"so an autopilot round runs only with --generator {STUB}")
        env[name] = read(keys["prefix"] + name, value=True)
    return {name: key for name, key in env.items() if key}, None


def bench_home(run):
    """The run's own zo home: its settings consent the run's folder (the
    evidence folder the autopilot's questions come from) and nothing else,
    and every ledger the driver writes lands under it."""
    home = run / "home"
    home.mkdir(mode=0o700)
    write_atomic(home / "settings.json", {"smart": {"jev": {"workspaces": [str(run)]}}})
    return home


def refusal(idle_s, locked, ready, values):
    """Why a run may not start now, or None: the screen locked, a keyboard or
    mouse used within the table's idle time, or the pointer not over the
    fixture (every input of the run must land on it)."""
    need = values["reflex_safety"]["idle_s"]
    if locked:
        return "the screen is locked"
    if idle_s < need:
        return f"the keyboard or mouse was used {idle_s:.0f} s ago; a run waits for {need} s idle"
    if not ready.get("pointerInside"):
        return "the pointer is not over the fixture's window, so the first glide would cross another app"
    return None


# The only line the runner ever writes to the driver: a run is started by its
# plan, and a paused run is stopped, never resumed.
DRIVER_WORDS = ("stop",)


class Supervisor:
    """What the runner does with what it hears while a run moves the pointer:
    the first sign of anyone else — a run paused for input that was not its
    hand's, anyone else heard, a monitor that can no longer hear, an event the
    fixture got from another source — stops the run, once. Nothing restarts
    or resumes it: the person's return ends the run for good."""

    def __init__(self, helper_pid):
        self.helper_pid = helper_pid
        self.stopped_by = None
        self.sent = []

    def _stop(self, why):
        self.stopped_by = why
        self.sent.append(DRIVER_WORDS[0])
        return DRIVER_WORDS[0]

    def status(self, status):
        if self.stopped_by:
            return None
        if status.get("reason") == "external_input" or (status.get("othersHeard") or 0) > 0:
            return self._stop(FOREIGN)
        if status.get("monitor") in ("interrupted", "unavailable"):
            return self._stop("monitor")
        if status.get("state") == "paused":
            return self._stop("paused")
        return None

    def fixture_event(self, event):
        if self.stopped_by:
            return None
        if event.get("sourcePid") != self.helper_pid:
            return self._stop(FOREIGN)
        return None


# ------------------------------------------------------------------ the runner --
# prepare builds the fixture into an app of its own; rehearse plays a round with
# no hand at all; run plays one with the helper's hand and judges it; judge
# judges a run's folder again from its files.

BUNDLE_PREFIX = "dev.zerocode.bench.reflex"
EXECUTABLE = "ReflexFixture"
SOURCE = HERE / "ReflexFixture.swift"
# The driver: the window's reflex road, as an ignored test of the window's crate.
DRIVER_TEST = "computer_use::reflex::bench::a_reflex_run_on_the_benchs_own_fixture"
FOLDER_ENV = "ZEROCODE_REFLEX_BENCH_DIR"
HELPER_ENV = "ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH"
# zo's own config home, first in its order: the bench's settings and ledgers.
HOME_ENV = "ZO_CONFIG_HOME"
SHIM = "zerocode-computer"


def write_atomic(path, value):
    partial = path.with_suffix(path.suffix + ".partial")
    partial.write_text(json.dumps(value, indent=2) + "\n")
    partial.replace(path)


def load(folder):
    """A run's record from its folder: the round, the fixture's files, the
    driver's files and the runner's own."""
    folder = str(folder)
    read = lambda name: tally.read_json(folder, name)
    lines = lambda name: tally.read_lines(folder, name)
    ended = read("ended.json")
    runs = (ended or {}).get("runs")
    ledgers = (ended or {}).get("ledgers") or {}
    receipts = lines("receipts.jsonl") if runs is None else [
        {**receipt, "run": run["runId"]} for run in runs for receipt in lines(run["receipts"])]
    return {
        "run": read("run.json"),
        "schedule": (read("round.json") or {}).get("schedule"),
        "geometry": read("geometry.json"),
        "ready": read("ready.json"),
        "fixture": read("fixture.json"),
        "frames": lines("frames.jsonl"),
        "events": lines("events.jsonl"),
        "started": read("started.json"),
        "receipts": receipts,
        "ended": ended,
        "calls": lines("calls.jsonl"),
        "decisions": lines(ledgers["decisions"]) if ledgers.get("decisions") else [],
        "plans": lines(ledgers["plans"]) if ledgers.get("plans") else [],
    }


def the_round(owner, seed, values, table_limits):
    """What the fixture plays: the round and everything it draws it with — no
    number of the fixture's own."""
    return {"owner": owner, "seed": seed, "canvas": values["reflex_canvas_pt"], "hud": values["reflex_hud_pt"],
            "palette": values["reflex_palette"], "framesPerSecond": table_limits["frames_per_second"],
            "frameAgeNs": table_limits["max_frame_age_ns"], "schedule": schedule(seed, values)}


def prepare(folder):
    """Compile the fixture into an app bundle of its own (a fresh owner, so a
    fresh bundle id the run's scope names); nothing is launched."""
    folder.mkdir(mode=0o700, parents=False, exist_ok=False)
    owner = uuid.uuid4().hex[:12]
    app = folder / f"{EXECUTABLE}-{owner}.app"
    executable = app / "Contents/MacOS" / EXECUTABLE
    executable.parent.mkdir(parents=True)
    with (app / "Contents/Info.plist").open("wb") as handle:
        plistlib.dump({"CFBundleIdentifier": f"{BUNDLE_PREFIX}.{owner}", "CFBundleExecutable": EXECUTABLE,
                       "CFBundleName": f"{EXECUTABLE}-{owner}", "CFBundlePackageType": "APPL",
                       "NSHighResolutionCapable": True}, handle)
    built = subprocess.run(["swiftc", "-O", "-swift-version", "6", "-warnings-as-errors", str(SOURCE),
                            "-o", str(executable)])
    write_atomic(folder / "session.json", {"owner": owner, "app": str(app), "executable": str(executable),
                                           "bundle": f"{BUNDLE_PREFIX}.{owner}", "swiftc": built.returncode})
    return built.returncode


class Desk:
    """One session folder's fixture: its runs, each in a folder of its own."""

    def __init__(self, folder, values, table_limits):
        self.folder = folder
        self.values = values
        self.limits = table_limits
        self.session = json.loads((folder / "session.json").read_text())
        self.fixture = None

    def run_folder(self, seed):
        # One run a seed, never reset: a folder that exists is a run that happened.
        path = self.folder / f"run-{seed}"
        path.mkdir(mode=0o700, exist_ok=False)
        return path

    def launch(self, run, seed):
        write_atomic(run / "round.json", the_round(self.session["owner"], seed, self.values, self.limits))
        with (run / "fixture.log").open("w") as log:
            self.fixture = subprocess.Popen([self.session["executable"], str(run / "round.json"), str(run)],
                                            stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        deadline = time.monotonic() + self.values["reflex_safety"]["ready_s"]
        while time.monotonic() < deadline:
            ready = tally.read_json(str(run), "ready.json")
            if ready:
                return ready
            if self.fixture.poll() is not None:
                break
            time.sleep(self.values["reflex_safety"]["poll_ms"] / 1_000)
        self.quit()
        raise RuntimeError("the fixture did not come up")

    def quit(self):
        """The fixture, and only it: its own pid, which flushes its record on SIGTERM."""
        if self.fixture and self.fixture.poll() is None:
            self.fixture.send_signal(signal.SIGTERM)
            try:
                self.fixture.wait(timeout=self.values["reflex_safety"]["grace_s"])
            except subprocess.TimeoutExpired:
                self.fixture.kill()
                self.fixture.wait()

    def refused(self, ready):
        return refusal(Bench.hid_idle_s(), Bench.screen_locked(), ready, self.values)

    def another_operator(self):
        """Why the desk is someone else's now, or None: the window's operator
        stopped, or another operator acted within the table's quiet time."""
        answer = subprocess.run([SHIM, "status", "--json"], capture_output=True, text=True)
        status = (tally.first_json_line(answer.stdout) or tally.first_json_line(answer.stderr) or {}).get("result") or {}
        helper = status.get("helper") if isinstance(status.get("helper"), dict) else {}
        if status.get("stopped") or helper.get("stopped"):
            return "the operator is stopped"
        last = helper.get("lastActionAt")
        quiet_ms = self.values["reflex_safety"]["operator_quiet_s"] * 1_000
        if isinstance(last, int) and int(time.time() * 1_000) - last < quiet_ms:
            return "another operator acted a moment ago"
        return None

    def another_bench(self):
        """Why the pointer may be someone else's now, or None: another reflex
        round standing on this machine — a runner, its driver or its fixture,
        from any checkout — would put two hands on one pointer, or cover this
        fixture with its own."""
        standing = other_benches(os.getpid())
        return f"another reflex round is standing ({standing[0]})" if standing else None

    def rehearse(self, seed, seconds):
        """A round with no hand: the fixture plays and records, nothing is pressed."""
        run = self.run_folder(f"rehearsal-{seed}")
        try:
            self.launch(run, seed)
            write_atomic(run / "start.json", {"t0Ns": uptime_ns()})
            time.sleep(seconds)
        finally:
            self.quit()
        fixture = tally.read_json(str(run), "fixture.json") or {}
        frames = tally.read_lines(str(run), "frames.jsonl")
        spans = [(b["ns"] - a["ns"]) / 1e6 for a, b in zip(frames, frames[1:])]
        return {"folder": str(run), "frames": fixture.get("frames"), "events": fixture.get("events"),
                "frame_ms": spread(spans), "shown": sorted({name for frame in frames for name in frame["shown"]})[:12]}

    def run(self, seed, driver, helper_app, rules=None, autopilot=None):
        """One run: announce, re-check, the goal, the driver, the supervision,
        the verdict. `autopilot` ({"generator", "words"}) gives the driver
        bench.json's goal in place of a plan."""
        safety = self.values["reflex_safety"]
        goal = self.values["reflex_goal"]
        keys = {}
        if autopilot is not None:
            autopilot = {"generator": autopilot.get("generator") or goal["generator"],
                         "words": autopilot.get("words") or goal["words"], "l1": autopilot.get("l1") or goal["l1"]}
            keys, why = keys_for(self.values, autopilot["generator"], keychain)
            if why:
                raise Refused(why)
        why = self.refused({"pointerInside": True}) or self.another_operator() or self.another_bench()
        if why:
            raise Refused(why)
        print(f"reflex bench: the pointer will move on the fixture in {safety['announce_s']} s for "
              f"{self.values['reflex_round']['run_s']} s — any keyboard or mouse input stops it", flush=True)
        time.sleep(safety["announce_s"])
        run = self.run_folder(seed)
        ready = self.launch(run, seed)
        many = rules or self.values["reflex_plan"]["rules_per_colour"]
        config = CONFIG if many == self.values["reflex_plan"]["rules_per_colour"] else f"{CONFIG}+rules{many}"
        if autopilot is not None:
            config += f"+autopilot-{autopilot['generator']}"
            if autopilot["l1"] != goal["l1"]:
                config += f"-l1{autopilot['l1']}"
        record = {"owner": self.session["owner"], "seed": seed, "fixturePid": ready["pid"], "rules": many,
                  "config": config, "readyNs": ready["readyNs"], "stoppedBy": None}
        if autopilot is not None:
            record["autopilot"] = {"generator": autopilot["generator"], "l1": autopilot["l1"]}
        driver_process = None
        try:
            why = self.refused(ready) or self.another_operator() or self.another_bench()
            if why:
                record["refused"] = why
                raise Refused(why)
            record["t0Ns"] = uptime_ns()
            record["load"] = {"goal": list(os.getloadavg())}
            write_atomic(run / "start.json", {"t0Ns": record["t0Ns"]})
            request = {"bundle": self.session["bundle"], "pollMs": safety["poll_ms"],
                       "seconds": self.values["reflex_round"]["run_s"], "renew": True, "restore": ready["pointer"]}
            env = {**os.environ, FOLDER_ENV: str(run), HELPER_ENV: helper_app}
            if autopilot is not None:
                home = bench_home(run)
                request.update(goal=autopilot["words"], generator=autopilot["generator"], l1=autopilot["l1"],
                               home=str(home))
                env.update({HOME_ENV: str(home), **keys})
            write_atomic(run / "request.json", request)
            with (run / "driver.log").open("w") as log:
                driver_process = subprocess.Popen(
                    [os.path.abspath(driver), DRIVER_TEST, "--exact", "--ignored", "--nocapture", "--test-threads", "1"],
                    stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT, text=True,
                    env=env, start_new_session=True)
            del env, keys
            record["stoppedBy"] = self.supervise(run, record, driver_process)
        finally:
            if driver_process:
                self.end_driver(driver_process)
            self.quit()
            write_atomic(run / "run.json", record)
        # The wall ends at the verdict: every file read, the oracle's to judge.
        loaded = load(run)
        record["verdictNs"] = loaded["run"]["verdictNs"] = uptime_ns()
        record.setdefault("load", {})["verdict"] = list(os.getloadavg())
        write_atomic(run / "run.json", record)
        result = judged(loaded, self.values, self.limits)
        write_atomic(run / tally.REFLEX_RUN, result)
        return result

    def supervise(self, run, record, driver_process):
        """Plan off the window server's geometry, then watch the run until it
        ends: the first input that is not the hand's stops it for good."""
        safety = self.values["reflex_safety"]
        poll = safety["poll_ms"] / 1_000
        prep_until = record["t0Ns"] + safety["prep_s"] * 1_000_000_000
        end_by = prep_until + (self.values["reflex_round"]["run_s"] + safety["grace_s"]) * 1_000_000_000
        watch = None
        cursors = {"status.jsonl": 0, "events.jsonl": 0}
        planned = False

        def stop(why):
            if driver_process.poll() is None:
                driver_process.stdin.write(DRIVER_WORDS[0] + "\n")
                driver_process.stdin.flush()
            return why

        def fresh(name):
            """The lines appended to `name` since the last read, whole lines only."""
            try:
                with (run / name).open("rb") as handle:
                    handle.seek(cursors[name])
                    text = handle.read()
            except OSError:
                return []
            complete = text[: text.rfind(b"\n") + 1]
            cursors[name] += len(complete)
            return [json.loads(line) for line in complete.splitlines() if line.strip()]

        while True:
            if (run / "ended.json").exists() and driver_process.poll() is not None:
                return watch.stopped_by if watch else None
            if driver_process.poll() is not None and not (run / "ended.json").exists():
                return stop("driver exited")
            geometry = tally.read_json(str(run), "geometry.json")
            if not planned:
                if geometry:
                    watch = Supervisor(helper_pid=geometry["helperPid"])
                    # A person's plan, or the stand-in's answer; the window's generator writes its own.
                    if (record.get("autopilot") or {}).get("generator", STUB) == STUB:
                        write_atomic(run / "plan.json", plan(geometry, self.values, self.session["bundle"],
                                                             contract(), rules=record["rules"],
                                                             table_limits=self.limits))
                    planned = True
                elif uptime_ns() > prep_until:
                    return stop("preparation overran")
            if watch:
                for row in fresh("status.jsonl"):
                    if watch.status(row["status"]):
                        stop(watch.stopped_by)
                for event in fresh("events.jsonl"):
                    if watch.fixture_event(event):
                        stop(watch.stopped_by)
            if uptime_ns() > end_by:
                return stop("overran its deadline")
            time.sleep(poll)

    def end_driver(self, driver_process):
        """The driver drops its helper session on its way out (the helper ends
        with it); one that does not leave in the grace is ended, its own
        process group only."""
        try:
            driver_process.wait(timeout=self.values["reflex_safety"]["grace_s"])
        except subprocess.TimeoutExpired:
            os.killpg(driver_process.pid, signal.SIGTERM)
            driver_process.wait()
        finally:
            if driver_process.stdin:
                driver_process.stdin.close()


def other_benches(own, listing=None):
    """Every reflex round standing now that is not `own`'s or its children's:
    a runner (`run` or `rehearse`), a driver test, or a fixture executable —
    read with `ps -Aww`, since a clipped command line would hide one."""
    if listing is None:
        listing = subprocess.run(["ps", "-Aww", "-o", "pid=,ppid=,command="], capture_output=True, text=True).stdout
    marks = (f"{pathlib.Path(__file__).name} run", f"{pathlib.Path(__file__).name} rehearse", DRIVER_TEST,
             f"/Contents/MacOS/{EXECUTABLE}")
    standing = []
    for line in listing.splitlines():
        parts = line.split(None, 2)
        if len(parts) < 3 or not parts[0].isdigit() or not parts[1].isdigit():
            continue
        pid, ppid, command = int(parts[0]), int(parts[1]), parts[2]
        if own in (pid, ppid):
            continue
        if any(mark in command for mark in marks):
            standing.append(command)
    return standing


def installed_helper():
    """The helper the window runs, as the window names it."""
    answer = subprocess.run([SHIM, "permissions", "--json"], capture_output=True, text=True)
    result = (tally.first_json_line(answer.stdout) or {}).get("result") or {}
    return result.get("helper_app_path")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "rehearse", "run", "judge"])
    parser.add_argument("folder", type=pathlib.Path)
    parser.add_argument("--seed", type=int)
    parser.add_argument("--seconds", type=float, help="rehearse: how long the round plays with no hand")
    parser.add_argument("--driver", help="run: the window crate's test binary (target/debug/deps/zerocode_shell-…)")
    parser.add_argument("--helper-app", help="run: the helper app (default: the one the window names)")
    parser.add_argument("--rules", type=int, help="run: rules a colour (default: the table's rules_per_colour)")
    parser.add_argument("--autopilot", nargs="?", const="", metavar="WORDS",
                        help="run: a goal in place of a plan (default words: bench.json's reflex_goal)")
    parser.add_argument("--generator", choices=["window", STUB],
                        help="run --autopilot: who writes the plans (default: bench.json's reflex_goal)")
    parser.add_argument("--l1", choices=["auto", "shadow", "off"],
                        help="run --autopilot: the reflex decision's forced word (default: bench.json's reflex_goal)")
    args = parser.parse_args(argv)
    values, table_limits = tally.table(), limits()
    signals = Signals().install()
    try:
        if args.command == "prepare":
            return prepare(args.folder.resolve())
        if args.command == "judge":
            result = judged(load(args.folder), values, table_limits)
            write_atomic(args.folder / tally.REFLEX_RUN, result)
        else:
            desk = Desk(args.folder.resolve(), values, table_limits)
            if args.seed is None:
                parser.error("--seed is required")
            if args.command == "rehearse":
                result = desk.rehearse(args.seed, args.seconds or values["reflex_phase"]["length_ms"][1] / 1_000)
            else:
                helper_app = args.helper_app or installed_helper()
                if not args.driver or not helper_app:
                    parser.error("run needs --driver and a helper app")
                autopilot = None if args.autopilot is None else {"words": args.autopilot, "generator": args.generator,
                                                                 "l1": args.l1}
                result = desk.run(args.seed, args.driver, helper_app, rules=args.rules, autopilot=autopilot)
        print(json.dumps(result, indent=2))
        return 0
    except Refused as why:
        print(f"REFUSED: {why}", file=sys.stderr)
        return 3
    except Stopped as why:
        print(f"STOPPED: {why}", file=sys.stderr)
        return 3
    finally:
        signals.quiet()


if __name__ == "__main__":
    sys.exit(main())
