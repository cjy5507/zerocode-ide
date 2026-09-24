"""Summarize own-window measurements. Never turns missing data into zero."""
import json
import math
import sys


def percentile(values, share):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * share) - 1)] if values else None


def distribution(values):
    return {"n": len(values), "p50": percentile(values, .5), "p95": percentile(values, .95),
            "max": max(values) if values else None}


def cursor_changes(samples):
    """Observed plateaus, not a claim that every posted waypoint was received."""
    changes = []
    for row in samples:
        if not changes or (row["x"], row["y"]) != (changes[-1]["x"], changes[-1]["y"]):
            changes.append(row)
    return changes


def summarize(data):
    rows = data["rows"]
    frames = [r for r in rows if r["kind"] == "frame"]
    if data.get("rc") != 0 or data.get("stream_error") or not frames:
        raise ValueError("No valid capture run")
    if any(r["age_ms"] < 0 for r in frames):
        raise ValueError("Frame clock is in the future")
    output = {"arms": [], "baseline": [r for r in rows if r["kind"] == "baseline"]}
    for arm in (r for r in rows if r["kind"] == "arm"):
        selected = [r for r in frames if r["stage"] == arm["stage"]]
        if not selected:
            raise ValueError("An arm captured nothing")
        polls = [r["age_ms"] for r in rows if r["kind"] == "poll" and r["stage"] == arm["stage"]]
        output["arms"].append({**arm, "cpu_percent_one_core": 100 * arm["cpu_s"] / arm["wall_s"],
            "delivered_fps": len(selected) / arm["wall_s"],
            "callback_age_ms": distribution([r["age_ms"] for r in selected]),
            "latest_age_at_poll_ms": distribution(polls),
            "tick_without_post_ms": distribution([r["tick_ms"] for r in selected])})
    moves = [r for r in rows if r["kind"] == "move_event"]
    output["cli"] = []
    for call in (r for r in rows if r["kind"] == "cli"):
        events = [r for r in moves if call["start_ns"] <= r["at_ns"] <= call["end_ns"]]
        intervals = [(b["at_ns"] - a["at_ns"]) / 1e6 for a, b in zip(events, events[1:])]
        output["cli"].append({"steps": call["steps"], "rc": call["result"]["rc"],
            "wall_ms": (call["end_ns"] - call["start_ns"]) / 1e6,
            "observed_events": len(events), "waypoint_gap_ms": distribution(intervals),
            "event_span_ms": (events[-1]["at_ns"] - events[0]["at_ns"]) / 1e6 if len(events) > 1 else None})
    posts = [r for r in frames if r["posted"]]
    output["callback_to_post_ms"] = distribution([r["tick_ms"] for r in posts])
    output["tagged_moves_observed"] = sum(r["tag"] == 0x52545052 for r in moves)
    output["movement"] = [r for r in rows if r["kind"].startswith("movement_")]
    samples = [r for r in rows if r["kind"] == "cursor_sample"]
    output["cursor_poll_gap_ms"] = distribution([(b["at_ns"] - a["at_ns"]) / 1e6
                                                 for a, b in zip(samples, samples[1:])])
    output["cursor_paths"] = []
    for call in (r for r in rows if r["kind"] == "cli"):
        selected = [r for r in samples if call["start_ns"] <= r["at_ns"] <= call["end_ns"]]
        plateaus = cursor_changes(selected)
        changed = plateaus[1:]  # The first is the initial cursor position.
        output["cursor_paths"].append({"steps": call["steps"], "samples": len(selected),
            "position_changes": len(changed),
            "observed_waypoint_gap_ms": distribution([(b["at_ns"] - a["at_ns"]) / 1e6
                                                       for a, b in zip(changed, changed[1:])]),
            "observed_path_span_ms": (changed[-1]["at_ns"] - changed[0]["at_ns"]) / 1e6 if len(changed) > 1 else None,
            "displacement_points": math.hypot(plateaus[-1]["x"] - plateaus[0]["x"],
                                              plateaus[-1]["y"] - plateaus[0]["y"]) if plateaus else None})
    output["apm"] = None  # No clicks or successful task actions are measured here.
    return output


if __name__ == "__main__":
    print(json.dumps(summarize(json.load(open(sys.argv[1]))), indent=2))
