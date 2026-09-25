#!/usr/bin/env python3
"""The browser door's look and settle, timed before and after a change (t-6721).

One command, one table: the page scripts `cmd/browser.rs` hands a pane, as
they stand at `--before <rev>` and as they stand in this checkout, run turn
about in the SAME hidden tab of the running window through its own `eval`
road — so the two sides pay the same CLI, bridge and callback — plus the
installed `marks --json` verb as the calibration of that road.

What is measured, per page and arm (`--rounds`, arms alternated ABBA):

- the look: wall ms per call, the page's own ms for the script, the answer's
  bytes, and what it carried (numbers, fields, candidates, a document epoch);
- the settle after a press: the product's settle policy (the core's
  `settle_verdict` rule, read from the Rust constants and driven here poll
  by poll through the same eval road) — its verdict, wall ms and polls — and
  whether the look right after the press saw what the press did (stale or
  not), against the look taken at once with no settle, the road before;
- focus: the frontmost app and the tab's own `hasFocus()`/active element
  around every call — a change is a steal.

The settle's polls here each cross the CLI (tens of milliseconds); in the
product they are in-process callbacks, so wall numbers here are an upper
bound of the product's and the page-clock numbers are the settle's own.
Nothing is written to the person's pages: the probe opens its own tabs on
its own fixtures (`pages/`) and closes them. Outputs go to `--out`, outside
git. No model and no key is used.
"""
import argparse
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
PAGES = Path(__file__).resolve().parent / "pages"
DOOR = "crates/zerocode-shell/src/cmd/browser.rs"
CORE = "crates/zerocode-core/src/agent_browser.rs"
SCREEN = "crates/zerocode-core/src/screen_action.rs"
JEV = "crates/zerocode-core/src/jev.rs"
VALUE_QUESTION = "crates/zerocode-core/fixtures/type-value/question.json"
GUEST_KEY = "ui/browser-guest-key.txt"


# ---- the Rust the pane runs, read at a revision --------------------------------

def source(rev, path):
    """A file's text at `rev` (None: this checkout)."""
    if rev is None:
        return (ROOT / path).read_text()
    done = subprocess.run(["git", "show", f"{rev}:{path}"], cwd=ROOT,
                          capture_output=True, text=True, check=True)
    return done.stdout


def rust_text(text, name):
    """A Rust `&str` constant's text, raw or plain; None when absent."""
    raw = re.search(r"const " + name + r': &str = r(#+)"([\s\S]*?)"\1;', text)
    if raw:
        return raw.group(2)
    plain = re.search(r"const " + name + r': &str = "((?:[^"\\]|\\.)*)";', text)
    return json.loads('"' + plain.group(1) + '"') if plain else None


def rust_list(text, name):
    held = re.search(r"const " + name + r": (?:&\[&str\]|\[&str; \d+\]) = &?\[([\s\S]*?)\];", text)
    if not held:
        return None
    return [json.loads('"' + word + '"') for word in re.findall(r'"((?:[^"\\]|\\.)*)"', held.group(1))]


def rust_number(text, name):
    held = re.search(r"const " + name + r": \w+ = ([\d_.]+);", text)
    return float(held.group(1).replace("_", "")) if held else None


def observe_key(screen, head):
    return re.search(r"pub const fn key\(self\)[\s\S]*?Self::" + head + r' => "(\w+)"', screen).group(1)


class Scripts:
    """The door's page scripts at one revision, assembled as `automation_script`
    assembles them: helpers, the request, the body inside one try."""

    def __init__(self, rev):
        self.rev = rev or "HEAD+worktree"
        door = source(rev, DOOR)
        core = source(rev, CORE)
        self.helpers = rust_text(door, "BROWSER_AUTOMATION_HELPERS")
        self.mark_helpers = rust_text(door, "BROWSER_MARK_HELPERS")
        self.marks_body = rust_text(door, "BROWSER_MARKS_BODY")
        self.click_body = rust_text(door, "CLICK_BODY")
        self.observe = rust_text(door, "BROWSER_OBSERVE_HELPERS") or ""
        self.settle_body = rust_text(door, "BROWSER_SETTLE_BODY") or ""
        self.snapshot = bool(self.observe)
        self.request = {"selectors": rust_list(core, "BROWSER_MARKABLE"),
                        "answerCap": int(rust_number(door, "BROWSER_CALLBACK_CAP"))}
        if self.snapshot:
            screen = source(rev, SCREEN)
            cap = int(rust_number(source(rev, JEV), "SCREEN_CANDIDATE_CAP"))

            def key(name):
                return rust_text(screen, name)
            self.request.update({
                "keys": {
                    "epoch": key("EPOCH_KEY"), "kind": key("FIELD_KIND_KEY"),
                    "secret": key("FIELD_SECRET_KEY"), "label": key("LABEL_KEY"),
                    "placeholder": key("FIELD_PLACEHOLDER_KEY"), "near": key("FIELD_NEAR_KEY"),
                    "value": key("FIELD_VALUE_KEY"), "role": key("ROLE_KEY"),
                    "count": key("COUNT_KEY"), "alt": key("ALT_KEY"), "width": key("WIDTH_KEY"),
                    "height": key("HEIGHT_KEY"), "text": key("TEXT_KEY"),
                    "selector": key("SELECTOR_KEY"),
                    "containers": observe_key(screen, "Container"),
                    "images": observe_key(screen, "Image"), "rows": observe_key(screen, "Row"),
                },
                "observed": {
                    "containers": rust_list(core, "BROWSER_OBSERVED_CONTAINERS"),
                    "rows": rust_list(core, "BROWSER_OBSERVED_ROWS"),
                    "images": rust_list(core, "BROWSER_OBSERVED_IMAGES"),
                    "chrome": rust_list(core, "BROWSER_OBSERVED_CHROME"),
                    "cap": cap,
                    "imageMinPx": rust_number(core, "BROWSER_OBSERVED_IMAGE_MIN_PX"),
                },
                "field": {"regions": rust_list(core, "BROWSER_FIELD_REGIONS"),
                          "headings": rust_list(core, "BROWSER_FIELD_HEADINGS")},
                "wordCap": int(rust_number(screen, "SHOWS_CHAR_CAP")) // cap,
                "valueCap": json.loads(source(rev, VALUE_QUESTION))["valueCharCap"],
            })
            self.epoch_key = self.request["keys"]["epoch"]
            self.quiet_ms = rust_number(core, "BROWSER_SETTLE_QUIET_MS")
            self.settle_ms = rust_number(core, "BROWSER_SETTLE_MS")
            self.busy = ",".join(rust_list(core, "BROWSER_SETTLE_BUSY"))
        self.watch = source(rev, GUEST_KEY).strip()

    def assembled(self, request, body):
        return ("(() => {\n" + self.helpers + "\nconst request = " + json.dumps(request)
                + ";\ntry {\n" + body + '\n} catch (_) { return zcFail("evaluation_failed"); }\n})()')

    def look(self):
        body = (self.observe + "\n" if self.snapshot else "") + self.mark_helpers + "\n" + self.marks_body
        return self.assembled(self.request, body)

    def press(self, selector, expect=None):
        request = {"selector": selector, "blockRoots": "main"}
        body = self.click_body
        if expect is not None:
            request["expect"] = expect
            body = self.observe + "\n" + body
        return self.assembled(request, body)

    def settle(self):
        return self.assembled({"watch": self.watch, "busy": self.busy},
                              self.observe + "\n" + self.settle_body)


# ---- the road: the window's own CLI ------------------------------------------------

def browser(*words, timeout=30):
    done = subprocess.run(["zerocode-browser", *words], capture_output=True, text=True, timeout=timeout)
    if done.returncode != 0:
        raise RuntimeError("zerocode-browser " + words[0] + " refused: " + done.stderr.strip()[:200])
    return done.stdout


def unframe(text):
    """The eval answer inside its untrusted-content fence, decoded to the value."""
    lines = [line for line in text.splitlines() if not line.startswith("<<<")]
    value = json.loads("\n".join(lines))
    while isinstance(value, str):
        try:
            value = json.loads(value)
        except json.JSONDecodeError:
            break
    return value


# The page-side wrapper: run one assembled script, time it on the page's own
# clock, and hand back only a summary (an eval answer is capped far below a
# look's).
SUMMARY = """(() => {
  const focus0 = document.hasFocus();
  const active0 = document.activeElement ? (document.activeElement.id || document.activeElement.tagName) : null;
  const scroll0 = [window.scrollX, window.scrollY].join(",");
  const t0 = performance.now();
  const raw = %s;
  const ms = performance.now() - t0;
  const said = JSON.parse(raw);
  const value = said.ok ? said.value : null;
  const faces = value && Array.isArray(value.faces) ? value.faces : [];
  const seen = value && value.snapshot ? value.snapshot : null;
  const count = (key) => seen && Array.isArray(seen[key]) ? seen[key].length : 0;
  return JSON.stringify({ ms, bytes: raw.length, ok: said.ok === true, code: said.code || null,
    faces: faces.length, hits: faces.filter((face) => face.hit).length,
    fields: faces.filter((face) => face.field).length,
    epoch: seen ? seen[%s] || null : null,
    containers: count(%s), images: count(%s), rows: count(%s),
    results: faces.filter((face) => /^#book-/.test(face.selector)).length,
    hidden: document.hidden === true, focus0, focus: document.hasFocus(), active0,
    active: document.activeElement ? (document.activeElement.id || document.activeElement.tagName) : null,
    scrolled: scroll0 !== [window.scrollX, window.scrollY].join(","),
    pressedAt: value && typeof value.pressedAt === "number" ? value.pressedAt : null,
    settle: value && typeof value.now === "number" ? value : null });
})()"""


def run_summary(pane, scripts, script):
    keys = scripts.request.get("keys", {})
    expr = SUMMARY % (script, json.dumps(keys.get("epoch", "documentEpoch")),
                      json.dumps(keys.get("containers", "containers")),
                      json.dumps(keys.get("images", "images")), json.dumps(keys.get("rows", "rows")))
    began = time.perf_counter()
    said = unframe(browser("eval", pane, expr))
    said["wall_ms"] = (time.perf_counter() - began) * 1000
    return said


def focus_of(said):
    """What one call left of the tab's focus, active element and scroll."""
    return {key: said.get(key) for key in ("focus0", "focus", "active0", "active", "scrolled")}


def frontmost():
    try:
        front = subprocess.run(["lsappinfo", "front"], capture_output=True, text=True, timeout=5).stdout.strip()
        name = subprocess.run(["lsappinfo", "info", "-only", "name", front],
                              capture_output=True, text=True, timeout=5).stdout
        return name.split("=", 1)[-1].strip().strip('"')
    except (OSError, subprocess.SubprocessError):
        return None


# ---- the settle: the core's rule, poll by poll -------------------------------------

def still_for(facts, since):
    return facts["now"] - max(facts["last"], since)


def settle_verdict(facts, epoch, since, quiet):
    """`zerocode_core::agent_browser::settle_verdict`, as data: the same order
    and the same words (the unit test holds this copy to the Rust's cases)."""
    if facts.get("documentEpoch") != epoch:
        return ("invalidated", "replaced")
    if not facts.get("watched"):
        return ("not_ready", "unwatched")
    if facts.get("busy"):
        return None
    return ("ready", "quiet") if still_for(facts, since) >= quiet else None


def settle(pane, scripts, epoch, since):
    """The product's settle loop (`settle_with`), each poll one eval."""
    began = time.perf_counter()
    wall = scripts.settle_ms / 1000
    polls, why, facts, seen = 0, "unanswered", None, []
    while True:
        if time.perf_counter() - began >= wall:
            return {"state": "not_ready", "why": why, "polls": polls,
                    "wall_ms": (time.perf_counter() - began) * 1000, "facts": facts, "seen": seen}
        polls += 1
        said = run_summary(pane, scripts, scripts.settle())
        seen.append(focus_of(said))
        facts = said.get("settle")
        if not facts:
            why = "unanswered"
            continue
        verdict = settle_verdict(facts, epoch, since, scripts.quiet_ms)
        if verdict:
            return {"state": verdict[0], "why": verdict[1], "polls": polls,
                    "wall_ms": (time.perf_counter() - began) * 1000, "facts": facts,
                    "page_ms": facts["now"] - since, "seen": seen}
        why = "busy" if facts.get("busy") else "moving"
        wait = scripts.quiet_ms if facts.get("busy") else max(1.0, scripts.quiet_ms - still_for(facts, since))
        time.sleep(min(wait / 1000, max(0.0, wall - (time.perf_counter() - began))))


# ---- the run ---------------------------------------------------------------------

def percentile(values, share):
    """Nearest rank; None for no values."""
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, -(-len(ordered) * share // 100))
    return ordered[int(rank) - 1]


def abba(arms, rounds):
    """Arm order per round: forward, then back — ABBA over the rounds."""
    return [list(arms) if index % 2 == 0 else list(reversed(arms)) for index in range(rounds)]


def page_url(page):
    """A fixture's address, its query kept apart from the file's name."""
    name, _, query = page.partition("?")
    return (PAGES / name).as_uri() + ("?" + query if query else "")


def open_tab(url):
    said = browser("open", url)
    label = re.search(r"(browser-\d+)", said)
    if not label:
        raise RuntimeError("open answered no label: " + said.strip()[:120])
    label = label.group(1)
    for _ in range(50):
        line = next((line for line in browser("tabs").splitlines() if label in line.split()), "")
        if "finished" in line:
            break
        time.sleep(0.1)
    return label


def run(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    before, after = Scripts(args.before), Scripts(None)
    assert after.snapshot, "this checkout carries no snapshot to measure"
    rows = []

    def record(row):
        row["load"] = os.getloadavg()[0]
        rows.append(row)
        with (out / "rows.jsonl").open("a") as held:
            held.write(json.dumps(row) + "\n")

    opened = []
    try:
        # -- the look ----------------------------------------------------------
        for page in args.look_pages:
            pane = open_tab(page_url(page))
            opened.append(pane)
            for index, order in enumerate(abba(["before", "after", "verb"], args.rounds)):
                for arm in order:
                    front = frontmost()
                    began = time.perf_counter()
                    try:
                        if arm == "verb":
                            text = browser("marks", pane, "--json")
                            answer = json.loads(text)
                            said = {"wall_ms": (time.perf_counter() - began) * 1000, "ok": True,
                                    "faces": len(answer.get("items", [])), "bytes": len(text)}
                        else:
                            scripts = before if arm == "before" else after
                            said = run_summary(pane, scripts, scripts.look())
                    except (RuntimeError, subprocess.SubprocessError, json.JSONDecodeError) as refused:
                        # A call the window did not answer is a row, never a gap.
                        said = {"wall_ms": (time.perf_counter() - began) * 1000, "ok": False,
                                "error": str(refused)[:160]}
                    said.pop("settle", None)
                    observations = [focus_of(said)] if arm != "verb" else []
                    record(dict(kind="look", page=page, arm=arm, round=index, **said,
                                observations=observations, front_before=front, front_after=frontmost()))
        # -- the settle after a press ------------------------------------------
        for page, selector in args.settle_pages:
            pane = open_tab(page_url(page))
            opened.append(pane)
            # Whether a look saw the press is read where the press makes
            # controls (the form's results); elsewhere it is not a question.
            answers = page.startswith("form")
            for index, order in enumerate(abba(["before", "after"], args.rounds)):
                for arm in order:
                    front = frontmost()
                    if arm == "before":
                        pressed = run_summary(pane, before, before.press(selector))
                        look = run_summary(pane, before, before.look())
                        record(dict(kind="settle", page=page, arm=arm, round=index, state="none",
                                    look_results=look["results"] if answers else None,
                                    look_hidden=look["hidden"], press_ok=pressed["ok"],
                                    presses=[focus_of(pressed)], observations=[focus_of(look)],
                                    front_before=front, front_after=frontmost()))
                        continue
                    first = run_summary(pane, after, after.look())
                    epoch = first["epoch"]
                    pressed = run_summary(pane, after, after.press(
                        selector, {"epoch": epoch, "value": None, "watch": after.watch}))
                    done = settle(pane, after, epoch, pressed["pressedAt"] or float("-inf"))
                    look = run_summary(pane, after, after.look())
                    record(dict(kind="settle", page=page, arm=arm, round=index, state=done["state"],
                                why=done["why"], polls=done["polls"], wall_ms=done["wall_ms"],
                                page_ms=done.get("page_ms"),
                                look_results=look["results"] if answers else None,
                                look_hidden=look["hidden"], press_ok=pressed["ok"],
                                presses=[focus_of(pressed)],
                                observations=[focus_of(first), *done["seen"], focus_of(look)],
                                front_before=front, front_after=frontmost()))
        # -- the hidden tab's own timers ----------------------------------------
        pane = open_tab(page_url("normal.html"))
        opened.append(pane)
        for index in range(args.rounds):
            for asked in args.timer_ms:
                browser("eval", pane, "(() => { window.__timer = { at: performance.now(), fired: null }; "
                        "setTimeout(() => { window.__timer.fired = performance.now() - window.__timer.at; }, "
                        + str(asked) + "); return true; })()")
                began, fired = time.perf_counter(), None
                while time.perf_counter() - began < args.timer_deadline_ms / 1000:
                    fired = unframe(browser("eval", pane, "JSON.stringify(window.__timer)"))["fired"]
                    if fired is not None:
                        break
                    time.sleep(0.02)
                record(dict(kind="timer", page="normal.html", arm="hidden", round=index, asked=asked,
                            fired_ms=fired, deadline_ms=args.timer_deadline_ms))
        # -- the document replaced under a look --------------------------------
        pane = open_tab(page_url("normal.html"))
        opened.append(pane)
        for index in range(args.rounds):
            first = run_summary(pane, after, after.look())
            pressed = run_summary(pane, after, after.press(
                "#advance", {"epoch": first["epoch"], "value": None, "watch": after.watch}))
            browser("goto", pane, page_url("normal.html?n=" + str(index)))
            time.sleep(0.3)
            done = settle(pane, after, first["epoch"], pressed["pressedAt"] or float("-inf"))
            stale = run_summary(pane, after, after.press(
                "#advance", {"epoch": first["epoch"], "value": None, "watch": after.watch}))
            record(dict(kind="replaced", page="normal.html", arm="after", round=index,
                        state=done["state"], why=done["why"], wall_ms=done["wall_ms"],
                        stale_press=stale.get("code")))
    finally:
        for pane in opened:
            try:
                browser("close", pane)
            except (RuntimeError, subprocess.SubprocessError):
                pass
    summary = summarize(rows)
    (out / "summary.json").write_text(json.dumps(summary, indent=2))
    (out / "table.md").write_text(table(summary))
    print(table(summary))


def summarize(rows):
    summary = {"look": {}, "settle": {}, "replaced": {}, "focus": {}, "timers": {}}
    for row in rows:
        if row["kind"] == "timer":
            summary["timers"].setdefault(str(row["asked"]), []).append(row["fired_ms"])
    for asked, fired in list(summary["timers"].items()):
        came = [ms for ms in fired if ms is not None]
        # A timer that did not fire inside the deadline is censored: counted
        # as not fired, never given the deadline as its time.
        summary["timers"][asked] = {"n": len(fired), "fired": len(came),
                                    "p50": percentile(came, 50), "max": max(came) if came else None}
    for row in rows:
        if row["kind"] == "look":
            held = summary["look"].setdefault(row["page"], {}).setdefault(row["arm"], [])
            held.append(row)
    for page, arms in summary["look"].items():
        for arm, held in list(arms.items()):
            walls = [row["wall_ms"] for row in held if row.get("ok")]
            pages_ms = [row["ms"] for row in held if row.get("ms") is not None]
            arms[arm] = {"n": len(held), "wall_p50": percentile(walls, 50), "wall_p95": percentile(walls, 95),
                         "page_p50": percentile(pages_ms, 50), "page_p95": percentile(pages_ms, 95),
                         "bytes": percentile([row["bytes"] for row in held if row.get("bytes")], 50),
                         "faces": held[0].get("faces"), "fields": held[0].get("fields"),
                         "containers": held[0].get("containers"), "images": held[0].get("images"),
                         "rows": held[0].get("rows"), "epoch": bool(held[0].get("epoch")),
                         "refused": sum(1 for row in held if not row.get("ok")),
                         "hidden": all(row.get("hidden", True) for row in held if "hidden" in row),
                         "load": [min(row["load"] for row in held), max(row["load"] for row in held)]}
    for row in rows:
        if row["kind"] == "settle":
            summary["settle"].setdefault(row["page"], {}).setdefault(row["arm"], []).append(row)
    for page, arms in summary["settle"].items():
        for arm, held in list(arms.items()):
            walls = [row["wall_ms"] for row in held if row.get("wall_ms") is not None]
            states = {}
            for row in held:
                states[row["state"]] = states.get(row["state"], 0) + 1
            arms[arm] = {"n": len(held), "states": states,
                         "wall_p50": percentile(walls, 50), "wall_p95": percentile(walls, 95),
                         "page_p50": percentile([row["page_ms"] for row in held if row.get("page_ms") is not None], 50),
                         "polls_p50": percentile([row["polls"] for row in held if row.get("polls")], 50),
                         "look_saw_press": None if held[0]["look_results"] is None
                         else sum(1 for row in held if row["look_results"] > 0),
                         "hidden": all(row["look_hidden"] for row in held)}
    replaced = [row for row in rows if row["kind"] == "replaced"]
    summary["replaced"] = {"n": len(replaced),
                           "states": {state: sum(1 for row in replaced if row["state"] == state)
                                      for state in {row["state"] for row in replaced}},
                           "stale_press_refused": sum(1 for row in replaced if row["stale_press"] == "document_replaced")}
    fronts = [row for row in rows if "front_before" in row]
    observed = [call for row in rows for call in row.get("observations", [])]
    summary["focus"] = {"calls": len(fronts),
                        "front_changed": sum(1 for row in fronts if row["front_before"] != row["front_after"]),
                        "observations": len(observed),
                        "observation_took_focus": sum(1 for call in observed if call["focus"] and not call["focus0"]),
                        "observation_moved_active": sum(1 for call in observed if call["active"] != call["active0"]),
                        "observation_scrolled": sum(1 for call in observed if call["scrolled"]),
                        "press_took_focus": sum(1 for row in rows for call in row.get("presses", [])
                                                if call["focus"] and not call["focus0"])}
    return summary


def fmt(value):
    return "—" if value is None else (f"{value:,.1f}" if isinstance(value, float) else str(value))


def table(summary):
    lines = ["## Look (one IPC call through the window's eval road; wall p50/p95 ms, page p50 ms)", "",
             "| page | arm | n | wall p50 | wall p95 | page p50 | bytes | faces | fields | containers/images/rows | epoch | refused | load |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for page, arms in summary["look"].items():
        for arm, held in arms.items():
            lines.append(f"| {page} | {arm} | {held['n']} | {fmt(held['wall_p50'])} | {fmt(held['wall_p95'])} | "
                         f"{fmt(held['page_p50'])} | {fmt(held['bytes'])} | {fmt(held['faces'])} | {fmt(held['fields'])} | "
                         f"{fmt(held['containers'])}/{fmt(held['images'])}/{fmt(held['rows'])} | {held['epoch']} | "
                         f"{held['refused']} | {held['load'][0]:.1f}–{held['load'][1]:.1f} |")
    lines += ["", "## Settle after a press (hidden tab)", "",
              "| page | arm | n | states | wall p50/p95 ms | page-clock p50 ms | polls p50 | look saw the press |",
              "|---|---|---|---|---|---|---|---|"]
    for page, arms in summary["settle"].items():
        for arm, held in arms.items():
            lines.append(f"| {page} | {arm} | {held['n']} | {json.dumps(held['states'])} | "
                         f"{fmt(held['wall_p50'])} / {fmt(held['wall_p95'])} | {fmt(held['page_p50'])} | "
                         f"{fmt(held['polls_p50'])} | "
                         f"{'—' if held['look_saw_press'] is None else str(held['look_saw_press']) + '/' + str(held['n'])} |")
    replaced = summary["replaced"]
    lines += ["", f"Document replaced after a press: {json.dumps(replaced['states'])}; a stale press refused "
              f"`document_replaced` {replaced['stale_press_refused']}/{replaced['n']}.",
              f"Focus: {summary['focus']['front_changed']} frontmost-app changes over {summary['focus']['calls']} rows; "
              f"of {summary['focus']['observations']} looks and settle polls, {summary['focus']['observation_took_focus']} "
              f"took the tab's focus, {summary['focus']['observation_moved_active']} moved its active element, "
              f"{summary['focus']['observation_scrolled']} scrolled it; presses that took the tab's focus: "
              f"{summary['focus']['press_took_focus']}.", ""]
    timers = summary.get("timers")
    if timers:
        lines += ["## The hidden tab's own timers (what a page that answers later waits on)", "",
                  "| asked ms | n | fired | fired p50 ms | fired max ms |", "|---|---|---|---|---|"]
        for asked, held in timers.items():
            lines.append(f"| {asked} | {held['n']} | {held['fired']} | {fmt(held['p50'])} | {fmt(held['max'])} |")
        lines.append("")
    return "\n".join(lines)


def parse():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--before", required=True, help="the revision whose page scripts are 'before'")
    parser.add_argument("--out", required=True, help="a folder outside git for rows, summary and table")
    parser.add_argument("--rounds", type=int, default=12)
    parser.add_argument("--timer-ms", nargs="+", type=int, default=[0, 50, 120])
    parser.add_argument("--timer-deadline-ms", type=int, default=3000)
    parser.add_argument("--look-pages", nargs="+",
                        default=["normal.html", "results.html", "results.html?cards=400"])
    parser.add_argument("--settle-pages", nargs="+", type=lambda word: tuple(word.split("@", 1)),
                        default=[("normal.html", "#advance"), ("form.html", "#search"),
                                 ("form.html?delay=0", "#search"), ("moving.html", "#advance")],
                        help="page@selector pairs to press and settle")
    return parser.parse_args()


if __name__ == "__main__":
    run(parse())
