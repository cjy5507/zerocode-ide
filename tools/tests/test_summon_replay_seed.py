#!/usr/bin/env python3
"""A historical replay sees only dispatch facts available at its row."""
import importlib.util
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('summon_seed', REPO / 'tools/summon-replay/seed.py')
seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(seed)


class HistoricalSeed(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.store = self.root / 'authority.sqlite'
        with closing(sqlite3.connect(self.store)) as db:
            db.executescript('''
                CREATE TABLE ledger_workers (ledger_id TEXT, run TEXT, id TEXT, agent TEXT, started_ms INTEGER);
                CREATE TABLE ledger_dispatches (ledger_id TEXT, run TEXT, id TEXT, worker TEXT, task TEXT, started_ms INTEGER, ended_ms INTEGER, succeeded INTEGER);
                CREATE TABLE ledger_tasks (ledger_id TEXT, run TEXT, id TEXT, title TEXT, spec TEXT, failures INTEGER, created_ms INTEGER);
                INSERT INTO ledger_workers VALUES ('ledger', 'run', 'worker', 'agent', 10);
                INSERT INTO ledger_tasks VALUES ('ledger', 'run', 'task', 'earlier task', 'earlier work', 3, 1);
                INSERT INTO ledger_tasks VALUES ('ledger', 'run', 'later', 'later task', 'later work', 0, 70);
                INSERT INTO ledger_dispatches VALUES ('ledger', 'run', 'first', 'worker', 'task', 20, 100, 1);
                INSERT INTO ledger_dispatches VALUES ('ledger', 'run', 'later', 'worker', 'later', 80, 120, 1);
            ''')
        self.row = dict(at=50, run='run', task='task', agent='agent', agreed=True,
                        options=['agent', 'another'], carriesATask=True)

    def replay(self):
        ledger = self.root / 'summon.jsonl'
        ledger.write_text(json.dumps(self.row) + '\n')
        output = self.root / 'seed.json'
        with patch.object(sys, 'argv', ['seed', '--ledger', str(ledger), '--store', str(self.store), '--out', str(output)]), patch.object(seed, 'gauges', return_value={}):
            self.assertEqual(seed.main(), 0)
        return json.loads(output.read_text())

    def historical_carried(self, output):
        return output['replays'][0].get('carried', output.get('carried', []))

    def test_an_open_attempt_does_not_borrow_its_future_outcome(self):
        carried = self.historical_carried(self.replay())
        self.assertEqual(len(carried), 1)
        self.assertIsNone(carried[0]['endedMs'])
        self.assertIsNone(carried[0]['succeeded'])

    def test_a_reused_worker_does_not_quote_its_future_task(self):
        carried = self.historical_carried(self.replay())
        self.assertEqual(carried[0]['title'], 'earlier task')

    def test_attempts_and_failures_are_read_at_the_row(self):
        row = self.replay()['replays'][0]
        self.assertEqual((row['attempts'], row['failures']), (1, 0))

    def test_committed_wal_rows_are_in_the_readonly_snapshot(self):
        db = sqlite3.connect(self.store)
        self.addCleanup(db.close)
        db.execute('PRAGMA journal_mode=WAL')
        db.execute("INSERT INTO ledger_workers VALUES ('ledger', 'run', 'second', 'another', 15)")
        db.commit()
        self.assertEqual(len(self.historical_carried(self.replay())), 2)


if __name__ == '__main__':
    unittest.main()
