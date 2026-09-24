#!/usr/bin/env python3
"""Contract for `tools/ask-wait-replay/seed.py` — it copies what the ledger
held about a question's receiver while the asker waited, reads the ledger's
own definition of an answer, moves nothing, and lets no body out.

Run: python3 tools/tests/test_ask_wait_replay_seed.py   (stdlib only)
"""

import importlib.util
import json
import re
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("ask_wait_replay_seed", REPO / "tools" / "ask-wait-replay" / "seed.py")
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)

CORE = (REPO / "crates" / "zerocode-core" / "src" / "orchestration.rs").read_text()

SCHEMA = """
CREATE TABLE ledger_messages (ledger_id TEXT, ordinal INTEGER, run TEXT, id TEXT, sender TEXT,
  recipient TEXT, kind TEXT, body TEXT, subject TEXT DEFAULT '', priority TEXT DEFAULT 'normal',
  payload TEXT DEFAULT '', thread TEXT, task TEXT, dispatch TEXT, author_seat TEXT, created_ms INTEGER);
CREATE TABLE ledger_dispatches (ledger_id TEXT, ordinal INTEGER, run TEXT, id TEXT, task TEXT,
  worker TEXT, started_ms INTEGER, ended_ms INTEGER, succeeded INTEGER, retry_of TEXT, remote TEXT);
CREATE TABLE ledger_workers (ledger_id TEXT, ordinal INTEGER, run TEXT, id TEXT, team TEXT, agent TEXT,
  pane TEXT, state TEXT, started_ms INTEGER, dispatch TEXT, taken_over INTEGER DEFAULT 0);
"""

SECRET_BODY = "the token is sk-live-do-not-copy"


def a_ledger(path: Path) -> None:
    db = sqlite3.connect(path)
    db.executescript(SCHEMA)
    rows = []

    def message(ordinal, mid, sender, recipient, kind, body, thread=None, task=None, dispatch=None, created=0):
        rows.append(("L", ordinal, "run-1", mid, sender, recipient, kind, body, thread, task, dispatch, created))

    # A worker receiver carrying an open attempt.
    db.execute("INSERT INTO ledger_workers VALUES ('L',0,'run-1','w-1','team-1','claude','%2','active',100,'dp-1',0)")
    db.execute("INSERT INTO ledger_workers VALUES ('L',1,'run-1','w-2','team-1','codex','%3','active',100,'dp-2',0)")
    db.execute("INSERT INTO ledger_dispatches VALUES ('L',0,'run-1','dp-1','t-1','w-1',100,5000,NULL,NULL,NULL)")
    db.execute("INSERT INTO ledger_dispatches VALUES ('L',1,'run-1','dp-2','t-2','w-2',100,NULL,NULL,NULL,NULL)")
    # A turn end BEFORE the question: never a fact about this wait.
    message(0, "m-0", "ledger", "run:run-1", "went_quiet",
            json.dumps({"workerId": "w-1", "dispatchId": "dp-1", "turnEndedMs": 900}), created=950)
    # The question, from w-2 to w-1.
    message(1, "m-1", "worker:w-2", "worker:w-1", "question", SECRET_BODY, dispatch="dp-2", created=1000)
    # A turn end the ledger learned during the wait.
    message(2, "m-2", "ledger", "run:run-1", "went_quiet",
            json.dumps({"workerId": "w-1", "dispatchId": "dp-1", "turnEndedMs": 2000}), created=2010)
    # The ledger's own line in the thread, wearing the receiver's name: never the answer.
    message(3, "m-3", "worker:w-1", "worker:w-2", "went_quiet", "{}", thread="m-1", created=2500)
    # A stranger's word in the thread: not the answer either.
    message(4, "m-4", "worker:w-9", "worker:w-2", "status", "me too", thread="m-1", created=2600)
    # The answer.
    message(5, "m-5", "worker:w-1", "worker:w-2", "question", SECRET_BODY + " answered", thread="m-1", created=4000)
    # A turn end after the answer: not this wait's.
    message(6, "m-6", "ledger", "run:run-1", "went_quiet",
            json.dumps({"workerId": "w-1", "dispatchId": "dp-1", "turnEndedMs": 4500}), created=4510)
    # A second question, to the run: unobservable, and never answered — the asker's dispatch is open, so censored.
    message(7, "m-7", "worker:w-2", "run:run-1", "question", "how?", dispatch="dp-2", created=6000)
    # A third, to w-1 after its attempt ended (5000): no open attempt.
    message(8, "m-8", "worker:w-2", "worker:w-1", "question", "again?", dispatch="dp-2", created=7000)
    db.executemany("INSERT INTO ledger_messages (ledger_id, ordinal, run, id, sender, recipient, kind, body, thread, task, dispatch, created_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)", rows)
    db.commit()
    db.close()


class Gathering(unittest.TestCase):
    def setUp(self):
        self.raw = tempfile.TemporaryDirectory()
        self.db = Path(self.raw.name) / "authority.sqlite"
        a_ledger(self.db)

    def tearDown(self):
        self.raw.cleanup()

    def rows(self):
        return {row["question"]: row for row in seed.gather(self.db)["rows"]}

    def test_the_answer_is_the_asked_seats_word_and_never_the_ledgers_or_a_strangers(self):
        rows = self.rows()
        first = rows[seed.hashed("m-1")]
        self.assertEqual(first["outcome"], "answered")
        self.assertEqual(first["wait_ms"], 3000, "the answer is m-5 at 4000, not the ledger's line at 2500")
        self.assertTrue(first["misread_answer"], "today's reading would have taken the ledger's line at 2500")

    def test_only_facts_learned_during_the_wait_are_carried(self):
        rows = self.rows()
        first = rows[seed.hashed("m-1")]
        self.assertEqual(first["receiver_facts"], "observed")
        self.assertEqual([one["fact_ms"] for one in first["turn_ends"]], [2000])
        self.assertEqual(first["turn_ends"][0]["learned_ms"], 2010)
        self.assertEqual(first["wait_after_first_turn_end_ms"], 2000)
        self.assertIsNone(first["receiver_ended"], "the attempt ended after the answer")

    def test_a_run_receiver_is_unobservable_and_an_open_asker_is_censored(self):
        rows = self.rows()
        second = rows[seed.hashed("m-7")]
        self.assertEqual(second["receiver_facts"], "unobservable")
        self.assertEqual(second["outcome"], "censored")
        third = rows[seed.hashed("m-8")]
        self.assertEqual(third["receiver_facts"], "no_open_attempt")

    def test_nothing_leaves_but_hashes_words_and_numbers(self):
        text = json.dumps(seed.gather(self.db))
        self.assertNotIn("sk-live", text)
        self.assertNotIn("m-1", text.replace(seed.hashed("m-1"), ""))
        self.assertNotIn("run-1", text)

    def test_the_ledger_is_not_moved(self):
        before = self.db.read_bytes()
        seed.gather(self.db)
        self.assertEqual(before, self.db.read_bytes())
        with self.assertRaises(sqlite3.OperationalError):
            seed.open_read_only(self.db).execute("DELETE FROM ledger_messages")


class Constants(unittest.TestCase):
    def test_the_ledgers_own_kinds_and_words_match_the_rust_source(self):
        self.assertIn(f'pub const LEDGER_ITSELF: &str = "{seed.LEDGER_ITSELF}";', CORE)
        self.assertIn(f'pub const STALL_JUDGED_REASON: &str = "{seed.STALL_JUDGED_REASON}";', CORE)
        own = re.search(r"pub const fn is_the_ledgers_own\(self\) -> bool \{\s*matches!\(\s*self,\s*(.*?)\)", CORE, re.S)
        self.assertIsNotNone(own)
        variants = re.findall(r"Self::([A-Za-z]+)", own.group(1))
        words = []
        for variant in variants:
            found = re.search(rf"Self::{variant} => \"([a-z_]+)\"", CORE)
            self.assertIsNotNone(found, variant)
            words.append(found.group(1))
        self.assertEqual(tuple(words), seed.LEDGERS_OWN_KINDS)


if __name__ == "__main__":
    unittest.main()
