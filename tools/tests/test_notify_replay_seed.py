#!/usr/bin/env python3
"""Contract for `tools/notify-replay/seed.py` — it gathers rings and the facts
the bell had at each, and reads nothing from after a ring except its label.

Run: python3 tools/tests/test_notify_replay_seed.py   (stdlib only)
"""

import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("notify_replay_seed", REPO / "tools" / "notify-replay" / "seed.py")
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)

BASE_MS = 1_789_600_000_000


def stamp(ms: int) -> str:
    return datetime.fromtimestamp(ms / 1000, tz=timezone.utc).isoformat().replace("+00:00", "Z")


def user(ms: int, text: str, *, source: str = "typed", meta: bool = False) -> dict:
    row = {"type": "user", "timestamp": stamp(ms), "cwd": "/w/api", "promptSource": source,
           "message": {"role": "user", "content": text}}
    if meta:
        row["isMeta"] = True
    return row


def assistant(ms: int, text: str, *, ask: tuple[str, str] | None = None) -> dict:
    content = [{"type": "text", "text": text}]
    if ask:
        tool_use_id, question = ask
        content.append({"type": "tool_use", "id": tool_use_id, "name": "AskUserQuestion",
                        "input": {"questions": [{"question": question}]}})
    return {"type": "assistant", "timestamp": stamp(ms), "cwd": "/w/api", "message": {"role": "assistant", "content": content}}


def tool_result(ms: int, tool_use_id: str) -> dict:
    return {"type": "user", "timestamp": stamp(ms), "cwd": "/w/api",
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_use_id, "content": "yes"}]}}


def write(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(row) + "\n" for row in rows))


class Gathering(unittest.TestCase):
    def test_a_finished_turn_is_a_ring_labeled_by_the_persons_next_prompt(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "p" / "s1.jsonl"
            write(path, [
                user(BASE_MS, "start"),
                assistant(BASE_MS + 10_000, "first line of the answer\nsecond line"),
                user(BASE_MS + 40_000, "thanks, go on"),
                assistant(BASE_MS + 90_000, "done"),
                user(BASE_MS + 90_000 + seed.LABEL_WINDOW_MS + 1, "too late to count"),
            ])
            events, hands = seed.events_of(path)
            self.assertEqual([e["verb"] for e in events], [seed.VERB_FINISHED, seed.VERB_FINISHED])
            self.assertEqual(events[0]["words"], "first line of the answer")
            self.assertTrue(seed.reacted(events[0]))
            self.assertFalse(seed.reacted(events[1]), "a prompt past the minute is not the reaction")
            self.assertEqual(hands, [BASE_MS, BASE_MS + 40_000, BASE_MS + 90_000 + seed.LABEL_WINDOW_MS + 1])
            self.assertEqual(events[0]["pane"], "api")

    def test_a_question_is_needs_input_answered_by_its_tool_result(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "p" / "s2.jsonl"
            write(path, [
                user(BASE_MS, "start"),
                assistant(BASE_MS + 5_000, "", ask=("t1", "merge now?")),
                tool_result(BASE_MS + 25_000, "t1"),
                assistant(BASE_MS + 30_000, "merged"),
            ])
            events, _ = seed.events_of(path)
            asked = [e for e in events if e["verb"] == seed.VERB_ATTENTION]
            self.assertEqual(len(asked), 1)
            self.assertEqual(asked[0]["words"], "merge now?")
            self.assertEqual(asked[0]["reactedAt"], BASE_MS + 25_000)
            self.assertTrue(seed.reacted(asked[0]))
            # The turn the transcript ends on rang too, with no reaction.
            self.assertEqual(events[-1]["verb"], seed.VERB_FINISHED)
            self.assertFalse(seed.reacted(events[-1]))

    def test_the_ledgers_typing_and_hook_notes_are_nobodys_hand(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "p" / "s3.jsonl"
            write(path, [
                user(BASE_MS, '<pasted_content id="1">\nYou are a worker in this window\'s orchestration'),
                assistant(BASE_MS + 10_000, "reporting"),
                user(BASE_MS + 20_000, "You have 1 orchestration message. Run `zerocode-orc check`."),
                assistant(BASE_MS + 30_000, "checked"),
                user(BASE_MS + 35_000, "<task-notification>done</task-notification>", source="system"),
                user(BASE_MS + 36_000, "Continue from where you left off.", meta=True),
                assistant(BASE_MS + 40_000, "final"),
            ])
            events, hands = seed.events_of(path)
            self.assertEqual(hands, [], "nothing here was a person")
            # Every turn end still rang; none was reacted to by a person.
            self.assertEqual([e["verb"] for e in events], [seed.VERB_FINISHED])
            self.assertFalse(seed.reacted(events[0]))

    def test_a_stopped_turn_wears_the_stopped_verb(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "p" / "s4.jsonl"
            write(path, [
                user(BASE_MS, "start"),
                assistant(BASE_MS + 5_000, "half way"),
                user(BASE_MS + 8_000, "[Request interrupted by user]"),
                user(BASE_MS + 9_000, "do it differently"),
                assistant(BASE_MS + 20_000, "ok"),
            ])
            events, _ = seed.events_of(path)
            self.assertEqual(events[0]["verb"], seed.VERB_STOPPED)
            self.assertTrue(events[0]["interrupted"])
            self.assertEqual(events[0]["reactedAt"], BASE_MS + 9_000)


class NoFutureInformation(unittest.TestCase):
    def test_attendance_reads_only_hands_before_the_ring(self):
        hands = [BASE_MS - 1_000, BASE_MS + 5_000]
        self.assertEqual(seed.attendance_at(BASE_MS, hands), "present")
        self.assertEqual(seed.attendance_at(BASE_MS - seed.ATTENDANCE_WINDOW_MS - 2_000, hands), "away")
        self.assertEqual(seed.attendance_at(BASE_MS - 1_000, hands), "away", "a hand at the same instant is not before it")
        self.assertEqual(seed.attendance_at(BASE_MS + 2_000, [BASE_MS + 5_000]), "away", "a later hand is the future")
        one = [BASE_MS - 1_000]
        self.assertEqual(seed.attendance_at(BASE_MS - 1_000 + seed.ATTENDANCE_WINDOW_MS, one), "present")
        self.assertEqual(seed.attendance_at(BASE_MS - 1_000 + seed.ATTENDANCE_WINDOW_MS + 1, one), "away")

    def test_a_recent_rings_reaction_is_unknown_while_its_minute_is_open(self):
        earlier = [
            {"at": BASE_MS, "verb": seed.VERB_FINISHED, "interrupted": False, "reactedAt": BASE_MS + 10_000},
            {"at": BASE_MS + 100_000, "verb": seed.VERB_FINISHED, "interrupted": False, "reactedAt": BASE_MS + 110_000},
        ]
        now = {"at": BASE_MS + 130_000}
        recent = seed.recent_before(now, earlier)
        self.assertEqual([one["reacted"] for one in recent], [True, None])
        self.assertEqual(recent[1]["agoMs"], 30_000)
        many = [{"at": BASE_MS + n, "verb": seed.VERB_FINISHED, "interrupted": False, "reactedAt": None} for n in range(seed.RECENT_CAP + 4)]
        self.assertEqual(len(seed.recent_before({"at": BASE_MS + 10 ** 6}, many)), seed.RECENT_CAP)

    def test_waiting_panes_counts_other_panes_open_questions_at_that_moment(self):
        asks = [(BASE_MS, BASE_MS + 50_000, "a"), (BASE_MS + 10_000, None, "b"), (BASE_MS + 20_000, BASE_MS + 30_000, "c")]
        self.assertEqual(seed.waiting_panes_at(BASE_MS + 25_000, "z", asks), 3)
        self.assertEqual(seed.waiting_panes_at(BASE_MS + 40_000, "z", asks), 2)
        self.assertEqual(seed.waiting_panes_at(BASE_MS + 40_000, "b", asks), 1, "a pane's own question is not another pane's")
        self.assertEqual(seed.waiting_panes_at(BASE_MS + 60_000, "z", asks), 1)


class TheCommand(unittest.TestCase):
    def test_it_writes_the_windows_and_the_rows_with_the_label_apart(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            write(tmp / "projects" / "p" / "s.jsonl", [
                user(BASE_MS, "start"),
                assistant(BASE_MS + 10_000, "answer"),
                user(BASE_MS + 20_000, "next"),
            ])
            out = tmp / "seed.json"
            result = subprocess.run(
                [sys.executable, str(REPO / "tools/notify-replay/seed.py"), "--projects", str(tmp / "projects"), "--days", "100000", "--out", str(out)],
                text=True, capture_output=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            written = json.loads(out.read_text())
            self.assertEqual(written["labelWindowMs"], seed.LABEL_WINDOW_MS)
            self.assertEqual(written["attendanceWindowMs"], seed.ATTENDANCE_WINDOW_MS)
            self.assertEqual(written["recentCap"], seed.RECENT_CAP)
            self.assertEqual(len(written["rows"]), 1)
            row = written["rows"][0]
            self.assertEqual(row["verb"], seed.VERB_FINISHED)
            self.assertEqual(row["attendance"], "present")
            self.assertEqual(row["label"], {"reacted": True, "afterMs": 10_000})
            self.assertEqual(row["recent"], [])
            self.assertEqual(row["waitingPanes"], 0)


class ConstantsMatchTheirSource(unittest.TestCase):
    """The numbers the seeder carries are copies of Rust constants, and a copy
    that drifts is a seed made for another window."""

    def value_of(self, text: str, name: str, depth: int = 0) -> int:
        self.assertLess(depth, 6, f"{name} chains too deep")
        found = re.search(rf"const\s+{name}\s*:[^=]*=\s*([^;]+);", text)
        self.assertIsNotNone(found, f"{name} is no longer declared")
        expression = found.group(1).strip()
        if re.fullmatch(r"[0-9_*\s]+", expression):
            return eval(expression.replace("_", ""))  # digits and `*` only
        return self.value_of(text, expression, depth + 1)

    def setUp(self):
        self.jev = (REPO / "crates/zerocode-core/src/jev.rs").read_text()

    def test_the_label_window(self):
        self.assertEqual(seed.LABEL_WINDOW_MS, self.value_of(self.jev, "NOTIFY_LABEL_WINDOW_MS"))

    def test_the_attendance_window(self):
        self.assertEqual(seed.ATTENDANCE_WINDOW_MS, self.value_of(self.jev, "NOTIFY_ATTENDANCE_WINDOW_MS"))

    def test_the_recent_cap(self):
        self.assertEqual(seed.RECENT_CAP, self.value_of(self.jev, "NOTIFY_RECENT_CAP"))

    def test_the_words_cap(self):
        self.assertEqual(seed.WORDS_CHAR_CAP, self.value_of(self.jev, "NOTIFY_WORDS_CHAR_CAP"))

    def test_the_verbs_are_the_notify_tables(self):
        notify = (REPO / "crates/zerocode-core/src/notify.rs").read_text()
        for name, word in [("VERB_ATTENTION", seed.VERB_ATTENTION), ("VERB_FINISHED", seed.VERB_FINISHED), ("VERB_STOPPED", seed.VERB_STOPPED)]:
            found = re.search(rf'const\s+{name}:\s*&str\s*=\s*"([^"]+)";', notify)
            self.assertIsNotNone(found, name)
            self.assertEqual(word, found.group(1))


if __name__ == "__main__":
    unittest.main()
