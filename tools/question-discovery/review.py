#!/usr/bin/env python3
"""Frozen, paired question studies over `zo jev export` review evidence.

This is an offline review boundary: no agent is launched, no model is called,
and no production setting is changed. Requests for a candidate are emitted
separately from labels. A held-out comparison can be opened only once, after
the candidate has been frozen. Acceptance here means a candidate is ready for
code review; runtime validation and latency/cost measurements remain separate.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import importlib.util
import json
import os
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
MAX_BYTES = 128 * 1024 * 1024
MAX_CASES = 512
MIN_DEV_GROUPS = 5
MIN_HELD_GROUPS = 5

_spec = importlib.util.spec_from_file_location("jev_question_review_loop", Path(__file__).with_name("loop.py"))
loop = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = loop
_spec.loader.exec_module(loop)


class Refused(ValueError):
    """Evidence or a study transition is not sufficient for a comparison."""


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def read(path, with_digest=False):
    with Path(path).open("rb") as source:
        body = source.read(MAX_BYTES + 1)
    if len(body) > MAX_BYTES:
        raise Refused("study input exceeds its byte limit")
    value = json.loads(body, parse_constant=lambda value: (_ for _ in ()).throw(Refused(f"invalid JSON number: {value}")))
    return (value, hashlib.sha256(body).hexdigest()) if with_digest else value


def file_digest(path):
    result = hashlib.sha256()
    size = 0
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(64 * 1024), b""):
            size += len(chunk)
            if size > MAX_BYTES:
                raise Refused("study input exceeds its byte limit")
            result.update(chunk)
    return result.hexdigest()


def outside(path):
    path = Path(path).resolve()
    if path.is_relative_to(REPO):
        raise Refused("review evidence and studies must stay outside the public checkout")
    return path


def create(path, value):
    """Claim once. A retry cannot overwrite a frozen candidate or opened test."""
    path = Path(path)
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError as error:
        raise Refused(f"{path.name} already exists; start a new study") from error
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(canonical(value) + b"\n")
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def state_group(case):
    # Compute this too instead of trusting an imported grouping field. Question,
    # response and workspace changes cannot split the same visible input.
    return digest(case["request"]["state"])


def schema(case):
    return {key: {name: value for name, value in question.items() if name != "instructions"}
            for key, question in case["request"]["questions"].items()}


def pair_key(case):
    return digest([case.get("workspace"), case.get("originGroup"), state_group(case), schema(case)])


def export(path, data=None):
    data = read(path) if data is None else data
    if not isinstance(data, dict) or type(data.get("schemaVersion")) is not int or data["schemaVersion"] != 1 or data.get("invalid", 0) != 0:
        raise Refused("use a complete, valid `zo jev export` snapshot")
    cases, outcomes = data.get("cases"), data.get("outcomes")
    if not isinstance(cases, list) or len(cases) > MAX_CASES or not isinstance(outcomes, list) or len(outcomes) > MAX_CASES * 100 * 3:
        raise Refused("review export has invalid bounds")
    by_id = {}
    for case in cases:
        if (not isinstance(case, dict) or not isinstance(case.get("id"), str)
                or len(case["id"]) != 64 or any(c not in "0123456789abcdef" for c in case["id"])
                or case["id"] in by_id or type(case.get("at")) is not int
                or type(case.get("rubricVersion")) is not int or case["rubricVersion"] < 1
                or not isinstance(case.get("seat"), str) or not case["seat"].strip()
                or not isinstance(case.get("model"), str) or not case["model"].strip()
                or not isinstance(case.get("request"), dict) or "state" not in case["request"]
                or not isinstance(case["request"].get("questions"), dict)
                or not isinstance(case.get("answers"), dict)
                or not 1 <= len(case["answers"]) <= 100
                or set(case["answers"]) != set(case["request"]["questions"])):
            raise Refused("invalid or duplicate case in review export")
        origin = case.get("originGroup")
        if origin is not None and (not isinstance(origin, str) or len(origin) != 64
                or any(char not in "0123456789abcdef" for char in origin)):
            raise Refused("invalid local origin group")
        for identity, question in case["request"]["questions"].items():
            answer = case["answers"][identity]
            if (not identity or not isinstance(question, dict) or not isinstance(answer, dict)
                    or question.get("type") not in {"choice", "noul", "score"}
                    or answer.get("type") != question["type"]
                    or not isinstance(question.get("instructions"), str)):
                raise Refused("case does not contain typed questions and answers")
        by_id[case["id"]] = case
    labels = {}
    for outcome in outcomes:
        case = by_id.get(outcome.get("caseId")) if isinstance(outcome, dict) else None
        if (case is None or outcome.get("question") not in case["answers"]
                or type(outcome.get("correct")) is not bool or type(outcome.get("at")) is not int
                or outcome["at"] < case["at"] or outcome.get("reviewer") not in {"human", "agent", "execution"}
                or not isinstance(outcome.get("note"), str) or not outcome["note"].strip()
                or len(outcome["note"]) > 1000):
            raise Refused("invalid outcome in review export")
        key = (case["id"], outcome["question"], outcome["reviewer"])
        old = labels.get(key)
        if old and old["at"] == outcome["at"] and old != outcome:
            raise Refused("conflicting reviews have the same timestamp")
        if old is None or outcome["at"] > old["at"]:
            labels[key] = outcome
    return by_id, labels


def checked_export(path, expected):
    data, actual = read(path, with_digest=True)
    if actual != expected:
        raise Refused("study input changed while it was read")
    return export(path, data)


def manifest(workdir):
    value = read(Path(workdir) / "manifest.json")
    if "originKnown" not in value or not isinstance(value.get("bundles"), dict):
        raise Refused("study lacks session/task provenance; prepare a new study")
    if file_digest(value["source"]) != value["sourceSha256"]:
        raise Refused("source changed; start a new study")
    return value


def prepare(source, workdir, seat, rubric, model, reviewer="human"):
    source, workdir = outside(source), outside(workdir)
    data, source_hash = read(source, with_digest=True)
    cases, labels = export(source, data)
    latest = {}
    for case in cases.values():
        if (case["seat"], case["rubricVersion"], case["model"]) != (seat, rubric, model):
            continue
        if not any(key[0] == case["id"] and key[2] == reviewer for key in labels):
            continue
        key = pair_key(case)
        if key not in latest or (case["at"], case["id"]) > (latest[key]["at"], latest[key]["id"]):
            latest[key] = case
    rows = [loop.Row(case["id"], case["at"], index, case.get("originGroup") or "unknown-origin", False, {}, state_group(case))
            for index, case in enumerate(sorted(latest.values(), key=lambda one: (one["at"], one["id"])))]
    dev, held = loop.split(rows)
    dev_ids, held_ids = [row.id for row in dev], [row.id for row in held]
    bundles = dict(zip([row.id for row in rows], loop.bundle_ids(rows)))
    groups = lambda ids: len({bundles[identity] for identity in ids})
    known_origins = all(cases[identity].get("originGroup") for identity in dev_ids + held_ids)
    value = {"schemaVersion": 1, "source": str(source), "sourceSha256": source_hash,
             "seat": seat, "rubricVersion": rubric, "model": model, "reviewer": reviewer,
             "development": dev_ids, "heldout": held_ids,
             "developmentGroups": groups(dev_ids), "heldoutGroups": groups(held_ids),
             "originKnown": bool(known_origins), "bundles": bundles,
             "purged": loop.purged(rows), "label": "explicit correctness review of the captured answer",
             "judgeable": bool(known_origins) and groups(dev_ids) >= MIN_DEV_GROUPS and groups(held_ids) >= MIN_HELD_GROUPS}
    workdir.mkdir(mode=0o700, parents=True, exist_ok=False)
    create(workdir / "manifest.json", value)
    create(workdir / "development.json", {"schemaVersion": 1, "invalid": 0,
        "cases": [cases[identity] for identity in dev_ids],
        "outcomes": [label for key, label in labels.items() if key[0] in dev_ids and key[2] == reviewer]})
    return {key: value[key] for key in ("developmentGroups", "heldoutGroups", "purged", "judgeable")}


def candidate(path, study):
    if path is None:
        raise Refused("development requests and evaluation need --candidate")
    value = read(path)
    if (value.get("schemaVersion") != 1 or value.get("seat") != study["seat"]
            or value.get("baseRubricVersion") != study["rubricVersion"]
            or type(value.get("rubricVersion")) is not int or value["rubricVersion"] != study["rubricVersion"] + 1
            or value.get("model") != study["model"]):
        raise Refused("candidate must name this seat, model and next rubric version")
    rewrites = value.get("rewrites")
    if not isinstance(rewrites, list) or not 1 <= len(rewrites) <= 8:
        raise Refused("candidate needs 1 to 8 literal instruction-prefix rewrites")
    for rewrite in rewrites:
        if (not isinstance(rewrite, dict) or set(rewrite) != {"from", "to"}
                or any(not isinstance(rewrite[key], str) or not rewrite[key].strip() or len(rewrite[key]) > 4096 for key in ("from", "to"))
                or rewrite["from"] == rewrite["to"]):
            raise Refused("invalid or unchanged instruction rewrite")
    prefixes = [rewrite["from"] for rewrite in rewrites]
    if any(a.startswith(b) for i, a in enumerate(prefixes) for j, b in enumerate(prefixes) if i != j):
        raise Refused("instruction prefixes overlap")
    return value


def rewritten(case, definition):
    request = copy.deepcopy(case["request"])
    changed = []
    for identity, question in request["questions"].items():
        words = question.get("instructions", "")
        for rewrite in definition["rewrites"]:
            if words.startswith(rewrite["from"]):
                question["instructions"] = rewrite["to"] + words[len(rewrite["from"]):]
                changed.append(identity)
                break
    if not changed:
        raise Refused("candidate does not rewrite a question in this case")
    return request, changed


def freeze(workdir, candidate_path, now=None):
    workdir = outside(workdir)
    study = manifest(workdir)
    if not study["judgeable"]:
        raise Refused("not enough independent development and held-out groups")
    definition = candidate(candidate_path, study)
    cases, _ = checked_export(study["source"], study["sourceSha256"])
    # Only development questions are inspected before the immutable claim.
    for identity in study["development"]:
        rewritten(cases[identity], definition)
    value = {"candidate": definition, "candidateSha256": digest(definition),
             "manifestSha256": file_digest(workdir / "manifest.json"),
             "at": int(time.time() * 1000) if now is None else now}
    create(workdir / "freeze.json", value)
    return {"candidateSha256": value["candidateSha256"], "productionChanged": False}


def frozen(workdir):
    study = manifest(workdir)
    value = read(Path(workdir) / "freeze.json")
    if value["manifestSha256"] != file_digest(Path(workdir) / "manifest.json") or value["candidateSha256"] != digest(value["candidate"]):
        raise Refused("frozen definition or manifest changed")
    return study, value


def requests(workdir, candidate_path=None, heldout=False):
    workdir = outside(workdir)
    if heldout:
        study, fixed = frozen(workdir)
        definition = fixed["candidate"]
        ids = study["heldout"]
    else:
        study = manifest(workdir)
        definition = candidate(candidate_path, study)
        ids = study["development"]
    cases, _ = checked_export(study["source"], study["sourceSha256"])
    return {"schemaVersion": 1, "candidateSha256": digest(definition), "phase": "heldout" if heldout else "development",
        "seat": study["seat"], "rubricVersion": definition["rubricVersion"], "model": study["model"],
        "requests": [{"baseCaseId": identity, "workspace": cases[identity].get("workspace"),
                      "originGroup": cases[identity].get("originGroup"),
                      "request": rewritten(cases[identity], definition)[0]} for identity in ids]}


def evaluate(workdir, candidate_export, *, development=False, candidate_path=None):
    workdir = outside(workdir)
    candidate_export = outside(candidate_export)
    if development:
        study = manifest(workdir)
        definition = candidate(candidate_path, study)
        ids, earliest = study["development"], None
    else:
        study, fixed = frozen(workdir)
        definition = fixed["candidate"]
        ids, earliest = study["heldout"], fixed["at"]
        if not Path(candidate_export).is_file():
            raise Refused("candidate export is absent")
        # Claim before opening either side's labels. Even an incomplete or
        # invalid final export cannot become a sequence of held-out retries.
        claim = {"candidateSha256": digest(definition), "responses": str(candidate_export),
                 "responsesSha256": file_digest(candidate_export)}
        create(workdir / "evaluation.json", claim)
    original, base_labels = checked_export(study["source"], study["sourceSha256"])
    proposed, new_labels = (export(candidate_export) if development
                            else checked_export(candidate_export, claim["responsesSha256"]))
    matches = {}
    for case in proposed.values():
        if (case["seat"], case["rubricVersion"], case["model"]) != (study["seat"], definition["rubricVersion"], study["model"]):
            raise Refused("candidate export mixes seats, rubrics or answering models")
        key = pair_key(case)
        if key in matches:
            raise Refused("candidate contains more than one answer for a paired input")
        matches[key] = case
    groups = {}
    questions = 0
    for identity in ids:
        before = original[identity]
        after = matches.get(pair_key(before))
        expected, changed = rewritten(before, definition)
        if after is None or after["request"] != expected or (earliest is not None and after["at"] < earliest):
            raise Refused("a paired answer is missing, predates the freeze or used different questions")
        pairs = []
        for question in changed:
            base = base_labels.get((identity, question, study["reviewer"]))
            if base is None:
                continue
            new = new_labels.get((after["id"], question, study["reviewer"]))
            if new is None:
                raise Refused("candidate is missing an independent review for a scored question")
            pairs.append((int(base["correct"]), int(new["correct"])))
        if not pairs:
            raise Refused("the rewritten questions have no reviewed baseline")
        questions += len(pairs)
        groups.setdefault(study["bundles"][identity], []).extend(pairs)
    scores = [(sum(a for a, _ in pairs) / len(pairs), sum(b for _, b in pairs) / len(pairs)) for pairs in groups.values()]
    wins = sum(b > a for a, b in scores)
    losses = sum(b < a for a, b in scores)
    lower = loop.wilson_lower(wins, wins + losses)
    result = {"phase": "development" if development else "heldout", "candidateSha256": digest(definition),
        "groups": len(scores), "questions": questions, "wins": wins, "losses": losses,
        "baselineAgreement": sum(a for a, _ in scores) / len(scores) if scores else None,
        "candidateAgreement": sum(b for _, b in scores) / len(scores) if scores else None,
        "pairedWinLowerBound": lower, "productionChanged": False,
        "verdict": "ready_for_code_review" if not development and len(scores) >= MIN_HELD_GROUPS and lower is not None and lower > 0.5 else "not_proven",
        "scope": "reviewed cohort only; latency, cost and runtime application are not inferred"}
    if not development:
        create(workdir / "result.json", result)
        create(workdir / "result-sha256.json", {"sha256": file_digest(workdir / "result.json")})
    return result


def read_result(workdir):
    workdir = outside(workdir)
    _, fixed = frozen(workdir)
    claim = read(workdir / "evaluation.json")
    if claim["candidateSha256"] != fixed["candidateSha256"] or file_digest(claim["responses"]) != claim["responsesSha256"]:
        raise Refused("the evaluated candidate responses changed")
    result, actual = read(workdir / "result.json", with_digest=True)
    if actual != read(workdir / "result-sha256.json")["sha256"] or result["candidateSha256"] != fixed["candidateSha256"]:
        raise Refused("the saved result changed")
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("prepare")
    command.add_argument("source", type=Path)
    command.add_argument("workdir", type=Path)
    command.add_argument("--seat", required=True)
    command.add_argument("--rubric", required=True, type=int)
    command.add_argument("--model", required=True)
    command.add_argument("--reviewer", choices=("human", "execution", "agent"), default="human")
    commands.add_parser("result").add_argument("workdir", type=Path)
    for name in ("requests", "freeze", "evaluate"):
        command = commands.add_parser(name)
        command.add_argument("workdir", type=Path)
        command.add_argument("--candidate", type=Path, required=name == "freeze")
        if name == "requests": command.add_argument("--heldout", action="store_true")
        if name == "evaluate":
            command.add_argument("responses", type=Path)
            command.add_argument("--development", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare": result = prepare(args.source, args.workdir, args.seat, args.rubric, args.model, args.reviewer)
        elif args.command == "freeze": result = freeze(args.workdir, args.candidate)
        elif args.command == "requests": result = requests(args.workdir, args.candidate, args.heldout)
        elif args.command == "result": result = read_result(args.workdir)
        else: result = evaluate(args.workdir, args.responses, development=args.development, candidate_path=args.candidate)
    except (Refused, OSError, TypeError, KeyError, json.JSONDecodeError) as error:
        parser.exit(2, f"review study refused: {error}\n")
    print(json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
