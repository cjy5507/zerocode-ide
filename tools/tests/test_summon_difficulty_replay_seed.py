#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import sqlite3
import unittest

spec = importlib.util.spec_from_file_location('difficulty_seed', Path(__file__).parents[1] / 'summon-difficulty-replay' / 'seed.py')
seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(seed)


class SeedTest(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:')
        self.db.row_factory = sqlite3.Row
        self.db.executescript('''
        CREATE TABLE ledger_tasks(ledger_id,run,id,title,spec,result,result_author);
        CREATE TABLE ledger_dispatches(ledger_id,run,id,task,worker,started_ms,ended_ms,succeeded,retry_of);
        CREATE TABLE ledger_workers(ledger_id,run,id,started_ms,agent,effort);
        CREATE TABLE ledger_messages(ledger_id,run,kind,sender,body,payload,dispatch,created_ms);
        INSERT INTO ledger_tasks VALUES('l','r','t','translate','labels','{"verified":true,"merged":true}','{"kind":"coordinator"}');
        INSERT INTO ledger_workers VALUES('l','r','w1',1,'claude','max'),('l','r','w2',4,'claude','high');
        INSERT INTO ledger_dispatches VALUES('l','r','d1','t','w1',1,3,0,NULL),('l','r','d2','t','w2',4,7,1,'d1');
        INSERT INTO ledger_messages VALUES('l','r','worker_done','worker:w1','{"ok":false,"summary":"no receipt"}','','d1',3),
        ('l','r','worker_done','worker:w2','{"ok":true,"head":"abc1234"}','','d2',7);
        ''')

    def tearDown(self):
        self.db.close()

    def test_retry_counts_and_receipts_are_observations_not_difficulty_labels(self):
        rows = seed.extract(self.db, 'r')
        self.assertEqual([(r['attempt'], r['failures'], r['retryOf']) for r in rows], [(0, 0, False), (1, 1, True)])
        self.assertEqual([(r['reworkRounds'], r['retryCount']) for r in rows], [(1, 1), (1, 1)])
        self.assertEqual([r['doneWithReceipts'] for r in rows], [False, True])
        self.assertTrue(all(r['verified'] and r['merged'] for r in rows))
        self.assertTrue(all('agreed' not in r and 'tokens' not in r for r in rows))

    def test_a_later_dispatch_on_the_same_worker_cannot_lend_its_receipt(self):
        self.db.executescript('''
        DELETE FROM ledger_messages;
        INSERT INTO ledger_dispatches VALUES('l','r','d3','t','w1',9,10,1,'d2');
        INSERT INTO ledger_messages VALUES('l','r','worker_done','worker:w1','{"head":"def5678"}','','d3',10);
        UPDATE ledger_tasks SET result_author=NULL;
        ''')
        rows = seed.extract(self.db, 'r')
        self.assertFalse(rows[0]['doneWithReceipts'])
        self.assertFalse(rows[0]['workerDone'])
        self.assertFalse(rows[0]['verified'])
        self.assertEqual(rows[0]['attempt'], 0)
        self.assertEqual(rows[0]['reworkRounds'], 2)


if __name__ == '__main__':
    unittest.main()
