#!/usr/bin/env python3
"""Contract for `tools/jev_token_diet_baseline.py` — its DEFINITIONS, pinned on
transcripts written here, so a change to a rule shows up as a red case rather
than as a quietly different number in the design doc.

Run: python3 tools/tests/test_jev_token_diet_baseline.py   (stdlib only)
"""

import importlib.util
import json
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("jev_token_diet_baseline", REPO / "tools" / "jev_token_diet_baseline.py")
diet = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = diet
_spec.loader.exec_module(diet)

WINDOW = (0, 10**13)


def user(text):
    return {"role": "user", "blocks": [{"type": "text", "text": text}]}


def assistant(*blocks, read=0, write=0, uncached=100, output=10, model="claude-opus-5"):
    return {"role": "assistant", "model": model, "blocks": list(blocks),
            "usage": {"cache_read_input_tokens": read, "cache_creation_input_tokens": write,
                      "input_tokens": uncached, "output_tokens": output}}


def text(body):
    return {"type": "text", "text": body}


def use(call_id, name, **arguments):
    return {"type": "tool_use", "id": call_id, "name": name, "input": json.dumps(arguments)}


def result(call_id, name, output):
    return {"role": "tool", "blocks": [{"type": "tool_result", "tool_use_id": call_id, "tool_name": name,
                                        "output": output, "is_error": False}]}


def read_output(path, content):
    return json.dumps({"type": "text", "file": {"filePath": path, "content": content, "numLines": 1,
                                                 "startLine": 1, "totalLines": 1}})


def build(messages, compactions=()):
    ordered = [(index, message, None) for index, message in enumerate(messages)]
    builder = diet.SessionBuilder("project/session-1-0", ordered, list(compactions), subagent=False, ledger={},
                                  effort_pref=None, cleared={}, skill_share=0.0, window=WINDOW)
    return builder.build()


class Turns(unittest.TestCase):
    def test_a_person_opens_a_turn_and_the_gate_and_steering_continue_it(self):
        session = build([
            user("fix the parser"),
            assistant(text("done")),
            user("[zo:turn-end-gate] say what you verified"),
            assistant(text("ran it")),
            user("[User steering — typed while you worked] also the lexer"),
            assistant(text("ok")),
            user("[task notification] background job finished"),
            assistant(text("noted")),
            user("next question"),
            assistant(text("answer")),
        ])
        self.assertEqual([t.kind for t in session.turns], ["person", "notification", "person"])
        self.assertEqual(len(session.turns[0].requests), 3)
        self.assertEqual(session.turns[0].gates, [2])

    def test_a_gate_is_one_gate_however_many_carriers_name_it(self):
        gate = ("[zo:turn-end-gate] <system-reminder>Your reply ended by promising work you have not done yet."
                "</system-reminder>")
        session = build([user("fix it"), assistant(text("will do")), user(gate), assistant(text("done"))])
        self.assertEqual(session.turns[0].gates, [2])

    def test_harness_text_is_a_reminder_and_a_person_is_user_input(self):
        session = build([user("fix it"), assistant(text("x")), user("[zo:turn-end-gate] check"), assistant(text("y"))])
        self.assertIn("user_input", session.messages[0].kinds)
        self.assertNotIn("user_input", session.messages[2].kinds)
        self.assertIn("reminders", session.messages[2].kinds)


class Repeats(unittest.TestCase):
    CONTENT = "\n".join(f"let value_{n} = compute_something_long({n});" for n in range(20))

    def test_an_identical_read_is_safe_and_a_changed_one_is_not(self):
        session = build([
            user("look"),
            assistant(use("a", "read_file", path="src/lib.rs")),
            result("a", "read_file", read_output("src/lib.rs", self.CONTENT)),
            assistant(use("b", "read_file", path="src/lib.rs")),
            result("b", "read_file", read_output("src/lib.rs", self.CONTENT)),
            assistant(use("c", "edit_file", path="src/lib.rs", old_string="x", new_string="y")),
            result("c", "edit_file", "{}"),
            assistant(use("d", "read_file", path="src/lib.rs")),
            result("d", "read_file", read_output("src/lib.rs", self.CONTENT.replace("value_3", "renamed_3"))),
            assistant(text("done")),
        ])
        calls = session.calls
        self.assertEqual(calls[1].duplicate, "safe")
        self.assertTrue(calls[1].rule_would_block, "nothing touched the file between the two reads")
        self.assertEqual(calls[3].duplicate, "changed")
        self.assertFalse(calls[3].rule_would_block, "an edit to the path ran since the last read")

    def test_another_window_of_a_file_already_in_context_is_a_safe_near_repeat(self):
        session = build([
            user("look"),
            assistant(use("a", "read_file", path="src/lib.rs")),
            result("a", "read_file", read_output("src/lib.rs", self.CONTENT)),
            assistant(use("b", "read_file", path="src/lib.rs", offset=5, limit=10)),
            result("b", "read_file", read_output("src/lib.rs", "\n".join(self.CONTENT.splitlines()[5:15]))),
            assistant(text("done")),
        ])
        second = session.calls[1]
        self.assertIsNone(second.duplicate, "the inputs differ")
        self.assertEqual(second.near, "safe")
        self.assertTrue(second.redundant)

    def test_a_shell_command_since_the_earlier_read_stops_the_rule(self):
        session = build([
            user("look"),
            assistant(use("a", "grep_search", pattern="compute_something_long", path="src")),
            result("a", "grep_search", json.dumps({"content": self.CONTENT})),
            assistant(use("b", "bash", command="cargo fmt")),
            result("b", "bash", json.dumps({"stdout": ""})),
            assistant(use("c", "grep_search", pattern="compute_something_long", path="src")),
            result("c", "grep_search", json.dumps({"content": self.CONTENT})),
            assistant(text("done")),
        ])
        third = session.calls[2]
        self.assertEqual(third.duplicate, "safe")
        self.assertFalse(third.rule_would_block)


class Use(unittest.TestCase):
    def test_a_result_is_used_when_a_word_it_introduced_is_said_later_in_the_turn(self):
        body = "fn parse_header_block() {}\nfn render_footer_block() {}\nfn measure_column_width() {}"
        session = build([
            user("where is the header parsed"),
            assistant(use("a", "grep_search", pattern="header", path="src")),
            result("a", "grep_search", json.dumps({"content": body})),
            assistant(text("It is parse_header_block.")),
            user("thanks"),
            assistant(use("b", "grep_search", pattern="footer_thing", path="src")),
            result("b", "grep_search", json.dumps({"content": "fn unrelated_helper_one() {}\nfn unrelated_helper_two() {}\nfn unrelated_helper_six() {}"})),
            assistant(text("Nothing relevant there.")),
        ])
        self.assertTrue(session.calls[0].used)
        self.assertFalse(session.calls[1].used)

    def test_a_word_the_person_wrote_is_not_evidence(self):
        body = "fn parse_header_block() {}\nfn render_footer_block() {}\nfn measure_column_width() {}"
        session = build([
            user("is parse_header_block, render_footer_block or measure_column_width dead code"),
            assistant(use("a", "grep_search", pattern="block", path="src")),
            result("a", "grep_search", json.dumps({"content": body})),
            assistant(text("parse_header_block is used.")),
        ])
        self.assertIsNone(session.calls[0].used, "every introduced word came from the person")


class CarryCost(unittest.TestCase):
    def session(self, reads):
        messages = [user("go")]
        for n, read in enumerate(reads):
            messages.append(assistant(text(f"step {n}"), read=read, uncached=100))
        return build(messages)

    def test_a_block_is_fresh_once_then_read_until_the_session_ends(self):
        session = self.session([0, 1000, 1100, 1200])
        cost = diet.Carry(session, diet.Prices(diet.PRICE_TABLE)).block(50, 0)
        self.assertEqual((cost.fresh, cost.read), (50, 150))

    def test_a_break_rewrites_it_and_a_cleared_block_stops_at_the_first_break(self):
        session = self.session([0, 1000, 10, 1200])  # the third request read almost nothing back
        self.assertTrue(session.requests[2].brk)
        carry = diet.Carry(session, diet.Prices(diet.PRICE_TABLE))
        self.assertEqual((carry.block(50, 0).fresh, carry.block(50, 0).read), (100, 100))
        cleared = carry.block(50, 0, cleared=True)
        self.assertEqual((cleared.fresh, cleared.read), (50, 50))

    def test_a_model_that_never_reads_a_cache_has_no_breaks(self):
        messages = [user("go")] + [assistant(text("s"), read=0, uncached=500 + n, model="glm-5.3") for n in range(3)]
        self.assertFalse(any(r.brk for r in build(messages).requests))


class Arithmetic(unittest.TestCase):
    def test_solve_recovers_non_negative_coefficients(self):
        rows = [[1, 0, 2, 1], [0, 1, 1, 1], [2, 1, 0, 1], [1, 3, 1, 1], [4, 1, 2, 1]]
        truth = [0.4, 0.3, 0.0, 5.0]
        ys = [sum(t * x for t, x in zip(truth, row)) for row in rows]
        for got, want in zip(diet.solve(rows, ys), truth):
            self.assertAlmostEqual(got, want, places=6)

    def test_a_negative_judged_n_times_misses_with_one_minus_h_to_the_n(self):
        perfect = diet.Cost(read=1000)
        miss = diet.Cost(read=100)
        net = diet.net_at(perfect, [(miss, 1), (miss, 3)], 0.9)
        self.assertAlmostEqual(net.read, 900 - 100 * 0.1 - 100 * (1 - 0.9 ** 3))

    def test_union_takes_each_item_once_at_its_largest_claim(self):
        first, second = diet.Claims(), diet.Claims()
        value = diet.Cost(read=100)
        first.claim("s:result:1", value, 0.5)
        second.claim("s:result:1", value, 1.0)
        second.claim("s:request:2", diet.Cost(read=10))
        self.assertEqual(diet.union([first, second]).total().read, 110)

    def test_prices_match_a_dated_anthropic_id_and_leave_an_unknown_model_unpriced(self):
        prices = diet.Prices(diet.PRICE_TABLE)
        self.assertEqual(prices.row("claude-haiku-4-5-20251001")[0], "anthropic")
        self.assertIsNone(prices.cost("gemini-3.8-flash", 1, 1, 1))
        opus = prices.row("claude-opus-5")[1]
        write = prices.anthropic["cache_write_5m_rate"]
        self.assertAlmostEqual(prices.cost("claude-opus-5", 1_000_000, 0, 0), opus["input"] * write)


if __name__ == "__main__":
    unittest.main()
