#!/usr/bin/env python3
"""Contract for `tools/question-discovery/` — the loop proposes, fits and
judges without ever reading a held-out label; a revision or a removal is
kept only when the dev error drops; an addition is dropped only for no
spread; every new question of a round rides one asking stage call; the
judgment happens once and freezes; a round asked already is never paid for
again; and the seed counts ledgers without reading a word.

Run: python3 tools/tests/test_question_discovery.py   (stdlib only)
"""

from __future__ import annotations

import importlib.util
import contextlib
import io
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[2]


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


loop = load("question_discovery_loop", REPO / "tools" / "question-discovery" / "loop.py")
seed = load("question_discovery_seed", REPO / "tools" / "question-discovery" / "seed.py")


def row_line(i: int, group: str, label: bool, answers: dict | None = None, outcome: str = "answered", at: int | None = None) -> str:
    return json.dumps({"row": f"notify:{i}", "at": at if at is not None else 1_000 + i, "seq": i, "group": group, "label": label,
                       "shippedPredicts": label if answers else None,
                       "baseline": {"waitingPanes": i % 3, "patchBytes": 10 * i}, "outcome": outcome, "answers": answers or {},
                       "requests": 0 if outcome == "unasked" else 1, "retries": 0,
                       "costUsd": None if outcome == "unasked" else 0.001})


class FakeAsker:
    """The asking stage as a function of the round: the rows are a fixed
    sample; each question's answer is whatever the test's rule says."""

    def __init__(self, rows: list[tuple[str, bool]], rule):
        self.rows = rows
        self.rule = rule
        self.calls: list[dict] = []

    def __call__(self, spec: dict, out: Path, states: Path | None) -> dict:
        self.calls.append(spec)
        wanted = spec["rows"]
        lines = []
        for i, (group, label) in enumerate(self.rows):
            rid = f"notify:{i}"
            if wanted is not None and rid not in wanted:
                continue
            questions = spec["questions"]
            answers = {}
            if questions == "shipped":
                answers = {"call": {"choice": "batch", "probabilities": {"interrupt": 0.2, "batch": 0.6, "ignore": 0.2}, "confidence": 0.4}}
            elif isinstance(questions, dict):
                answers = {qid: self.rule(qid, i, label) for qid in questions}
            lines.append(row_line(i, group, label, answers if questions != {} else None,
                                  outcome="unasked" if questions == {} else "answered"))
        summary = {"summary": {"labeled": len(self.rows), "unlabeled": 0, "duplicateIds": 0,
                               "asked": 0 if spec["questions"] == {} else len(lines), "inputTokens": 100 * len(lines),
                               "costUsd": 0.001 * len(lines), "transcriptsRead": 1, "transcriptsSkipped": 0}}
        out.write_text("\n".join(lines + [json.dumps(summary)]) + "\n", encoding="utf-8")
        if states is not None:
            states.write_text("\n".join(json.dumps({"row": f"notify:{i}", "state": {"words": f"state of row {i}"}}) for i, _ in enumerate(self.rows)) + "\n", encoding="utf-8")
        return {}


def sample(n: int = 40) -> list[tuple[str, bool]]:
    """Twenty bundles of two rows; the label alternates inside a bundle
    pattern so both classes sit on both sides of any cut."""
    return [(f"g{i // 2}", (i * 7) % 3 == 0) for i in range(n)]


# The two ways the asking stage leaves a row's bill unsettled
# (`question_discovery.rs`, `ask`): a response lost after the wire, which
# the provider may have billed, and server usage past the row's reservation.
UNSETTLED = {
    "lostResponse": {"outcome": "timeout", "answers": {}, "requests": 1, "costUsd": None, "costUnknown": True},
    "overReservation": {"requests": 1, "costUsd": 0.5, "budgetExceeded": True},
}


def unsettle_last_row(asker: FakeAsker, change: dict):
    """The asking stage whose paid round leaves its LAST row unsettled — so
    no later row is capped, and the round comes back whole."""

    def ask(spec: dict, out: Path, states: Path | None) -> dict:
        asker(spec, out, states)
        if spec["questions"] == {}:
            return {}
        lines = [json.loads(line) for line in out.read_text().splitlines()]
        [line for line in lines if "summary" not in line][-1].update(change)
        out.write_text("\n".join(json.dumps(line) for line in lines) + "\n")
        return {}

    return ask


def informative(qid: str, i: int, label: bool):
    """A question whose answer leans with the label; `noise_*` questions
    lean with nothing; `flat` says the same of every row."""
    if qid.startswith("noise"):
        return 0.3 + 0.4 * ((i * 13) % 5) / 4.0
    if qid == "flat":
        return 0.5
    return 0.8 if label else 0.2


class HeldOutSealing(unittest.TestCase):
    def search(self, tmp: Path, rows, rule, proposals: list[str], rounds: int = 2) -> loop.Search:
        asker = FakeAsker(rows, rule)
        search = loop.Search("notify", "seed.json", tmp, asker, loop.file_proposer([]), None, 300, rounds)
        held = list(proposals)
        search.proposer = lambda _prompt: held.pop(0) if held else "{}"
        search.asker_calls = asker.calls
        return search

    def test_the_loop_never_reads_held_out_labels(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            proposal = json.dumps({"add": {"q_lean": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}})
            search = self.search(tmp, sample(), informative, [proposal, proposal])
            reads = []
            original = loop.HeldOut.labels

            def watched(self_):
                reads.append(self_.unsealed)
                return original(self_)

            loop.HeldOut.labels = watched
            try:
                search.enumerate()
                search.round_zero()
                self.assertEqual(search.held.unsealed, 0)
                search.step(1, loop.Proposal.parse(proposal))
                search.prompt(loop.read_states(tmp / "states.jsonl"))
                self.assertEqual(search.held.unsealed, 0, "two rounds and a prompt read no held-out label")
                self.assertEqual(reads, [], "nothing even asked for them")
                with self.assertRaises(loop.SealedLabel):
                    search.held.labels()
                result = search.judge()
                self.assertEqual(search.held.unsealed, 1, "the judgment unseals once")
                self.assertEqual(result["verdict"], "judged")
                with self.assertRaises(loop.AlreadyJudged):
                    search.judge()
                frozen = json.loads((tmp / "freeze.json").read_text())
                self.assertTrue(frozen["finalEvaluated"])
                self.assertEqual(sorted(frozen["questions"]), sorted(result["questions"]))
            finally:
                loop.HeldOut.labels = original

    def test_the_proposer_is_shown_dev_rows_only(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            search = self.search(tmp, sample(), informative, [])
            search.enumerate()
            search.round_zero()
            prompt = search.prompt(loop.read_states(tmp / "states.jsonl"))
            held_ids = {row.id for row in search.held.rows}
            shown = re.findall(r"state of row (\d+)", prompt)
            self.assertTrue(shown, "the prompt shows rows")
            self.assertTrue(all(f"notify:{i}" not in held_ids for i in shown), "no held-out row reaches the proposer")
            self.assertNotIn("label\": true, \"modelP\": 0.0", prompt)

    def test_a_judged_directory_refuses_a_second_search(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            search = self.search(tmp, sample(), informative, [], rounds=0)
            search.run()
            again = self.search(tmp, sample(), informative, [], rounds=0)
            with self.assertRaises(loop.AlreadyJudged):
                again.run()

    def test_a_new_object_cannot_judge_over_an_existing_freeze(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            first = self.search(tmp, sample(), informative, [], rounds=0)
            first.run()
            frozen = (tmp / "freeze.json").read_bytes()
            result = (tmp / "result.json").read_bytes()
            self.assertEqual(loop.read_result(tmp)["evaluationId"], json.loads(result)["evaluationId"])
            again = self.search(tmp, sample(), informative, [], rounds=0)
            again.dev = first.dev
            again.held = loop.HeldOut([loop.Row(id=row.id, at=row.at, seq=row.seq, group=row.group,
                                                label=label, baseline=row.baseline, duplicate=row.duplicate,
                                                shipped_predicts=row.shipped_predicts, features=row.features)
                                       for row, label in zip(first.held.rows, first.held.labels())])
            again.questions = first.questions
            again.shipped_ids = first.shipped_ids
            with self.assertRaises(loop.AlreadyJudged):
                again.judge()
            self.assertEqual((tmp / "freeze.json").read_bytes(), frozen)
            self.assertEqual((tmp / "result.json").read_bytes(), result)

    def test_completed_result_replay_rejects_changed_result(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            self.search(tmp, sample(), informative, [], rounds=0).run()
            result = tmp / "result.json"
            result.write_text(result.read_text() + " ")
            with self.assertRaisesRegex(RuntimeError, "result changed"):
                loop.read_result(tmp)


class RoundRules(unittest.TestCase):
    def test_a_patch_cohort_with_inverted_session_and_patch_order_is_not_paid(self):
        with tempfile.TemporaryDirectory() as raw:
            # A begins at 09:00 but patches at 12:00; B begins at 10:00
            # and patches at 10:05. Session creation order is reversed.
            a = loop.Row("patch_review:a", 9 * 3600, 0, "a", True, {})
            b = loop.Row("patch_review:b", 10 * 3600, 0, "b", False, {})
            self.assertEqual([row.id for row in loop.ordered([a, b])], [a.id, b.id])
            actual_patch_at = {a.id: 12 * 3600, b.id: 10 * 3600 + 5 * 60}
            self.assertEqual(sorted(actual_patch_at, key=actual_patch_at.get), [b.id, a.id])
            asker = FakeAsker(sample(), informative)
            search = loop.Search("patch_review", "seed.json", Path(raw), asker, None, None, 300, 0)
            result = search.run()
            self.assertEqual(result["verdict"], loop.NOT_EVALUABLE)
            self.assertFalse(result["manifest"]["split"]["byTime"])
            self.assertEqual(len(asker.calls), 1)

    def test_a_patch_cohort_is_never_judged_even_by_a_direct_judge(self):
        """The patch cohort's hold is the eligibility `run` and `judge` share,
        not a check on `run`'s road alone: a caller who enumerates and calls
        `judge` — with no question, or with features restored the way the
        external validation restores them — opens no label and writes no
        judgment, though each class has rows enough that a count would."""
        for restored in (False, True):
            with self.subTest(restored=restored), tempfile.TemporaryDirectory() as raw:
                tmp = Path(raw)
                asker = FakeAsker(sample(), informative)
                search = loop.Search("patch_review", "seed.json", tmp, asker, None, None, 300, 0)
                manifest = search.enumerate()
                counts = manifest["counts"]
                self.assertGreaterEqual(min(counts["devPositives"], counts["devNegatives"], counts["heldPositives"], counts["heldNegatives"]),
                                        loop.JUDGEABLE_PER_CLASS, "each class has rows enough that a count alone would judge")
                self.assertFalse(manifest["judgeable"])
                if restored:
                    for row in search.dev + search.held.rows:
                        row.features = {"restored": (row.seq % 5) / 4.0}
                    search.questions = {"restored": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}
                with self.assertRaises(RuntimeError) as refused:
                    search.judge()
                self.assertEqual(search.held.unsealed, 0, "no held-out label was read")
                self.assertFalse((tmp / "freeze.json").exists(), "nothing was frozen")
                self.assertFalse((tmp / "result.json").exists(), "no judgment was written")
                self.assertFalse(json.loads((tmp / "manifest.json").read_text())["finalEvaluated"])
                self.assertEqual([call["questions"] for call in asker.calls], [{}], "and nothing was asked")
                self.assertIsInstance(refused.exception, loop.NotEvaluable)

    def test_a_patch_cohort_is_never_asked_a_paid_question_by_any_road(self):
        """A caller who enumerates a patch cohort and asks round 0 or a round
        of its own directly is refused before a row is reserved: nothing
        reaches the asking stage, and no reservation is left standing as an
        uncertain bill for a question that was never sent."""
        for road in ("round_zero", "step"):
            with self.subTest(road=road), tempfile.TemporaryDirectory() as raw:
                tmp = Path(raw)
                asker = FakeAsker(sample(), informative)
                search = loop.Search("patch_review", "seed.json", tmp, asker, None, None, 300, 0)
                search.enumerate()
                with self.assertRaises(RuntimeError) as refused:
                    if road == "round_zero":
                        search.round_zero()
                    else:
                        search.step(1, loop.Proposal(add={"q": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}))
                self.assertEqual([call["questions"] for call in asker.calls], [{}], "only the enumeration reached the asking stage")
                reserved = [line for cache in tmp.glob("cache-*.jsonl") for line in cache.read_text().splitlines() if '"reserved"' in line]
                self.assertEqual(reserved, [], "nothing was reserved for a question that is never sent")
                self.assertIsInstance(refused.exception, loop.NotEvaluable)
                self.assertIsNone(search._unsettled(), "and the study's bill stays settled")

    def test_a_direct_judge_refuses_a_held_set_sharing_a_bundle_with_development(self):
        """`split` purges from the held-out set every bundle the development
        set holds. A caller's own sets meet the same rule before a label is
        read: a pane learned from and judged on is no judgment."""
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            loop.Search("notify", "seed.json", tmp / "plan", asker, None, None, 300, 0).enumerate()
            dev, held = loop.split(loop.read_rows(tmp / "plan" / "rows-enumerate.jsonl")[0])
            twin = dev[0]
            leaked = loop.Row(id="notify:leaked", at=held[-1].at + 1, seq=held[-1].seq + 1, group=twin.group, label=not twin.label, baseline={})
            direct = loop.Search("notify", "seed.json", tmp / "direct", asker, None, None, 300, 0)
            direct.dev, direct.held = dev, loop.HeldOut(held + [leaked])
            with self.assertRaises(RuntimeError) as refused:
                direct.judge()
            self.assertEqual(direct.held.unsealed, 0, "no held-out label was read")
            self.assertFalse((tmp / "direct" / "freeze.json").exists())
            self.assertIsInstance(refused.exception, loop.NotEvaluable)
            self.assertIn("bundle", str(refused.exception))
            clean = loop.Search("notify", "seed.json", tmp / "clean", asker, None, None, 300, 0)
            clean.dev, clean.held = dev, loop.HeldOut(held)
            self.assertEqual(clean.judge()["verdict"], "judged", "the split's own sets pass the same rule")

    def test_a_direct_judge_stands_on_the_runs_own_eligibility(self):
        """Five bundles of eight: the development set holds four — fewer than
        `FOLDS` — while each held-out class has three rows. `run` stops at
        the manifest; `judge`, called directly, stops at the same judgment
        before a label is read, instead of re-deciding on class counts."""
        rows = [(f"g{i // 8}", (i * 7) % 3 == 0) for i in range(40)]
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(rows, informative)
            direct = loop.Search("notify", "seed.json", tmp / "direct", asker, None, None, 300, 0)
            counts = direct.enumerate()["counts"]
            self.assertGreaterEqual(min(counts["heldPositives"], counts["heldNegatives"]), loop.JUDGEABLE_PER_CLASS)
            with self.assertRaises(RuntimeError) as refused:
                direct.judge()
            self.assertEqual(direct.held.unsealed, 0)
            self.assertFalse((tmp / "direct" / "freeze.json").exists())
            self.assertFalse((tmp / "direct" / "result.json").exists())
            ran = loop.Search("notify", "seed.json", tmp / "run", asker, None, None, 300, 0).run()
            self.assertEqual(ran["verdict"], loop.NOT_EVALUABLE)
            self.assertIsInstance(refused.exception, loop.NotEvaluable)
            self.assertEqual(str(refused.exception).split(";")[0], ran["reason"], "one judgment, one reason, on both roads")
            self.assertRegex(ran["reason"], rf"\b{loop.FOLDS}\b.*bundles", "the reason names the rule that failed")

    def test_a_changed_patch_transcript_refuses_before_paid_ask(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            transcript = tmp / "session.jsonl"
            transcript.write_text('{"type":"message","value":1}\n')
            source = tmp / "seed.json"
            source.write_text(json.dumps({"transcripts": [{"path": str(transcript)}]}))
            asker = FakeAsker(sample(), informative)
            search = loop.Search("patch_review", str(source), tmp / "study", asker, None, None, 300, 0)
            search.enumerate()
            transcript.write_text('{"type":"message","value":2}\n')
            with self.assertRaisesRegex(RuntimeError, "source input changed"):
                search.round_zero()
            self.assertEqual(len(asker.calls), 1)

    def test_wire_attempts_and_unknown_cost_survive_reservation_replacement(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 1, 0)
            cache = tmp / "cache-test.jsonl"
            cache.write_text(json.dumps({"row": "notify:0", "reserved": True}) + "\n" +
                             json.dumps({"row": "notify:0", "outcome": "timeout", "requests": 1,
                                         "retries": 0, "costUsd": None, "costUnknown": True}) + "\n")
            self.assertEqual(search._reservations(), (1, 0.0, 1))
            cache.write_text(json.dumps({"row": "notify:0", "outcome": "answered", "requests": 1,
                                         "costUsd": 0.001, "budgetExceeded": True}) + "\n")
            self.assertEqual(search._reservations(), (1, 0.001, 1))
            cache.write_text(json.dumps({"row": "notify:0", "outcome": "answered", "costUsd": 0.001}) + "\n")
            self.assertEqual(search._reservations(), (1, 0.001, 1), "legacy rows lack a wire receipt")

    def test_a_partly_capped_batch_is_not_judged_or_resent_on_restart(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 40, 0)
            search.enumerate()

            def partly_capped(spec, out, states):
                asker(spec, out, states)
                lines = [json.loads(line) for line in out.read_text().splitlines()]
                lines[1].update(outcome="capped", requests=None, costUsd=None)
                out.write_text("\n".join(json.dumps(line) for line in lines) + "\n")
                return {}

            search.asker = partly_capped
            with self.assertRaisesRegex(RuntimeError, "reserved request has no saved response"):
                search.round_zero()
            paid = len(asker.calls)
            again = loop.Search("notify", "seed.json", tmp, asker, None, None, 40, 0)
            again.enumerate()
            with self.assertRaisesRegex(RuntimeError, "reserved request has no saved response"):
                again.round_zero()
            self.assertEqual(len(asker.calls), paid)

    def test_a_revise_is_kept_only_when_dev_error_drops(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 3)
            search.enumerate()
            search.round_zero()
            search.step(1, loop.Proposal(add={"q_lean": {"type": "noul", "instructions": "a", "yes": "y", "no": "n"}}))
            self.assertIn("q_lean", search.questions)
            before = search.history[-1]["devLogloss"]
            worse = search.step(2, loop.Proposal(revise={"q_lean": {"type": "noul", "instructions": "noise_b", "yes": "y", "no": "n"}}))
            self.assertEqual(worse["revised"], [], "a revision that raises the dev error is not kept")
            self.assertIn("q_lean", search.questions)
            self.assertNotIn("q_lean__v2", search.questions)
            self.assertIn("q_lean", worse["dropped"])
            self.assertEqual(worse["devLogloss"], before)
            # a revision whose answers are better than the old ones is kept
            asker.rule = lambda qid, i, label: (0.95 if label else 0.05) if qid == "q_lean__v3" else informative(qid, i, label)
            better = search.step(3, loop.Proposal(revise={"q_lean": {"type": "noul", "instructions": "sharper", "yes": "y", "no": "n"}}))
            self.assertEqual(better["revised"], ["q_lean"])
            self.assertIn("q_lean__v3", search.questions)
            self.assertNotIn("q_lean", search.questions)
            self.assertLess(better["devLogloss"], before)

    def test_a_removal_is_kept_only_when_dev_error_drops(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 3)
            search.enumerate()
            search.round_zero()
            search.step(1, loop.Proposal(add={"q_lean": {"type": "noul", "instructions": "a", "yes": "y", "no": "n"},
                                              "noise_a": {"type": "noul", "instructions": "b", "yes": "y", "no": "n"}}))
            record = search.step(2, loop.Proposal(remove=["q_lean", "noise_a"]))
            self.assertNotIn("q_lean", record["removed"], "removing the question that carries the signal raises the error")
            self.assertIn("q_lean", search.questions)

    def test_an_addition_is_dropped_only_for_no_spread(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 3)
            search.enumerate()
            search.round_zero()
            record = search.step(1, loop.Proposal(add={"flat": {"type": "noul", "instructions": "same", "yes": "y", "no": "n"},
                                                       "noise_a": {"type": "noul", "instructions": "noise", "yes": "y", "no": "n"}}))
            self.assertIn("flat", record["dropped"])
            self.assertTrue(record["dropped"]["flat"].startswith("spread"))
            self.assertIn("noise_a", record["added"], "an addition with spread is kept even when it carries no signal — only a revision or a removal is held to the dev error")

    def test_one_request_carries_every_question_of_the_round(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 3)
            search.enumerate()
            search.round_zero()
            calls_before = len(asker.calls)
            search.step(1, loop.Proposal(add={f"q_{k}": {"type": "noul", "instructions": str(k), "yes": "y", "no": "n"} for k in range(5)},
                                         revise={"call": {"type": "noul", "instructions": "r", "yes": "y", "no": "n"}}))
            self.assertEqual(len(asker.calls) - calls_before, 1, "one asking stage call per round")
            asked = asker.calls[-1]["questions"]
            self.assertEqual(sorted(asked), sorted([f"q_{k}" for k in range(5)] + ["call__v1"]), "every new and revised question of the round in that one call")
            self.assertEqual(asker.calls[-1]["rows"], search.sample_ids(), "asked of the manifest's sample, dev and held alike")

    def test_a_proposal_is_bounded_to_eighteen_new_questions(self):
        proposal = loop.Proposal(add={f"q{k}": {} for k in range(20)}, revise={"r": {}})
        bounded = proposal.bounded()
        self.assertEqual(len(bounded.add) + len(bounded.revise), loop.PROPOSALS_PER_ROUND)

    def test_an_add_cannot_replace_the_shipped_baseline_or_send_bad_words(self):
        with tempfile.TemporaryDirectory() as raw:
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", Path(raw), asker, None, None, 300, 1)
            search.enumerate()
            search.round_zero()
            before = len(asker.calls)
            record = search.step(1, loop.Proposal(add={
                "call": {"type": "noul", "instructions": "replace baseline", "yes": "y", "no": "n"},
                "bad.id": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"},
                "empty": {"type": "noul", "instructions": "", "yes": "y", "no": "n"},
            }))
            self.assertEqual(len(asker.calls), before, "invalid words do not reach the wire")
            self.assertEqual(set(record["dropped"]), {"call", "bad.id", "empty"})
            self.assertIn("call", search.questions, "the shipped baseline remains intact")

    def test_a_round_that_does_not_improve_ends_the_loop(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            flat = json.dumps({"add": {"flat": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}})
            lean = json.dumps({"add": {"q_lean": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}})
            search = loop.Search("notify", "seed.json", tmp, asker, loop.file_proposer([]), None, 300, 4)
            held = [flat, lean, lean]
            search.proposer = lambda _prompt: held.pop(0)
            result = search.run()
            rounds = [r["round"] for r in result["rounds"]]
            self.assertEqual(rounds, [0, 1], "the first round's only question had no spread and was dropped, so nothing improved: the loop stopped and never used the second proposal")
            self.assertFalse(result["rounds"][-1]["improved"])
            self.assertEqual(len(held), 2)

    def test_a_worse_round_does_not_replace_the_frozen_candidate(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 1)
            search.enumerate()
            search.round_zero()
            strong = search.step(1, loop.Proposal(add={"q_lean": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}))
            self.assertTrue(strong["improved"])
            prior = dict(search.questions)
            record = search.step(2, loop.Proposal(add={"noise_extra": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}))
            self.assertFalse(record["improved"])
            self.assertEqual(search.questions, prior)
            result = search.judge()
            self.assertEqual(result["questions"], prior)

    def test_the_last_json_object_of_a_reply_is_the_proposal(self):
        text = 'Here is my thinking {not json}.\n```json\n{"add": {"q": {"type": "noul", "instructions": "i", "yes": "y", "no": "n"}}, "remove": ["old"]}\n```'
        proposal = loop.Proposal.parse(text)
        self.assertEqual(list(proposal.add), ["q"])
        self.assertEqual(proposal.remove, ["old"])
        self.assertEqual(loop.Proposal.parse("no json here").add, {})


class Billing(unittest.TestCase):
    def test_an_unsettled_last_row_stops_every_purchase_and_the_judgment_whatever_the_dollar_line(self):
        """The last row of a paid round leaves its bill unsettled, so the
        round comes back whole with nothing capped. With the default dollar
        line and with a stated one, with a proposal round ahead and with the
        judgment next: no other row is sent, no proposal is bought, no
        held-out label is read, nothing is judged — and a restart on the
        same directory resends nothing and stops at the same place."""
        lean = json.dumps({"add": {"q_lean": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}})
        for fault, change in UNSETTLED.items():
            for cap in (None, 2.0):
                for rounds in (0, 1):
                    with self.subTest(fault=fault, cap=cap, rounds=rounds), tempfile.TemporaryDirectory() as raw:
                        tmp = Path(raw)
                        asker = FakeAsker(sample(), informative)
                        bought: list[str] = []

                        def propose(prompt: str) -> str:
                            bought.append(prompt)
                            return lean

                        def study() -> loop.Search:
                            return loop.Search("notify", "seed.json", tmp, unsettle_last_row(asker, change), propose, None, 300, rounds, spend_cap_usd=cap)

                        first = study()
                        with self.assertRaisesRegex(RuntimeError, "uncertain"):
                            first.run()
                        self.assertEqual([call["questions"] for call in asker.calls], [{}, loop.SHIPPED], "round 0 was the last request sent")
                        self.assertEqual(bought, [], "no proposal was bought after the unsettled row")
                        self.assertEqual(first.held.unsealed, 0, "no held-out label was read")
                        self.assertFalse((tmp / "freeze.json").exists(), "nothing was frozen")
                        self.assertFalse((tmp / "result.json").exists(), "nothing was judged")
                        again = study()
                        with self.assertRaisesRegex(RuntimeError, "uncertain"):
                            again.run()
                        with self.assertRaisesRegex(RuntimeError, "uncertain") as refused:
                            again.judge()
                        self.assertEqual(len(asker.calls), 2, "a restart resends nothing")
                        self.assertEqual(bought, [])
                        self.assertEqual(again.held.unsealed, 0)
                        self.assertFalse((tmp / "freeze.json").exists())
                        self.assertFalse(json.loads((tmp / "manifest.json").read_text())["finalEvaluated"])
                        self.assertIsInstance(refused.exception, loop.UnsettledBill)

    def test_a_proposal_bought_with_no_saved_reply_stops_the_next_purchase_and_the_judgment(self):
        """The proposer's bill is a bill too: a proposal claimed and never
        answered stops every later purchase, and the judgment before a label
        is read; a claim whose reply was saved beside it is settled."""
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)

            def dies(_prompt: str) -> str:
                raise RuntimeError("the proposer exited without a reply")

            with self.assertRaisesRegex(RuntimeError, "without a reply"):
                loop.Search("notify", "seed.json", tmp, asker, dies, None, 300, 1).run()
            self.assertTrue((tmp / "proposal-1.pending").is_file())
            again = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 1)
            again.enumerate()
            again.round_zero()
            paid = len(asker.calls)
            with self.assertRaisesRegex(RuntimeError, "uncertain") as refused:
                again.judge()
            self.assertEqual(again.held.unsealed, 0, "no held-out label was read")
            self.assertFalse((tmp / "freeze.json").exists())
            with self.assertRaisesRegex(RuntimeError, "uncertain"):
                again.step(2, loop.Proposal(add={"q": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}))
            self.assertEqual(len(asker.calls), paid, "no row is sent behind an unsettled proposal")
            self.assertIsInstance(refused.exception, loop.UnsettledBill)
            (tmp / "proposal-1.txt").write_text("{}")
            self.assertEqual(again.judge()["verdict"], "judged", "a claim with its reply saved is settled")

    def test_an_unstated_dollar_line_is_the_briefs_and_the_manifest_says_so(self):
        """No stated dollar line is the brief's line, not none: the manifest
        writes the number in effect, a settled bill that reached it stops
        the next paid row, and the asking stage is handed what is left."""
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 0)
            self.assertEqual(search.enumerate()["caps"]["spendUsd"], loop.SPEND_CAP_USD)
            spent = tmp / "cache-spent.jsonl"
            spent.write_text(json.dumps({"row": "notify:0", "outcome": "answered", "requests": 1, "retries": 0, "costUsd": 1.0}) + "\n")
            search.round_zero()
            self.assertAlmostEqual(asker.calls[-1]["remainingSpendUsd"], loop.SPEND_CAP_USD - 1.0)
            spent.write_text(json.dumps({"row": "notify:0", "outcome": "answered", "requests": 1, "retries": 0, "costUsd": loop.SPEND_CAP_USD}) + "\n")
            paid = len(asker.calls)
            with self.assertRaisesRegex(RuntimeError, "spend"):
                search.step(1, loop.Proposal(add={"q": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}))
            self.assertEqual(len(asker.calls), paid, "the row past the line never reaches the asker")

    def test_the_asking_stage_is_handed_no_money_nobody_budgeted(self):
        """A round handed to the Rust stage without the loop's remaining line
        spends nothing: the stage's own ceiling is never the default."""
        seen: list[dict] = []

        def recorded(argv, **kwargs):
            seen.append(kwargs["env"])
            return subprocess.CompletedProcess(argv, 0)

        with tempfile.TemporaryDirectory() as raw, mock.patch.object(loop.subprocess, "run", recorded):
            tmp = Path(raw)
            ask = loop.cargo_asker(5, tmp / "asker.log")
            ask({"seat": "notify", "source": "seed.json", "sample": None, "rows": None, "questions": {}}, tmp / "rows.jsonl", None)
            ask({"seat": "notify", "source": "seed.json", "sample": None, "rows": ["notify:1"], "questions": loop.SHIPPED, "remainingSpendUsd": 1.25},
                tmp / "rows.jsonl", None)
        self.assertEqual([float(env[loop.ASKER_ENV_SPEND]) for env in seen], [0.0, 1.25])

    def test_the_run_lines_are_the_asking_stages_own(self):
        """The loop's two run lines and the Rust stage's ceilings are one pair
        of numbers, written twice only because two languages read them."""
        rust = (REPO / "zo-ide/crates/tools/src/misc_tools/smart_router/question_discovery.rs").read_text()
        self.assertEqual(float(re.search(r"const SPEND_CAP_USD: f64 = ([0-9.]+);", rust).group(1)), loop.SPEND_CAP_USD)
        self.assertEqual(int(re.search(r"const REQUEST_CAP: usize = ([0-9_]+);", rust).group(1).replace("_", "")), loop.REQUEST_CAP)


class SplitsAndFolds(unittest.TestCase):
    def rows(self, groups: list[str]) -> list:
        return [loop.Row(id=f"r{i}", at=i, seq=0, group=g, label=i % 2 == 0, baseline={}) for i, g in enumerate(groups)]

    def test_the_split_is_by_time_and_never_cuts_a_bundle(self):
        rows = self.rows(["a", "a", "b", "b", "b", "b", "c", "c", "d", "d"])
        dev, held = loop.split(rows, 0.5)
        self.assertEqual([r.group for r in dev], ["a", "a", "b", "b", "b", "b"], "the cut moved forward past bundle b")
        self.assertEqual([r.group for r in held], ["c", "c", "d", "d"])
        self.assertTrue(all(r.at < held[0].at for r in dev), "dev is older than held")

    def test_a_bundle_seen_in_dev_never_sits_in_held(self):
        # a pane rings on both days: its later rings must not be judged after its earlier ones were learned from
        rows = self.rows(["a", "b", "a", "c", "b", "d", "e", "a", "f", "g"])
        dev, held = loop.split(rows, 0.6)
        seen = {r.group for r in dev}
        self.assertTrue(all(r.group not in seen for r in held), [r.group for r in held])
        self.assertEqual([r.group for r in held], ["e", "f", "g"], "the later a is purged, not moved")
        self.assertEqual(loop.purged(rows, 0.6), 1, "one later ring of a; b recurred inside the development set")

    def test_identical_state_in_another_pane_is_purged_from_holdout_and_fold(self):
        rows = [loop.Row(id=f"r{i}", at=i, seq=i, group=f"pane{i}", label=i % 2 == 0, baseline={},
                         duplicate="same-state" if i in (1, 8) else f"state{i}") for i in range(10)]
        dev, held = loop.split(rows, 0.7)
        self.assertIn("r1", [row.id for row in dev])
        self.assertNotIn("r8", [row.id for row in held], "a retry with the same content cannot become holdout")
        self.assertGreater(loop.purged(rows, 0.7), 0)
        self.assertEqual(loop.folds_by_bundle([rows[1], rows[8]], 2), [0, 0])

    def test_folds_keep_a_bundle_on_one_side(self):
        rows = self.rows(["a", "a", "b", "b", "c", "c", "d", "e", "f", "f"])
        folds = loop.folds_by_bundle(rows, 3)
        by_group = {}
        for row, fold in zip(rows, folds):
            by_group.setdefault(row.group, set()).add(fold)
        self.assertTrue(all(len(f) == 1 for f in by_group.values()), by_group)
        self.assertEqual(sorted(set(folds)), [0, 1, 2])

    def test_standardization_is_fitted_on_the_training_rows_alone(self):
        train = self.rows(["a", "b", "c", "d"])
        for i, row in enumerate(train):
            row.features = {"q": float(i)}
        model = loop.fit(train, [r.label for r in train], ["q"])
        self.assertAlmostEqual(model.means[0], 1.5)
        far = loop.Row(id="x", at=9, seq=0, group="z", label=True, baseline={}, features={"q": 1_000.0})
        self.assertTrue(0.0 <= model.predict(far.features) <= 1.0)


class Judgment(unittest.TestCase):
    def test_the_cli_refuses_private_artifacts_inside_the_public_tree(self):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as stopped:
                loop.main(["run", "--seat", "notify", "--source", str(REPO / "justfile"),
                           "--workdir", str(REPO / "tools/question-discovery/results"), "--dry-run"])
        self.assertEqual(stopped.exception.code, 2)

    def test_too_few_of_a_class_is_not_evaluable_never_a_number(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            rows = [(f"g{i}", i < 18) for i in range(20)]  # the newest two rows are the only negatives: under the judgeable line
            asker = FakeAsker(rows, informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 0)
            result = search.run()
            self.assertEqual(result["verdict"], loop.NOT_EVALUABLE)
            self.assertEqual(result["requests"], 0, "an unevaluable plan is stopped before a paid round")
            self.assertEqual([call["questions"] for call in asker.calls], [{}])
            manifest = json.loads((tmp / "manifest.json").read_text())
            self.assertFalse(manifest["judgeable"])

    def test_every_baseline_is_read_on_the_same_held_rows_with_a_paired_difference(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 0)
            result = search.run()
            self.assertEqual(result["verdict"], "judged")
            for name in ("alwaysSame", "shippedQuestions", "seatsOwnDecision", "todaysRule", "waitingPanes"):
                self.assertIn(name, result["baselines"], name)
                self.assertIn("pairedDelta", result["baselines"][name])
                self.assertEqual(len(result["baselines"][name]["pairedDelta"]["auc95"]), 2)
            self.assertEqual(result["baselines"]["todaysRule"]["agreement"]["compared"], result["held"])
            self.assertEqual(result["baselines"]["seatsOwnDecision"]["agreement"]["share"], 1.0, "the fixture's seat decision is the label itself")
            self.assertEqual(result["baselines"]["alwaysSame"]["auc"], 0.5)

    def test_the_manifest_stands_before_anything_is_paid_for_and_a_dry_run_pays_nothing(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, 24, 300, 2, dry_run=True)
            result = search.run()
            self.assertEqual(result["verdict"], "dry_run")
            self.assertEqual([call["questions"] for call in asker.calls], [{}], "one enumeration, no question")
            manifest = json.loads((tmp / "manifest.json").read_text())
            self.assertEqual(manifest["counts"]["sampled"], 40)
            self.assertEqual(manifest["counts"]["unlabeledInSource"], 0)
            self.assertEqual(manifest["counts"]["dev"] + manifest["counts"]["held"], 40)
            self.assertEqual(manifest["caps"], {"rounds": 2, "requestsTotal": 300, "spendUsd": loop.SPEND_CAP_USD, "proposalsPerRound": loop.PROPOSALS_PER_ROUND},
                             "no stated dollar line is the brief's, and the manifest writes the number in effect")
            self.assertIn("writer", manifest["label"])
            self.assertFalse(manifest["finalEvaluated"])
            self.assertFalse((tmp / "rows-0.jsonl").exists())

    def test_a_round_asked_already_is_read_back_and_never_paid_for_again(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            first = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 0)
            first.enumerate()
            first.round_zero()
            paid = len(asker.calls)
            again = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 0)
            again.enumerate()
            again.round_zero()
            self.assertEqual(len(asker.calls), paid, "the same rows and questions come from the cache")
            self.assertEqual(again.summaries[-1]["cached"], 40)
            self.assertEqual(again.summaries[-1]["identity"], first.summaries[-1]["identity"])
            # a later round naming a subset of those rows — a purged split, a resume — pays for none of them
            subset = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 0)
            subset.enumerate()
            subset.dev, subset.held = subset.dev[:10], loop.HeldOut(subset.held.rows[:3])
            subset.round_zero()
            self.assertEqual(len(asker.calls), paid, "thirteen rows answered before are thirteen rows read back")
            self.assertEqual((subset.summaries[-1]["cached"], subset.summaries[-1]["asked"]), (13, 0))
            self.assertEqual(len(loop.read_rows(tmp / "rows-0.jsonl")[0]), 13)
            other = loop.Search("notify", "seed.json", tmp, asker, None, None, 300, 0, model="jev-other")
            with self.assertRaisesRegex(RuntimeError, "existing manifest differs"):
                other.enumerate()
            self.assertEqual(len(asker.calls), paid + 1, "the manifest refuses a changed model before a paid call")

    def test_the_request_cap_counts_the_whole_run_before_a_new_round(self):
        with tempfile.TemporaryDirectory() as raw:
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", Path(raw), asker, None, None, 40, 1)
            search.enumerate()
            search.round_zero()
            with self.assertRaisesRegex(RuntimeError, "run request cap"):
                search.step(1, loop.Proposal(add={"q": {"type": "noul", "instructions": "x", "yes": "y", "no": "n"}}))
            self.assertEqual(len(asker.calls), 2, "the extra paid round never reaches the asker")

    def test_a_crash_reservation_is_never_sent_again(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            asker = FakeAsker(sample(), informative)
            search = loop.Search("notify", "seed.json", tmp, asker, None, None, 100, 0)
            search.enumerate()

            def interrupted(spec, out, states):
                asker(spec, out, states)
                raise RuntimeError("process stopped after the wire")

            search.asker = interrupted
            with self.assertRaisesRegex(RuntimeError, "process stopped"):
                search.round_zero()
            prior_calls = len(asker.calls)
            again = loop.Search("notify", "seed.json", tmp, asker, None, None, 100, 0)
            again.enumerate()
            with self.assertRaisesRegex(RuntimeError, "reserved request has no saved response"):
                again.round_zero()
            self.assertEqual(len(asker.calls), prior_calls, "unknown paid calls are held for review, never retried")

    def test_the_model_and_the_metrics_read_a_known_signal(self):
        scores = [0.9, 0.8, 0.3, 0.2]
        labels = [True, True, False, False]
        self.assertEqual(loop.auc(scores, labels), 1.0)
        self.assertEqual(loop.auc([0.5] * 4, labels), 0.5)
        self.assertIsNone(loop.auc([0.1], [True]))
        self.assertAlmostEqual(loop.wilson_lower(50, 100), 0.4038, places=3)
        self.assertEqual(loop.flatten({"n": 0.7, "s": {"score": 1.5, "probabilities": {"0": 0.5}}, "c": {"probabilities": {"a": 0.25, "b": 0.75}}}),
                         {"n": 0.7, "s": 1.5, "c.a": 0.25, "c.b": 0.75})


class Seed(unittest.TestCase):
    def test_the_ledger_names_are_the_use_tables_own(self):
        jev = (REPO / "crates/zerocode-core/src/jev.rs").read_text()
        for seat, ledger in seed.LEDGERS.items():
            if seat == "routing":
                continue  # route-outcomes is the router's own file, not a use-table ledger
            row = re.search(rf'pub\s+const\s+{seat.upper()}\s*:\s*JevUse\s*=\s*JevUse\s*\{{(.*?)\n\}};', jev, re.S)
            self.assertIsNotNone(row, seat)
            self.assertEqual(re.search(r'ledger:\s*"([^"]+)"', row.group(1)).group(1), ledger, seat)
        outcome = (REPO / "zo-ide/crates/runtime/src/model_router/outcome.rs").read_text()
        self.assertEqual(seed.LEDGERS["routing"], re.search(r'const\s+OUTCOME_FILE\s*:\s*&str\s*=\s*"([^"]+)"\s*;', outcome).group(1))

    def test_the_seed_counts_marks_and_reads_no_words(self):
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw)
            (home / "jev").mkdir()
            (home / "jev" / "notify-call.jsonl").write_text('{"agreed": true, "words": "secret words"}\n{"agreed": false}\n{"outcome": "timeout"}\n')
            project = home / "projects" / "p" / "state" / "smart-router"
            project.mkdir(parents=True)
            (project / "patch-review.jsonl").write_text('{"agreed": true}\n')
            notify_seed = home / "notify-seed.json"
            notify_seed.write_text(json.dumps({"rows": [{"words": "secret words"}, {}]}))
            built = seed.build(home, {"notify": notify_seed, "patch_review": None})
            by_seat = {row["seat"]: row for row in built["candidates"]}
            self.assertEqual(by_seat["notify"]["marks"], {"rows": 3, "agreed": 1, "disagreed": 1, "unmarked": 1})
            self.assertEqual(by_seat["patch_review"]["marks"]["rows"], 1)
            self.assertEqual(by_seat["notify"]["source"]["rings"], 2)
            self.assertIsNone(by_seat["patch_review"]["source"])
            self.assertNotIn("secret words", json.dumps(built))
            self.assertEqual([row["seat"] for row in built["candidates"] if row["supported"]], ["patch_review", "notify"])


if __name__ == "__main__":
    unittest.main()
