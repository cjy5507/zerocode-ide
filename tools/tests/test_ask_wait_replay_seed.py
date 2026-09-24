#!/usr/bin/env python3
"""Contract for `tools/ask-wait-replay/seed.py` — it copies what the ledger
held about a question's receiver while the asker waited, reads the ledger's
own definition of an answer, reads one ledger at a time, calls an ending by
the ledger's record of it and never by guess, reads the rule's own notices,
moves nothing, and lets no body out.

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
CREATE TABLE ledger_tasks (ledger_id TEXT, ordinal INTEGER, run TEXT, id TEXT, spec TEXT DEFAULT '',
  title TEXT DEFAULT '', parent TEXT, status TEXT, result TEXT DEFAULT '', failures INTEGER DEFAULT 0,
  created_ms INTEGER DEFAULT 0);
"""

SECRET_BODY = "the token is sk-live-do-not-copy"
ADVICE = "the seat this question was asked of was ended by its coordinator"


class Rows:
    """One ledger's rows, written as the store keys them."""

    def __init__(self, db: sqlite3.Connection, ledger: str = "L"):
        self.db = db
        self.ledger = ledger
        self.ordinal = 0

    def next(self) -> int:
        self.ordinal += 1
        return self.ordinal

    def message(self, mid, sender, recipient, kind, body, thread=None, task=None, dispatch=None, created=0):
        self.db.execute(
            "INSERT INTO ledger_messages (ledger_id, ordinal, run, id, sender, recipient, kind, body, thread, "
            "task, dispatch, created_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            (self.ledger, self.next(), "run-1", mid, sender, recipient, kind, body, thread, task, dispatch, created),
        )

    def worker(self, wid, dispatch, started=100):
        self.db.execute(
            "INSERT INTO ledger_workers VALUES (?,?,?,?,?,?,?,?,?,?,0)",
            (self.ledger, self.next(), "run-1", wid, "team-1", "claude", "%2", "active", started, dispatch),
        )

    def dispatch(self, did, task, worker, started, ended=None):
        self.db.execute(
            "INSERT INTO ledger_dispatches VALUES (?,?,?,?,?,?,?,?,NULL,NULL,NULL)",
            (self.ledger, self.next(), "run-1", did, task, worker, started, ended),
        )

    def task(self, tid, result=""):
        self.db.execute(
            "INSERT INTO ledger_tasks (ledger_id, ordinal, run, id, status, result) VALUES (?,?,?,?,?,?)",
            (self.ledger, self.next(), "run-1", tid, "ready", result),
        )


def a_ledger(path: Path) -> None:
    db = sqlite3.connect(path)
    db.executescript(SCHEMA)
    rows = Rows(db)
    # A worker receiver carrying an open attempt.
    rows.worker("w-1", "dp-1")
    rows.worker("w-2", "dp-2")
    rows.dispatch("dp-1", "t-1", "w-1", 100, 5000)
    rows.dispatch("dp-2", "t-2", "w-2", 100)
    # A turn end BEFORE the question: never a fact about this wait.
    rows.message("m-0", "ledger", "run:run-1", "went_quiet",
                 json.dumps({"workerId": "w-1", "dispatchId": "dp-1", "turnEndedMs": 900}), created=950)
    # The question, from w-2 to w-1.
    rows.message("m-1", "worker:w-2", "worker:w-1", "question", SECRET_BODY, dispatch="dp-2", created=1000)
    # A turn end the ledger learned during the wait.
    rows.message("m-2", "ledger", "run:run-1", "went_quiet",
                 json.dumps({"workerId": "w-1", "dispatchId": "dp-1", "turnEndedMs": 2000}), created=2010)
    # The ledger's own line in the thread, wearing the receiver's name: never the answer.
    rows.message("m-3", "worker:w-1", "worker:w-2", "went_quiet", "{}", thread="m-1", created=2500)
    # A stranger's word in the thread: not the answer either.
    rows.message("m-4", "worker:w-9", "worker:w-2", "status", "me too", thread="m-1", created=2600)
    # The answer.
    rows.message("m-5", "worker:w-1", "worker:w-2", "question", SECRET_BODY + " answered", thread="m-1", created=4000)
    # A turn end after the answer: not this wait's.
    rows.message("m-6", "ledger", "run:run-1", "went_quiet",
                 json.dumps({"workerId": "w-1", "dispatchId": "dp-1", "turnEndedMs": 4500}), created=4510)
    # A second question, to the run: unobservable, and never answered — the asker's dispatch is open, so censored.
    rows.message("m-7", "worker:w-2", "run:run-1", "question", "how?", dispatch="dp-2", created=6000)
    # A third, to w-1 after its attempt ended (5000): no open attempt.
    rows.message("m-8", "worker:w-2", "worker:w-1", "question", "again?", dispatch="dp-2", created=7000)
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
        self.assertTrue(first["misread_answer"], "the older reading would have taken the ledger's line at 2500")

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


class Scope(unittest.TestCase):
    """Two ledgers in one store mint the same ids; a replay reads one."""

    def setUp(self):
        self.raw = tempfile.TemporaryDirectory()
        self.db = Path(self.raw.name) / "authority.sqlite"
        db = sqlite3.connect(self.db)
        db.executescript(SCHEMA)
        for ledger, answered_at in (("L", 4000), ("M", 9000)):
            rows = Rows(db, ledger)
            rows.worker("w-1", None)
            rows.worker("w-2", None)
            rows.message("m-1", "worker:w-2", "worker:w-1", "question", "which?", created=1000)
            rows.message("m-2", "worker:w-1", "worker:w-2", "question", "main", thread="m-1", created=answered_at)
        db.commit()
        db.close()

    def tearDown(self):
        self.raw.cleanup()

    def test_a_store_of_two_ledgers_is_read_one_at_a_time(self):
        with self.assertRaises(seed.LedgerScopeError):
            seed.gather(self.db)
        for ledger, waited in (("L", 3000), ("M", 8000)):
            rows = seed.gather(self.db, ledger)["rows"]
            self.assertEqual(len(rows), 1, f"{ledger} read another ledger's question")
            self.assertEqual(rows[0]["wait_ms"], waited, f"{ledger} took another ledger's answer")
        with self.assertRaises(seed.LedgerScopeError):
            seed.gather(self.db, "N")


class Endings(unittest.TestCase):
    """An attempt that ended while its asker waited is called by the
    ledger's record of the ending — never guessed to be a stop."""

    def setUp(self):
        self.raw = tempfile.TemporaryDirectory()
        self.db = Path(self.raw.name) / "authority.sqlite"
        db = sqlite3.connect(self.db)
        db.executescript(SCHEMA)
        rows = Rows(db)
        rows.worker("w-9", "dp-9")
        rows.dispatch("dp-9", "t-9", "w-9", 100)
        # Stopped, and the stop is the task's latest record.
        rows.worker("w-1", None)
        rows.dispatch("dp-1", "t-1", "w-1", 100, 3000)
        rows.task("t-1", json.dumps({"outcome": "stopped", "reason": "the prose nobody copies"}))
        # Ended with no reason the ledger kept.
        rows.worker("w-3", None)
        rows.dispatch("dp-3", "t-3", "w-3", 100, 3000)
        rows.task("t-3", "")
        # Ended, then the task was carried again and THAT attempt was abandoned:
        # the record speaks for the later attempt, not this one.
        rows.worker("w-4", None)
        rows.dispatch("dp-4", "t-4", "w-4", 100, 3000)
        rows.worker("w-5", None)
        rows.dispatch("dp-5", "t-4", "w-5", 3500, 3600)
        rows.task("t-4", json.dumps({"outcome": "abandoned"}))
        for question, receiver in (("m-1", "w-1"), ("m-3", "w-3"), ("m-4", "w-4")):
            rows.message(question, "worker:w-9", f"worker:{receiver}", "question", "which?", dispatch="dp-9", created=1000)
        rows.message("m-99", "ledger", "run:run-1", "status", "{}", created=10_000)
        db.commit()
        db.close()

    def tearDown(self):
        self.raw.cleanup()

    def test_an_ending_is_the_ledgers_record_and_never_a_guess(self):
        rows = {row["question"]: row for row in seed.gather(self.db)["rows"]}
        endings = {question: rows[seed.hashed(question)]["receiver_ended"]["ending"] for question in ("m-1", "m-3", "m-4")}
        self.assertEqual(
            endings,
            {"m-1": "stopped", "m-3": "unexplained", "m-4": "unexplained"},
            "an ending with no record of its own was called a stop",
        )
        self.assertNotIn("the prose nobody copies", json.dumps(rows))


class EndingsByAttempt(unittest.TestCase):
    """An ending is read off the rows that carry the attempt's own key
    (astra, t-6740 r3): a death or a report of ANOTHER attempt of the same
    worker is not this attempt's ending, however close in time, and a row
    that names no attempt at all is nobody's."""

    def setUp(self):
        self.raw = tempfile.TemporaryDirectory()
        self.db = Path(self.raw.name) / "authority.sqlite"
        db = sqlite3.connect(self.db)
        db.executescript(SCHEMA)
        rows = Rows(db)
        # The asker carries an attempt that stays open: every question is censored.
        rows.worker("w-9", "dp-9")
        rows.dispatch("dp-9", "t-9", "w-9", 100)
        # D1 reports done at 3000; the same worker's D2 begins at 3500 and dies
        # at 5000 while the question put during D1 still waits.
        rows.worker("w-1", None)
        rows.dispatch("dp-11", "t-11", "w-1", 100, 3000)
        rows.message("m-10", "worker:w-1", "run:run-1", "worker_done", '{"ok":true}',
                     task="t-11", dispatch="dp-11", created=3000)
        rows.dispatch("dp-12", "t-12", "w-1", 3500, 5000)
        rows.message("m-11", "ledger", "run:run-1", "worker_died",
                     json.dumps({"workerId": "w-1", "dispatchId": "dp-12"}),
                     task="t-12", dispatch="dp-12", created=5000)
        # D1's own death.
        rows.worker("w-2", None)
        rows.dispatch("dp-2", "t-2", "w-2", 100, 3000)
        rows.message("m-20", "ledger", "run:run-1", "worker_died",
                     json.dumps({"workerId": "w-2", "dispatchId": "dp-2"}),
                     task="t-2", dispatch="dp-2", created=3000)
        # A death that names no attempt, and a report that names none.
        rows.worker("w-3", None)
        rows.dispatch("dp-3", "t-3", "w-3", 100, 3000)
        rows.message("m-30", "ledger", "run:run-1", "worker_died", json.dumps({"workerId": "w-3"}), created=3000)
        rows.worker("w-4", None)
        rows.dispatch("dp-4", "t-4", "w-4", 100, 3000)
        rows.message("m-40", "worker:w-4", "run:run-1", "worker_done", '{"ok":true}', created=3000)
        for question, receiver in (("m-1", "w-1"), ("m-2", "w-2"), ("m-3", "w-3"), ("m-4", "w-4")):
            rows.message(question, "worker:w-9", f"worker:{receiver}", "question", "which?", dispatch="dp-9", created=1000)
        rows.message("m-99", "ledger", "run:run-1", "status", "{}", created=10_000)
        db.commit()
        db.close()

    def tearDown(self):
        self.raw.cleanup()

    def test_an_ending_is_read_off_its_own_attempts_rows(self):
        rows = {row["question"]: row for row in seed.gather(self.db)["rows"]}
        endings = {
            question: rows[seed.hashed(question)]["receiver_ended"]["ending"]
            for question in ("m-1", "m-2", "m-3", "m-4")
        }
        self.assertEqual(
            endings,
            {"m-1": "done", "m-2": "died", "m-3": "unexplained", "m-4": "unexplained"},
            "an ending was read off another attempt's row, or off a row naming none",
        )


class DeclineNews(unittest.TestCase):
    """A classifier decline and a model switch are the ledger's own news
    (t-7153): neither is a question's answer, and neither ends the attempt
    it is filed under — a declined worker ends by the stop its coordinator
    or the handover walk writes into the task."""

    def setUp(self):
        self.raw = tempfile.TemporaryDirectory()
        self.db = Path(self.raw.name) / "authority.sqlite"
        db = sqlite3.connect(self.db)
        db.executescript(SCHEMA)
        rows = Rows(db)
        rows.worker("w-9", "dp-9")
        rows.dispatch("dp-9", "t-9", "w-9", 100)
        news = ("classifier_declined", "model_deviated")
        # Declined and switched, then stopped by the handover walk.
        rows.worker("w-1", None)
        rows.dispatch("dp-1", "t-1", "w-1", 100, 3000)
        rows.task("t-1", json.dumps({"outcome": "stopped"}))
        # Declined and switched, then ended with no reason the ledger kept.
        rows.worker("w-2", None)
        rows.dispatch("dp-2", "t-2", "w-2", 100, 3000)
        rows.task("t-2", "")
        for question, receiver in (("m-1", "w-1"), ("m-2", "w-2")):
            rows.message(question, "worker:w-9", f"worker:{receiver}", "question", "which?", dispatch="dp-9", created=1000)
            for at, kind in enumerate(news):
                rows.message(f"{question}-{kind}", "ledger", "run:run-1", kind,
                             json.dumps({"workerId": receiver, "dispatchId": f"dp-{receiver[-1]}"}),
                             task=f"t-{receiver[-1]}", dispatch=f"dp-{receiver[-1]}", created=1500 + at)
                # The same kinds threaded on the question under the receiver's name.
                rows.message(f"{question}-{kind}-thread", f"worker:{receiver}", "worker:w-9", kind, "{}",
                             thread=question, created=2000 + at)
        rows.message("m-99", "ledger", "run:run-1", "status", "{}", created=10_000)
        db.commit()
        db.close()

    def tearDown(self):
        self.raw.cleanup()

    def test_a_decline_and_a_switch_are_news_and_never_an_answer_or_an_ending(self):
        rows = {row["question"]: row for row in seed.gather(self.db)["rows"]}
        for question in ("m-1", "m-2"):
            row = rows[seed.hashed(question)]
            self.assertEqual(row["outcome"], "censored", f"{question}: the ledger's news was taken as the answer")
            self.assertTrue(row["misread_answer"], f"{question}: the older reading would have taken it")
        endings = {question: rows[seed.hashed(question)]["receiver_ended"]["ending"] for question in ("m-1", "m-2")}
        self.assertEqual(endings, {"m-1": "stopped", "m-2": "unexplained"}, "a decline or a switch was read as an ending")


class Notices(unittest.TestCase):
    """The rule's own lines are read — for every receiver, while the asker
    waits, and without the advice sentence."""

    def setUp(self):
        self.raw = tempfile.TemporaryDirectory()
        self.db = Path(self.raw.name) / "authority.sqlite"
        db = sqlite3.connect(self.db)
        db.executescript(SCHEMA)
        rows = Rows(db)
        rows.worker("w-2", "dp-2")
        rows.dispatch("dp-2", "t-2", "w-2", 100)
        # A question to the run, and the coordinator's seat seen resting.
        rows.message("m-1", "worker:w-2", "run:run-1", "question", "which?", dispatch="dp-2", created=1000)
        rows.message("m-2", "ledger", "worker:w-2", "status", json.dumps({
            "questionId": "m-1", "receiver": "run:run-1", "reason": "turn_ended", "final": False,
            "advice": None, "factMs": 1500, "observedAtMs": 1510,
        }), thread="m-1", created=1510)
        # A question to a worker that is stopped: the final word, with its advice.
        rows.worker("w-1", None)
        rows.message("m-3", "worker:w-2", "worker:w-1", "question", "why?", dispatch="dp-2", created=2000)
        rows.message("m-4", "ledger", "worker:w-2", "status", json.dumps({
            "questionId": "m-3", "receiver": "worker:w-1", "reason": "cancelled", "final": True,
            "advice": ADVICE, "factMs": 2500, "observedAtMs": 2500,
        }), thread="m-3", created=2500)
        # The answer to m-1, and a line after it: not a line of that wait.
        rows.message("m-5", "run:run-1", "worker:w-2", "question", "main", thread="m-1", created=3000)
        rows.message("m-6", "ledger", "worker:w-2", "status", json.dumps({
            "questionId": "m-1", "receiver": "run:run-1", "reason": "turn_ended", "final": False,
            "factMs": 3100, "observedAtMs": 3110,
        }), thread="m-1", created=3110)
        db.commit()
        db.close()

    def tearDown(self):
        self.raw.cleanup()

    def test_the_rules_lines_are_read_while_the_asker_waits(self):
        seeded = seed.gather(self.db)
        rows = {row["question"]: row for row in seeded["rows"]}
        to_the_run = rows[seed.hashed("m-1")]
        self.assertEqual(to_the_run["receiver_facts"], "unobservable", "the baseline stays the baseline")
        self.assertEqual(
            to_the_run["notices"],
            [{"reason": "turn_ended", "final": False, "fact_ms": 1500, "learned_ms": 1510}],
        )
        to_the_worker = rows[seed.hashed("m-3")]
        self.assertEqual(
            to_the_worker["notices"],
            [{"reason": "cancelled", "final": True, "fact_ms": 2500, "learned_ms": 2500}],
        )
        self.assertNotIn(ADVICE, json.dumps(seeded))
        self.assertTrue(any("with a final word while waiting: 1" in line for line in seed.summarize(seeded)))


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

    def test_the_ending_outcomes_match_the_rust_source(self):
        ending = re.search(r"impl Ending \{.*?const fn as_str\(self\) -> &'static str \{\s*match self \{(.*?)\}", CORE, re.S)
        self.assertIsNotNone(ending)
        self.assertEqual(tuple(re.findall(r"=> \"([a-z_]+)\"", ending.group(1))), seed.ENDING_OUTCOMES)


if __name__ == "__main__":
    unittest.main()
