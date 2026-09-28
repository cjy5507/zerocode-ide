#!/usr/bin/env python3
"""Covered reflex rounds (t-12979): what stands over the fixture, where and
when, drawn from a seed, and the grade a run earns on it — read from the
fixture's own record and from the cover the bench put up, never from what
the hand says of itself.

A scene is one of bench.json's `cover_scene` kinds:
  fixture_panel   a second sheet of the fixture, floating above it — a move
                  of the fixture's window clears it;
  other_window    another app's ordinary window — bringing the fixture's
                  window forward clears it;
  modal           another app's dialog waiting for an answer — only the
                  person may clear it; the hand presses nothing and asks.
It hides part of the field or all of it, and stands from the goal or comes
in the middle of the round.

The grade holds a run to what a person would do. A run that did nothing, and
a hand that only stopped in front of a cover it could have cleared, score 0;
any wrong input — a press on the cover, or one the fixture judged a miss —
scores 0 whatever else it did; on a modal the right run presses nothing
after the cover came, makes no wrong input, and asks the person.

The seeds a measurement is taken on are fixed in `cover_seeds.json`, written
after this grader was committed and before any scene of them was drawn.

A run puts its scene up through `fixture_reflex.py run --cover-seed N`: a
fixture panel is the fixture's own second sheet (the round file carries it);
another app's window or dialog is CoverFixture's, an app of its own prepared
beside the fixture. The run's folder then holds cover.json — the scene, when
it showed, the presses it took and how often the hand held for the person.

draw SEED          prints the scene one seed draws (for a run, not for tuning).
grade RUN_DIR      grades a run's folder: its record beside cover.json.
"""
import argparse
import json
import pathlib
import random
import signal
import subprocess
import sys

import fixture_reflex as reflex
import tally

HERE = pathlib.Path(__file__).resolve().parent
SEEDS = HERE / "cover_seeds.json"
# The other app a scene's window or dialog comes from, prepared beside the fixture.
EXECUTABLE = "CoverFixture"
SOURCE = HERE / "CoverFixture.swift"
BUNDLE_PREFIX = "dev.zerocode.bench.cover"
FOLDER = "cover"
# The fixture's own second sheet: the one kind its round file carries.
OWN_SHEET = "fixture_panel"
# The cover seat's ledger in the bench's home, and the key a row that held the
# hand for the person carries.
LEDGER = "cover.jsonl"
HELD = "held"
# The code a press answers when the hand left the place to the person.
COVERED = "covered"
# The kinds a hand may clear by moves of the fixture's own window, and the
# one it may not.
CLEARABLE = ("fixture_panel", "other_window")
MODAL = "modal"


def draw(seed, values, box=None):
    """The scene `seed` draws from the table alone: its kind, whether it hides
    part of `box` — the reflex fixture's field unless another is named, in its
    window's content points — or all of it, where, and when it comes, in the
    round's milliseconds (0: it stands from the goal)."""
    table = values["cover_scene"]
    rng = random.Random(seed)
    box = box or reflex.field(values)
    kind = rng.choice(table["kinds"])
    full = rng.random() < table["full_share"]
    if full:
        margin = table["full_margin_pt"]
        rect = {"x": box["x"] - margin, "y": box["y"] - margin,
                "width": box["width"] + 2 * margin, "height": box["height"] + 2 * margin}
    else:
        share = rng.uniform(*table["partial_share"])
        # A band across the field, from one of its four edges, hiding `share` of it.
        edge = rng.choice(("left", "right", "top", "bottom"))
        across = edge in ("left", "right")
        extent = box["width"] if across else box["height"]
        depth = max(table["min_side_pt"], round(extent * share))
        depth = min(depth, extent)
        if across:
            x = box["x"] if edge == "left" else box["x"] + box["width"] - depth
            rect = {"x": x, "y": box["y"], "width": depth, "height": box["height"]}
        else:
            y = box["y"] if edge == "top" else box["y"] + box["height"] - depth
            rect = {"x": box["x"], "y": y, "width": box["width"], "height": depth}
    during = rng.random() < table["during_share"]
    appear = rng.randint(*table["during_ms"]) if during else 0
    return {"seed": seed, "kind": kind, "full": full, "rect": rect, "appearMs": appear}


def covered(target, rect):
    """Whether a target's centre, where a press lands, lies under the cover."""
    return (rect["x"] <= target["x"] <= rect["x"] + rect["width"]
            and rect["y"] <= target["y"] <= rect["y"] + rect["height"])


def grade(record, scene, cover):
    """What a run earned on `scene`: the round's targets that showed under the
    cover after it came, how many of them the fixture says were hit, the wrong
    inputs (the fixture's, and every press the cover itself took), how long
    from the cover's arrival to the first hit under it, how often the person
    was asked (`cover`, the bench's account of the cover it put up: when it
    stood and what it heard), and the score."""
    if not record.get("started"):
        # The hand never began (its plan was refused, or the driver was):
        # nothing to grade, and it says why.
        return {"kind": scene["kind"], "aborted": reflex.aborted(record), "score": None}
    start, deadline = reflex.acting(record)
    came = max(reflex.at_ns(record, scene["appearMs"]), start)
    under = [target for target in record["schedule"]["targets"]
             if covered(target, scene["rect"])
             and reflex.at_ns(record, target["appearMs"]) >= came
             and reflex.at_ns(record, target["expireMs"]) <= deadline]
    names = {target["id"] for target in under}
    hit_at = sorted(event["evNs"] for event in reflex.hits(record) if event["judged"]["hit"] in names)
    after = [event for event in reflex.presses(record) if event["evNs"] >= came]
    mistakes = dict(reflex.wrong(record))
    if cover.get("downs"):
        mistakes["on_the_cover"] = cover["downs"]
    asked = cover.get("asked", 0)
    if mistakes:
        score = 0.0
    elif scene["kind"] == MODAL:
        score = 1.0 if not after and asked > 0 else 0.0
    else:
        score = len(hit_at) / len(under) if under else None
    return {
        "kind": scene["kind"],
        "under": len(under),
        "hits": len(hit_at),
        "pressesAfter": len(after),
        "wrong": mistakes,
        "recoverMs": (hit_at[0] - came) / 1_000_000 if hit_at else None,
        "asked": asked,
        "score": score,
    }


def in_round(scene):
    """What the fixture's round file carries of `scene`: its own second sheet,
    or nothing when the cover is another app's."""
    if scene is None or scene["kind"] != OWN_SHEET:
        return None
    return {"kind": scene["kind"], **scene["rect"], "appearMs": scene["appearMs"]}


def on_screen(scene, ready):
    """The scene's place in the screen's top-left points: its place in the
    fixture window's content points, from the corner the fixture reported."""
    window, rect = ready["window"], scene["rect"]
    return {"x": window["x"] + rect["x"], "y": window["y"] + rect["y"],
            "width": rect["width"], "height": rect["height"]}


def put_up(scene, session_folder, run, ready, t0_ns, own_sheet_from_fixture=True):
    """Start the other app whose window or dialog the scene puts over the
    fixture, due at its moment of the round; None when the fixture shows the
    cover itself — its own second sheet, unless the fixture cannot
    (`own_sheet_from_fixture` false: the other app floats a panel instead)."""
    if scene is None or (scene["kind"] == OWN_SHEET and own_sheet_from_fixture):
        return None
    session = json.loads((session_folder / FOLDER / "session.json").read_text())
    place = on_screen(scene, ready)
    state = run / FOLDER
    state.mkdir(mode=0o700)
    due = t0_ns + scene["appearMs"] * 1_000_000
    with (state / "cover.log").open("w") as log:
        return subprocess.Popen([session["executable"], scene["kind"], str(place["x"]), str(place["y"]),
                                 str(place["width"]), str(place["height"]), str(due), str(state)],
                                stdout=log, stderr=subprocess.STDOUT, start_new_session=True)


def take_down(process, grace_s):
    """The cover's app, and only it: its own pid, which flushes its count on SIGTERM."""
    if process is None or process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=grace_s)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def held_for_the_person(run):
    """How often the cover seat held the hand for the person in this run: its
    rows in the bench home's ledger that say so."""
    held = 0
    for ledger in sorted(run.glob(f"home/**/{LEDGER}")):
        for line in ledger.read_text().splitlines():
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            held += HELD in row
    return held


def account(scene, run, own_sheet_from_fixture=True):
    """cover.json: the scene, when its cover showed, the presses it took, and
    how often the hand held for the person — read from whoever showed it."""
    by_fixture = scene["kind"] == OWN_SHEET and own_sheet_from_fixture
    shown_by = None if by_fixture else run / FOLDER / "fixture.json"
    if shown_by is not None:
        state = json.loads(shown_by.read_text()) if shown_by.exists() else {}
    else:
        fixture = run / "fixture.json"
        state = (json.loads(fixture.read_text()) if fixture.exists() else {}).get("cover") or {}
    # A hold the seat was never asked about (it was off) still ends the
    # autopilot `covered`, for the person: that is the run's one ask.
    ended = run / "ended.json"
    piloted = (json.loads(ended.read_text()) if ended.exists() else {}).get("autopilot") or {}
    ended_covered = (piloted.get("ended") or {}).get("reason") == COVERED
    return {"scene": scene, "shownNs": state.get("shownNs"), "downs": state.get("downs", 0),
            "asked": held_for_the_person(run) or int(ended_covered)}


def press_grade(presses, scene, cover):
    """What a run of presses by number earned on `scene` (fixture_apm.py
    covered): the presses whose button's centre the cover stood over, how
    many of those the fixture's oracle took as the right button, the wrong
    inputs (the oracle's errors, and every press the cover itself took), the
    time a press that had to uncover its button took (median and p95, in
    milliseconds), how often the press was refused `covered` for the person,
    and the score by the same rules as a reflex round's."""
    under = [press for press in presses if press["under"]]
    hits = sum(1 for press in under if press["hit"])
    mistakes = {}
    errors = max([press["errorsAfter"] for press in presses] or [0])
    if errors:
        mistakes["oracle"] = errors
    if cover.get("downs"):
        mistakes["on_the_cover"] = cover["downs"]
    asked = sum(1 for press in presses if press.get("code") == COVERED)
    took = sorted(press["wallMs"] for press in under if press.get("uncovered"))
    if mistakes:
        score = 0.0
    elif scene["kind"] == MODAL:
        score = 1.0 if under and hits == 0 and asked > 0 else 0.0
    else:
        score = hits / len(under) if under else None
    return {
        "kind": scene["kind"],
        "under": len(under),
        "hits": hits,
        "wrong": mistakes,
        "recoverMs": {key: reflex.spread(took)[key] for key in ("p50", "p95")},
        "asked": asked,
        "score": score,
    }


def seeds():
    """The seeds fixed for measurement, in the order they are run."""
    return json.loads(SEEDS.read_text())["seeds"]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    verbs = parser.add_subparsers(dest="verb", required=True)
    one = verbs.add_parser("draw")
    one.add_argument("seed", type=int)
    graded = verbs.add_parser("grade")
    graded.add_argument("run_dir", type=pathlib.Path)
    args = parser.parse_args(argv)
    values = tally.table()
    if args.verb == "draw":
        print(json.dumps(draw(args.seed, values)))
        return 0
    record = reflex.load(args.run_dir)
    shown = json.loads((args.run_dir / "cover.json").read_text())
    print(json.dumps(grade(record, shown["scene"], shown), indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
