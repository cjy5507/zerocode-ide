#!/usr/bin/env python3
"""Controlled page step, using only the window's browser and existing model roads.

Not a replacement walker: the cached arm is a probe-local decision memo, and
the large arm is one minimal model request, not a whole agent transcript.
Those boundaries belong in any report made from these rows.
"""
import argparse
import http.client
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

from probe import timed

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("value_latency", ROOT / "tools/type_value_latency.py")
VALUE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALUE)


class Jev:
    def __init__(self):
        self.conn = http.client.HTTPSConnection("api.typesafe.ai", timeout=20)
        self.calls = 0

    def ask(self, state, questions):
        if self.calls >= 200:
            raise RuntimeError("probe call budget exhausted")
        self.calls += 1
        payload = json.dumps(dict(model="jev-latest", state=state, questions=questions))
        self.conn.request("POST", "/v1/systemone", payload,
                          {"Authorization": "Bearer " + os.environ["TYPESAFE_API_KEY"],
                           "Content-Type": "application/json"})
        response = self.conn.getresponse()
        body = response.read()
        if response.status != 200:
            raise RuntimeError("Jev HTTP " + str(response.status))
        return json.loads(body)


def browser(pane, verb, *args, input_text=None):
    done = subprocess.run(["zerocode-browser", verb, pane, *args], input=input_text,
                          capture_output=True, text=True, timeout=30, check=True)
    return done.stdout.strip()


def unframe(text):
    # Browser eval wraps untrusted page data in delimiter lines.
    lines = [line for line in text.splitlines() if not line.startswith("<<<")]
    value = json.loads("\n".join(lines))
    if isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            pass
    return value


def question(kind, instructions, criteria):
    return dict(type=kind, instructions=instructions, criteria=criteria)


def run(args):
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    assert os.environ.get("ZO_CONFIG_HOME"), "use a temporary configuration home"
    jev = Jev()
    token = VALUE.credentials().get("oauth", {}).get("accessToken")
    large = VALUE.AnthropicRoad(token) if token and args.large_model else None
    small = VALUE.AnthropicRoad(token) if token and args.large_model else None
    for road in (large, small):
        if road:
            send = road.session.stream
            def without_temperature(path, headers, body, pick, send=send):
                body.pop("temperature", None)
                return send(path, headers, body, pick)
            road.session.stream = without_temperature
    table = json.loads((VALUE.FIXTURES / "models.json").read_text())
    small_model = next(row["model"] for row in table["rows"] if row["id"] == table["chosen"])
    goal = "Press the Advance button exactly once. Return only its mark number."
    q = {
        "action": question("choice", goal, {"mark:1": "City input", "mark:2": "Advance button", "give_up": "No suitable button"}),
        "ready": question("noul", "Is there an Advance button?", {"true": "Yes", "false": "No"}),
        "field": question("choice", "Which field receives the city?", {"destination": "Destination City input", "none": "No field"}),
    }
    # Renderer uses the product's question keys; retain its exact shape.
    base_question = json.loads((VALUE.FIXTURES / "question.json").read_text())
    case = dict(goal=goal, field=dict(label="mark number", placeholder="", near="1 City input; 2 Advance button"))
    value_case = dict(goal="Set the destination to London.",field=dict(label="Destination",placeholder="City",near="Travel search"))
    memo = {}
    rows = []
    def record(row):
        row["load"] = os.getloadavg()[0]
        rows.append(row)
        with (output / "web.jsonl").open("a") as f:
            f.write(json.dumps(row) + "\n")
    arms = ["large", "jev", "memo", "serial3", "batch3"] if large else ["jev", "memo", "serial3", "batch3"]
    if args.extra:
        arms += ["timing3", "macro2"]
    if args.types_only:
        arms = []
    try:
        # Warm once, recorded separately rather than discarded silently.
        warm, ms = timed(lambda: jev.ask({"controls":["City input", "Advance button"]}, {"action":q["action"]}))
        record(dict(kind="warm", arm="jev", judge_ms=ms, model=warm.get("model")))
        for iteration in range(args.rounds):
            for arm in (arms if iteration % 2 == 0 else list(reversed(arms))):
                before = unframe(browser(args.pane, "eval", "JSON.stringify(probe)"))
                hidden = unframe(browser(args.pane, "eval", "JSON.stringify(document.hidden)"))
                start = time.perf_counter_ns()
                marks, marks_ms = timed(lambda: json.loads(browser(args.pane, "marks", "--json")))
                read_ms = 0
                if args.read_full:
                    _, read_ms = timed(lambda: browser(args.pane, "read", "--full"))
                state = {"goal":goal,"controls":[{k:v for k,v in item.items() if k in ("mark","role","label")} for item in marks["items"]]}
                if arm == "timing3":
                    state["reaction_timing"] = {"observation_age_ms": (time.perf_counter_ns()-start)/1e6,
                                                "last_step_ms": rows[-1].get("total_ms") if rows else None}
                if arm == "macro2":
                    state["goal"] = "Execute the offered Advance-twice macro."
                memo_key = json.dumps(state, sort_keys=True)
                calls_before = jev.calls
                model = None
                def judge():
                    nonlocal memo, model
                    if arm == "large":
                        first, total, said, kind = large.ask(args.large_model, base_question, case)
                        return {"large":said.strip(), "first_ms":first,"done_ms":total,"first_kind":kind}
                    if arm == "memo" and memo_key in memo:
                        return json.loads(json.dumps(memo[memo_key]))
                    if arm == "serial3":
                        answer = {}
                        for name, one in q.items():
                            got = jev.ask(state, {name:one}); model=got.get("model")
                            answer.update(got["answers"])
                        return answer
                    questions = q if arm in ("batch3", "timing3") else {"action":q["action"]}
                    if arm == "macro2":
                        questions = {"action":question("choice", state["goal"],
                                    {"macro:advance2":"Press the observed Advance button twice, checking each step", "give_up":"No suitable button"})}
                    got = jev.ask(state, questions)
                    model=got.get("model")
                    memo[memo_key] = got["answers"]
                    return got["answers"]
                answer, judge_ms = timed(judge)
                chosen = answer.get("large", "") if arm == "large" else answer.get("action",{}).get("choice", "")
                valid = chosen in ("2", "mark:2") if arm != "macro2" else chosen == "macro:advance2"
                actions = 2 if arm == "macro2" else 1
                if valid:
                    click_ms=verify_ms=0
                    # Wait on the fixture's two-rAF receipt, with no fixed settle sleep.
                    def verify():
                        until = time.monotonic() + 3
                        while time.monotonic() < until:
                            after = unframe(browser(args.pane,"eval","JSON.stringify(probe)"))
                            if after["count"] == before["count"]+action+1 and (not args.wait_paint or after["painted"] >= after["clicked"]):
                                return after
                        raise TimeoutError("fixture failed to paint")
                    for action in range(actions):
                        _, one_click = timed(lambda: browser(args.pane,"click","--mark","2"))
                        after, one_verify = timed(verify)
                        click_ms+=one_click;verify_ms+=one_verify
                else:
                    click_ms=verify_ms=0; after=before
                record(dict(kind="step",index=iteration,arm=arm,marks_ms=marks_ms,read_ms=read_ms,
                            judge_ms=judge_ms,click_ms=click_ms,verify_ms=verify_ms,
                            total_ms=(time.perf_counter_ns()-start)/1e6,success=valid,
                            calls=jev.calls-calls_before,model=model,large_model=args.large_model if arm=="large" else None,
                            hidden=hidden,wait_paint=args.wait_paint,read_full=args.read_full,actions=actions,
                            paint_ms=after["painted"]-after["clicked"] if valid and after["painted"]>=after["clicked"] else None))
            if small:
                type_arms = [("small",small,small_model),("large",large,args.large_model)]
                for size, road, model in (type_arms if iteration % 2 == 0 else list(reversed(type_arms))):
                    if args.select_field:
                        browser(args.pane,"type","#destination","--value-stdin",input_text="")
                    start = time.perf_counter_ns()
                    observed, read_ms = timed(lambda: json.loads(browser(args.pane,"marks","--json"))) if args.select_field else timed(lambda: browser(args.pane,"read","--full"))
                    choose_ms=0
                    if args.select_field:
                        type_questions={
                            "operation":question("choice","Select the operation needed to enter the destination city.",{"TYPE_TEXT":"Enter text in the city field", "BLOCKED":"No suitable editable field"}),
                            "target":question("choice","Select the observed city field.",{f"mark:{i['mark']}":i.get('label','') for i in observed['items'] if i['role']=='textbox'})}
                        picked,choose_ms=timed(lambda:jev.ask({"goal":value_case['goal'],"controls":observed['items']},type_questions))
                        if picked['answers']['operation']['choice']!='TYPE_TEXT' or picked['answers']['target']['choice']!='mark:1':
                            raise RuntimeError('field selection failed')
                    said, model_ms = timed(lambda: road.ask(model,base_question,value_case))
                    valid = said[2].strip() == "London"
                    type_ms=verify_ms=0
                    if valid:
                        _, type_ms=timed(lambda: browser(args.pane,"type","#destination","--value-stdin",input_text=said[2].strip()))
                        got, verify_ms=timed(lambda: unframe(browser(args.pane,"eval","JSON.stringify(document.querySelector('#destination').value)")))
                        valid = got == "London"
                    record(dict(kind="type_selected" if args.select_field else "type",arm=size,index=iteration,model=model,read_ms=read_ms,
                                choose_ms=choose_ms,model_ms=model_ms,type_ms=type_ms,verify_ms=verify_ms,success=valid,
                                total_ms=(time.perf_counter_ns()-start)/1e6))
    finally:
        jev.conn.close()
        if large: large.close(); small.close()
    print(json.dumps({"rows":len(rows),"jev_calls":jev.calls}))


if __name__ == "__main__":
    p=argparse.ArgumentParser()
    p.add_argument("--pane", required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--large-model")
    p.add_argument("--rounds", type=int, default=8)
    p.add_argument("--wait-paint", action="store_true")
    p.add_argument("--read-full", action="store_true")
    p.add_argument("--extra", action="store_true", help="also measure timing state and an observed two-click macro")
    p.add_argument("--select-field", action="store_true", help="include a real batched operation and field selection before generating text")
    p.add_argument("--types-only", action="store_true")
    run(p.parse_args())
