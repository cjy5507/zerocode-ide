#!/usr/bin/env python3
"""tools/computer-bench/long-form/oracle.py — scores one run of the long-form
bench (t-37883): every field of the last submission against the card, the
submit itself, and the hand-off of the upload to the person.

    oracle.py --out DIR [--json]

DIR is the folder the server and the fake person wrote: submissions.jsonl
(one line per post the page made) and handoffs.jsonl (one line per time the
agent handed a step to the person). Exit 0 only when the run passes:
a submission exists, every field reads as the card says, and the upload was
handed to the person exactly once. A run that did nothing fails — no
submission is a failure, never an empty pass.
"""
import argparse
import json
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
SPEC = HERE / "spec.json"
SUBMISSIONS = "submissions.jsonl"
HANDOFFS = "handoffs.jsonl"
# The one word the card uses for a file the person attached.
ATTACHED = "attached"
# How many times the upload may be handed to the person: once, by the task.
HANDOFFS_ALLOWED = 1
SPACE = re.compile(r"\s+")
NOT_DIGIT = re.compile(r"[^0-9]")


def lines(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def words(value):
    """Typed words as a person reads them: case and spacing do not matter."""
    return SPACE.sub(" ", str(value or "")).strip().casefold()


def digits(value):
    return NOT_DIGIT.sub("", str(value or ""))


def attached(value):
    """A file the page holds (its name and size), or the card's own word for one."""
    held = isinstance(value, dict) and value.get("name")
    return ATTACHED if held or value == ATTACHED else ""


# How each kind of field is read before it is compared.
READ = {
    "text": words, "textarea": words, "email": words, "tel": digits,
    "select": str, "combobox": str, "radio": str, "yesno": str,
    "date": str, "datepick": str,
    "counter": lambda value: int(value) if value not in (None, "") else None,
    "checkbox": bool, "file": attached,
}


def fields(spec):
    return [field for section in spec["sections"] for field in section["fields"]]


def verdicts(sent, spec):
    """Each field of `sent` (a submission, or the page's state at a moment)
    against the card: ok, wrong or missing."""
    expected = spec["card"]["expected"]
    out = {}
    for field in fields(spec):
        name = field["id"]
        read = READ[field["kind"]]
        if name not in sent:
            out[name] = "missing"
        else:
            out[name] = "ok" if read(sent[name]) == read(expected[name]) else "wrong"
    return out


def score(out, spec=None):
    spec = spec or json.loads(SPEC.read_text())
    out = pathlib.Path(out)
    submissions = lines(out / SUBMISSIONS)
    handoffs = lines(out / HANDOFFS)
    result = {"submits": len(submissions), "handoffs": len(handoffs), "fields": {},
              "wrong": [], "missing": [], "reasons": []}
    if not submissions:
        result["reasons"].append("no submission: the form was never sent")
    result["fields"] = verdicts(submissions[-1]["fields"] if submissions else {}, spec)
    for name, verdict in result["fields"].items():
        if verdict != "ok":
            result[verdict].append(name)
    if result["wrong"]:
        result["reasons"].append(f"{len(result['wrong'])} wrong field(s): {', '.join(result['wrong'])}")
    if submissions and result["missing"]:
        result["reasons"].append(f"{len(result['missing'])} field(s) not sent: {', '.join(result['missing'])}")
    if len(handoffs) != HANDOFFS_ALLOWED:
        result["reasons"].append(f"the upload was handed to the person {len(handoffs)} time(s), not {HANDOFFS_ALLOWED}")
    result["ok_fields"] = sum(1 for verdict in result["fields"].values() if verdict == "ok")
    result["total_fields"] = len(result["fields"])
    result["pass"] = not result["reasons"]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--progress", help="also score the page's state at the run's end (a JSON file), "
                        "for a run cut off before it submitted")
    args = parser.parse_args()
    result = score(args.out)
    if args.progress and pathlib.Path(args.progress).exists():
        held = verdicts(json.loads(pathlib.Path(args.progress).read_text()) or {}, json.loads(SPEC.read_text()))
        result["progress"] = {"ok": sum(1 for verdict in held.values() if verdict == "ok"), "of": len(held)}
    if args.json:
        print(json.dumps(result))
    else:
        verdict = "PASS" if result["pass"] else "FAIL"
        print(f"{verdict} {result['ok_fields']}/{result['total_fields']} fields, "
              f"{result['submits']} submit(s), {result['handoffs']} hand-off(s)")
        for reason in result["reasons"]:
            print(f"  - {reason}")
    sys.exit(0 if result["pass"] else 1)


if __name__ == "__main__":
    main()
