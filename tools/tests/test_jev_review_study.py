#!/usr/bin/env python3
"""Review exports become paired evidence only across a frozen, disjoint split."""
from __future__ import annotations

import copy
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("jev_review_study", REPO / "tools/question-discovery/review.py")
review = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = review
spec.loader.exec_module(review)


def fixture(count=40):
    cases, outcomes = [], []
    for index in range(count):
        identity = f"{index + 1:064x}"
        case = {"id": identity, "group": f"group-{index}", "originGroup": review.digest(["session", index]), "at": index + 1,
                "seat": "compaction", "workspace": "/work/project", "rubricVersion": 2, "model": "jev-test",
                "request": {"model": "jev-latest", "state": {"input": index},
                            "questions": {"q": {"type": "noul", "instructions": "Old rule: read this input."}}},
                "answers": {"q": {"type": "noul", "noul": 0.5}}}
        cases.append(case)
        outcomes.append({"caseId": identity, "question": "q", "at": 1000 + index,
                         "correct": False, "reviewer": "human", "note": "Independent fixture assessment."})
    return {"schemaVersion": 1, "invalid": 0, "cases": cases, "outcomes": outcomes}


class ReviewStudyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source.json"
        self.study = self.root / "study"
        self.candidate = self.root / "candidate.json"
        self.data = fixture()
        self.definition = {"schemaVersion": 1, "seat": "compaction", "baseRubricVersion": 2,
                           "rubricVersion": 3, "model": "jev-test",
                           "rewrites": [{"from": "Old rule:", "to": "New rule:"}]}
        self.write(self.source, self.data)
        self.write(self.candidate, self.definition)

    @staticmethod
    def write(path, value):
        path.write_text(json.dumps(value))

    def prepare(self):
        return review.prepare(self.source, self.study, "compaction", 2, "jev-test")

    def responses(self, *, correct=True, old_time=False):
        data = copy.deepcopy(self.data)
        for case in data["cases"]:
            case["id"] = review.digest(["candidate", case["id"]])
            case["rubricVersion"] = 3
            case["at"] = 50 if old_time else 2000
            case["request"] = review.rewritten(case, self.definition)[0]
        for old, case, outcome in zip(self.data["cases"], data["cases"], data["outcomes"]):
            self.assertNotEqual(old["id"], case["id"])
            outcome.update(caseId=case["id"], correct=correct, at=3000)
        path = self.root / "responses.json"
        self.write(path, data)
        return path

    def test_development_export_and_requests_do_not_reveal_heldout_labels(self):
        self.assertTrue(self.prepare()["judgeable"])
        manifest = review.manifest(self.study)
        dev = review.read(self.study / "development.json")
        self.assertEqual({case["id"] for case in dev["cases"]}, set(manifest["development"]))
        self.assertFalse(set(manifest["heldout"]) & {row["caseId"] for row in dev["outcomes"]})
        plan = review.requests(self.study, self.candidate)
        self.assertNotIn("outcomes", plan)
        self.assertTrue(all("correct" not in row for row in plan["requests"]))
        with self.assertRaises(FileNotFoundError):
            review.requests(self.study, heldout=True)

    def test_one_session_with_many_different_states_is_not_independent_evidence(self):
        for case in self.data["cases"]:
            case["originGroup"] = review.digest(["one-session"])
        self.write(self.source, self.data)
        result = self.prepare()
        self.assertFalse(result["judgeable"])
        self.assertLessEqual(result["developmentGroups"] + result["heldoutGroups"], 1)
        with self.assertRaises(review.Refused):
            review.freeze(self.study, self.candidate, now=1000)

    def test_unknown_origins_remain_reviewable_but_cannot_earn_acceptance(self):
        for case in self.data["cases"]:
            case.pop("originGroup")
        self.write(self.source, self.data)
        self.assertFalse(self.prepare()["judgeable"])
        self.assertFalse(review.manifest(self.study)["originKnown"])

    def test_interleaved_turns_of_one_session_never_cross_the_split(self):
        for index in [0, len(self.data["cases"]) - 1]:
            self.data["cases"][index]["originGroup"] = review.digest(["shared-session"])
        self.write(self.source, self.data)
        self.prepare()
        study = review.manifest(self.study)
        origins = lambda ids: {case["originGroup"] for case in self.data["cases"] if case["id"] in ids}
        self.assertFalse(origins(study["development"]) & origins(study["heldout"]))

    def test_same_cleared_state_in_different_workspaces_cannot_cross_the_split(self):
        self.data["cases"][-1]["request"]["state"] = self.data["cases"][0]["request"]["state"]
        self.data["cases"][-1]["workspace"] = "/work/other"
        self.write(self.source, self.data)
        self.prepare()
        manifest = review.manifest(self.study)
        self.assertGreater(manifest["purged"], 0)
        self.assertNotIn(self.data["cases"][-1]["id"], manifest["heldout"])

    def test_frozen_paired_improvement_is_ready_for_review_and_changes_no_production_setting(self):
        self.prepare()
        review.freeze(self.study, self.candidate, now=1500)
        result = review.evaluate(self.study, self.responses())
        self.assertEqual(result["verdict"], "ready_for_code_review")
        self.assertEqual(result["baselineAgreement"], 0)
        self.assertEqual(result["candidateAgreement"], 1)
        self.assertFalse(result["productionChanged"])
        self.assertEqual(review.read_result(self.study), result)
        with self.assertRaises(review.Refused):
            review.evaluate(self.study, self.root / "responses.json")
        with self.assertRaises(review.Refused):
            review.freeze(self.study, self.candidate)

    def test_a_saved_result_requires_the_same_responses_and_result_bytes(self):
        self.prepare()
        review.freeze(self.study, self.candidate, now=1500)
        responses = self.responses()
        result = review.evaluate(self.study, responses)
        result["candidateAgreement"] = 0.75
        self.write(self.study / "result.json", result)
        with self.assertRaises(review.Refused):
            review.read_result(self.study)
        data = review.read(responses)
        data["outcomes"][0]["correct"] = False
        self.write(responses, data)
        with self.assertRaises(review.Refused):
            review.read_result(self.study)

    def test_an_unchanged_accuracy_is_not_accepted(self):
        self.prepare()
        review.freeze(self.study, self.candidate, now=1500)
        result = review.evaluate(self.study, self.responses(correct=False))
        self.assertEqual(result["verdict"], "not_proven")
        self.assertIsNone(result["pairedWinLowerBound"])

    def test_source_changes_and_candidate_tampering_are_refused(self):
        self.prepare()
        review.freeze(self.study, self.candidate, now=1500)
        fixed = review.read(self.study / "freeze.json")
        fixed["candidate"]["rewrites"][0]["to"] = "Later candidate:"
        self.write(self.study / "freeze.json", fixed)
        with self.assertRaises(review.Refused):
            review.requests(self.study, heldout=True)
        self.data["outcomes"][0]["correct"] = True
        self.write(self.source, self.data)
        with self.assertRaises(review.Refused):
            review.manifest(self.study)

    def test_missing_labels_old_answers_and_mixed_models_cannot_become_a_good_final_score(self):
        for problem in ("label", "time", "model", "duplicate"):
            with self.subTest(problem=problem):
                self.study = self.root / problem
                self.prepare()
                review.freeze(self.study, self.candidate, now=1500)
                path = self.responses(old_time=problem == "time")
                data = review.read(path)
                if problem == "label": data["outcomes"] = []
                if problem == "model": data["cases"][-1]["model"] = "jev-other"
                if problem == "duplicate":
                    extra = copy.deepcopy(data["cases"][-1])
                    extra["id"] = "f" * 64
                    data["cases"].append(extra)
                self.write(path, data)
                with self.assertRaises(review.Refused):
                    review.evaluate(self.study, path)
                self.assertFalse((self.study / "result.json").exists())
                self.assertTrue((self.study / "evaluation.json").exists(), "opened holdout cannot be retried")

    def test_small_cohorts_and_overlapping_rewrites_do_not_freeze(self):
        self.write(self.source, fixture(4))
        self.assertFalse(self.prepare()["judgeable"])
        with self.assertRaises(review.Refused):
            review.freeze(self.study, self.candidate)
        self.definition["rewrites"].append({"from": "Old rule: read", "to": "Ambiguous:"})
        self.write(self.candidate, self.definition)
        with self.assertRaises(review.Refused):
            review.candidate(self.candidate, review.manifest(self.study))


if __name__ == "__main__":
    unittest.main()
