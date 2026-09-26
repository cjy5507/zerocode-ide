#!/usr/bin/env python3
"""Contract for `tools/mail-triage-replay/seed.py` — it copies one ledger's
coordinator mail and the receipts its acts were filed under, in the shapes the
core reads them in, and nothing else: no word of a letter, no prose of an
answer, no retry name, no session in the clear; it reads one ledger at a time
and moves nothing.

Run: python3 tools/tests/test_mail_triage_replay_seed.py   (stdlib only)
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
_spec = importlib.util.spec_from_file_location(
    "mail_triage_replay_seed", REPO / "tools" / "mail-triage-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)

CORE = (REPO / "crates" / "zerocode-core" / "src" / "orchestration.rs").read_text()
REPLAY = (REPO / "crates" / "zerocode-core" / "src" / "mail_triage" / "tests.rs").read_text()

SCHEMA = """
CREATE TABLE ledger_runs (ledger_id TEXT, ordinal INTEGER, id TEXT, name TEXT, created_ms INTEGER,
  summary TEXT, coordinator TEXT, handover TEXT);
CREATE TABLE ledger_messages (ledger_id TEXT, ordinal INTEGER, run TEXT, id TEXT, sender TEXT,
  recipient TEXT, kind TEXT, body TEXT, subject TEXT DEFAULT '', priority TEXT DEFAULT 'normal',
  payload TEXT DEFAULT '', thread TEXT, task TEXT, dispatch TEXT, created_ms INTEGER, author_seat TEXT);
CREATE TABLE ledger_dispatches (ledger_id TEXT, ordinal INTEGER, run TEXT, id TEXT, task TEXT,
  worker TEXT, started_ms INTEGER, ended_ms INTEGER, succeeded INTEGER, retry_of TEXT, remote TEXT, source TEXT);
CREATE TABLE ledger_inboxes (ledger_id TEXT, ordinal INTEGER, run TEXT, address TEXT,
  open_delivery TEXT, open_holder TEXT, open_opened_ms INTEGER);
CREATE TABLE ledger_inbox_pending (ledger_id TEXT, inbox_ordinal INTEGER, ordinal INTEGER, message TEXT);
CREATE TABLE ledger_inbox_open_messages (ledger_id TEXT, inbox_ordinal INTEGER, ordinal INTEGER, message TEXT);
CREATE TABLE ledger_acked (ledger_id TEXT, ordinal INTEGER, run TEXT, address TEXT, delivery TEXT,
  holds_messages INTEGER, seq INTEGER, current INTEGER);
CREATE TABLE ledger_acked_messages (ledger_id TEXT, acked_ordinal INTEGER, ordinal INTEGER, message TEXT);
CREATE TABLE ledger_served (ledger_id TEXT, ordinal INTEGER, caller TEXT, request TEXT, fingerprint TEXT,
  renderer TEXT, inline TEXT, check_run TEXT, check_address TEXT, check_delivery TEXT, filed_ms INTEGER,
  expired INTEGER DEFAULT 0, verb TEXT);
CREATE TABLE ledger_served_messages (ledger_id TEXT, served_ordinal INTEGER, ordinal INTEGER, message TEXT);
"""

SECRET = "the key is sk-live-do-not-copy"
TITLE = "a title a person typed"
ANSWER_PROSE = "the answer a worker wrote"
COORDINATOR = "actor-v1:" + "c" * 64
WORKER = "actor-v1:" + "d" * 64
RETRY_NAME = "merge-t-1-after-review"


def a_store(path: Path, ledgers=("L",)) -> None:
    db = sqlite3.connect(path)
    db.executescript(SCHEMA)
    for ledger in ledgers:
        seat = json.dumps({"seat": "team-9/%1", "actor": COORDINATOR, "generation": 3, "since_ms": 1})
        db.execute("INSERT INTO ledger_runs VALUES (?,?,?,?,?,?,?,?)", (ledger, 0, "run-1", "a run", 1, None, seat, None))
        db.execute("INSERT INTO ledger_runs VALUES (?,?,?,?,?,?,?,?)", (ledger, 1, "run-2", "quiet", 1, None, None, None))
        messages = [
            ("m-1", "worker:w-1", "run:run-1", "question", SECRET, "normal", None, "t-1", "dp-1", 100),
            ("m-2", "run:run-1", "worker:w-1", "question", SECRET, "normal", "m-1", None, None, 200),
            ("m-3", "ledger", "run:run-1", "went_quiet", json.dumps({"workerId": "w-1"}), "high", None, "t-1", "dp-1", 300),
            ("m-4", "worker:w-1", "run:run-1", "worker_done", SECRET, "normal", None, "t-1", "dp-1", 50),
        ]
        for at, row in enumerate(messages):
            db.execute(
                "INSERT INTO ledger_messages (ledger_id, ordinal, run, id, sender, recipient, kind, body, subject, "
                "priority, payload, thread, task, dispatch, created_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (ledger, at, "run-1", row[0], row[1], row[2], row[3], row[4], SECRET, row[5], SECRET, row[6], row[7], row[8], row[9]),
            )
        db.execute("INSERT INTO ledger_dispatches VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                   (ledger, 0, "run-1", "dp-1", "t-1", "w-1", 10, 60, 1, None, None, None))
        db.execute("INSERT INTO ledger_dispatches VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                   (ledger, 1, "run-1", "dp-2", "t-1", "w-2", 70, None, None, "dp-1", None, None))
        db.execute("INSERT INTO ledger_inboxes VALUES (?,?,?,?,?,?,?)", (ledger, 0, "run-1", "run:run-1", "d-9", "team-9/%1", 310))
        db.execute("INSERT INTO ledger_inboxes VALUES (?,?,?,?,?,?,?)", (ledger, 1, "run-1", "worker:w-1", None, None, None))
        db.execute("INSERT INTO ledger_inbox_open_messages VALUES (?,?,?,?)", (ledger, 0, 0, "m-3"))
        db.execute("INSERT INTO ledger_inbox_pending VALUES (?,?,?,?)", (ledger, 1, 0, "m-2"))
        db.execute("INSERT INTO ledger_acked VALUES (?,?,?,?,?,?,?,?)", (ledger, 0, "run-1", "run:run-1", "d-5", 1, 1, 0))
        db.execute("INSERT INTO ledger_acked VALUES (?,?,?,?,?,?,?,?)", (ledger, 1, "run-1", "run:run-1", "d-7", 1, 2, 1))
        db.execute("INSERT INTO ledger_acked_messages VALUES (?,?,?,?)", (ledger, 0, 0, "m-4"))
        db.execute("INSERT INTO ledger_acked_messages VALUES (?,?,?,?)", (ledger, 1, 0, "m-1"))
        receipts = [
            (COORDINATOR, RETRY_NAME, None, json.dumps({"taskId": "t-1", "status": "completed", "author": "coordinator"}),
             None, None, None, 250, "task-update"),
            (COORDINATOR, "r-2", None, json.dumps({"messageId": "m-2"}), None, None, None, 200, None),
            (COORDINATOR, "r-3", None, json.dumps({"mode": "peek", "count": 1, "messages": [{"body": SECRET}]}),
             None, None, None, 150, None),
            (COORDINATOR, "r-4", "check-v1", None, "run-1", "run:run-1", "d-7", 120, "check"),
            (WORKER, "r-5", None, json.dumps({"questionId": "m-1", "answered": True,
                                               "answer": {"messageId": "m-2", "body": ANSWER_PROSE}}), None, None, None, 210, "ask"),
            (COORDINATOR, "r-6", None, json.dumps({"taskId": "t-2", "title": TITLE, "status": "pending"}),
             None, None, None, 260, "task-create"),
            (COORDINATOR, "r-7", None, "printed words, not an object", None, None, None, 270, None),
        ]
        for at, row in enumerate(receipts):
            db.execute(
                "INSERT INTO ledger_served (ledger_id, ordinal, caller, request, fingerprint, renderer, inline, "
                "check_run, check_address, check_delivery, filed_ms, verb) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                (ledger, at, row[0], row[1], "f" * 64, row[2], row[3], row[4], row[5], row[6], row[7], row[8]),
            )
        db.execute("INSERT INTO ledger_served_messages VALUES (?,?,?,?)", (ledger, 3, 0, "m-1"))
    db.commit()
    db.close()


class Gathering(unittest.TestCase):
    def setUp(self):
        self.raw = tempfile.TemporaryDirectory()
        self.db = Path(self.raw.name) / "authority.sqlite"
        a_store(self.db)

    def tearDown(self):
        self.raw.cleanup()

    def gathered(self, **knobs):
        return seed.gather(self.db, **knobs)

    def run_one(self, gathered):
        return {one["id"]: one for one in gathered["runs"]}["run-1"]

    def test_no_word_of_a_letter_or_an_answer_leaves(self):
        text = json.dumps(self.gathered())
        for secret in (SECRET, TITLE, ANSWER_PROSE, RETRY_NAME, "f" * 64):
            self.assertNotIn(secret, text)
        for message in self.run_one(self.gathered())["messages"]:
            self.assertEqual(message["body"], "", "a letter's words never leave")

    def test_messages_wear_the_shape_the_core_reads(self):
        messages = {one["id"]: one for one in self.run_one(self.gathered())["messages"]}
        self.assertEqual(set(messages), {"m-1", "m-2", "m-3", "m-4"})
        allowed = {"id", "from", "to", "kind", "body", "priority", "thread", "task", "dispatch", "created_ms"}
        for message in messages.values():
            self.assertLessEqual(set(message), allowed)
        self.assertEqual(messages["m-2"]["thread"], "m-1")
        self.assertEqual(messages["m-2"]["from"], "run:run-1")
        self.assertEqual(messages["m-3"]["priority"], "high")
        self.assertNotIn("priority", messages["m-1"], "normal is the core's default and left out")
        self.assertEqual(messages["m-1"]["created_ms"], 100)

    def test_receipts_carry_ids_and_a_looks_own_word_only(self):
        receipts = self.gathered()["receipts"]
        by_filed = {one["filedMs"]: one for one in receipts}
        self.assertEqual(by_filed[250]["answer"], {"taskId": "t-1"})
        self.assertEqual(by_filed[250]["verb"], "task-update")
        self.assertEqual(by_filed[150]["answer"], {"mode": "peek"}, "a look says it was one")
        self.assertIsNone(by_filed[150]["verb"], "an older row's missing verb is left missing")
        self.assertEqual(by_filed[210]["answer"], {"questionId": "m-1"}, "the question, not the answer")
        self.assertEqual(by_filed[260]["answer"], {"taskId": "t-2"})
        self.assertEqual(by_filed[270]["answer"], {})
        self.assertEqual(
            by_filed[120]["check"],
            {"address": "run:run-1", "delivery": "d-7", "messages": ["m-1"]},
        )
        self.assertIsNone(by_filed[250]["check"])
        self.assertEqual(set(by_filed[250]), {"caller", "verb", "filedMs", "answer", "check"})

    def test_sessions_are_hashed_alike_where_they_meet(self):
        gathered = self.gathered()
        text = json.dumps(gathered)
        self.assertNotIn(COORDINATOR, text)
        self.assertNotIn(WORKER, text)
        seat = self.run_one(gathered)["seatActor"]
        callers = {one["filedMs"]: one["caller"] for one in gathered["receipts"]}
        self.assertEqual(callers[250], seat, "the seat's session and its receipts meet")
        self.assertNotEqual(callers[210], seat)
        quiet = {one["id"]: one for one in gathered["runs"]}["run-2"]
        self.assertIsNone(quiet["seatActor"])

    def test_the_inbox_is_copied_as_it_stands(self):
        run = self.run_one(self.gathered())
        self.assertEqual(run["address"], "run:run-1")
        self.assertEqual(run["pending"], [], "the run's own inbox, not a worker's")
        self.assertEqual(run["open"], {"id": "d-9", "messages": ["m-3"], "opened_ms": 310})
        self.assertEqual(
            run["acked"],
            [{"delivery": "d-5", "messages": ["m-4"]}, {"delivery": "d-7", "messages": ["m-1"]}],
        )
        attempts = {one["id"]: one for one in run["dispatches"]}
        self.assertEqual(attempts["dp-1"]["succeeded"], True)
        self.assertEqual(attempts["dp-2"]["retry_of"], "dp-1")
        self.assertIsNone(attempts["dp-2"]["ended_ms"])

    def test_the_window_opens_where_asked_and_reads_to_the_ledgers_last_word(self):
        gathered = self.gathered(from_ms=90)
        self.assertEqual(gathered["fromMs"], 90)
        self.assertEqual(gathered["readAtMs"], 300, "the newest letter or receipt")
        self.assertEqual(gathered["schema"], seed.SEED_SCHEMA)
        self.assertEqual(gathered["ledgerId"], "L")

    def test_the_store_moves_not_one_byte(self):
        before = self.db.read_bytes()
        self.gathered()
        self.assertEqual(self.db.read_bytes(), before)
        with self.assertRaises(sqlite3.OperationalError):
            seed.open_read_only(self.db).execute("DELETE FROM ledger_runs")

    def test_one_ledger_at_a_time(self):
        two = Path(self.raw.name) / "two.sqlite"
        a_store(two, ledgers=("A", "B"))
        with self.assertRaises(seed.LedgerScopeError):
            seed.gather(two)
        self.assertEqual(seed.gather(two, ledger_id="B")["ledgerId"], "B")
        with self.assertRaises(seed.LedgerScopeError):
            seed.gather(two, ledger_id="C")

    def test_the_seed_is_written_and_summed_in_numbers(self):
        out = Path(self.raw.name) / "seed.json"
        said = seed.main(["--db", str(self.db), "--out", str(out), "--from-ms", "0"])
        self.assertEqual(said, 0)
        written = json.loads(out.read_text())
        self.assertEqual(written["fromMs"], 0)
        self.assertEqual(len(written["runs"]), 2)


class Constants(unittest.TestCase):
    """The seed speaks the core's words, read from the Rust itself."""

    def test_the_schema_is_the_replays(self):
        self.assertIn(f"const SEED_SCHEMA: u32 = {seed.SEED_SCHEMA};", REPLAY)

    def test_the_run_address_and_the_look_key_are_the_cores(self):
        self.assertIn(f'const RUN_ADDRESS_PREFIX: &str = "{seed.RUN_ADDRESS_PREFIX}";', CORE)
        self.assertIn(f'pub const LOOK_MODE_KEY: &str = "{seed.LOOK_MODE_KEY}";', CORE)

    def test_an_id_is_the_ledgers_minted_shape(self):
        minted = set(re.findall(r'self\.mint\("([a-z]+)-"\)', CORE))
        self.assertTrue(minted)
        for prefix in minted:
            self.assertTrue(seed.ID_SHAPE.fullmatch(f"{prefix}-12"), prefix)
        for word in ("completed", "coordinator", "peek", "", "m-", "t-1a"):
            self.assertFalse(seed.ID_SHAPE.fullmatch(word), word)


if __name__ == "__main__":
    unittest.main()
