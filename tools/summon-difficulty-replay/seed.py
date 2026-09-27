#!/usr/bin/env python3
"""Extract summons facts read-only. Task words/review are current reconstructions.

No secrets, session paths or checkout paths are extracted. The output is a
private measurement input, never a fixture to commit. Scoring lives in Rust.
"""
import argparse
from collections import defaultdict
from contextlib import closing
import json
from pathlib import Path
import sqlite3


def extract(db, run):
    tasks = {(r['ledger_id'], r['run'], r['id']): dict(r) for r in db.execute('SELECT * FROM ledger_tasks WHERE run = ?', (run,))}
    dispatches = [dict(r) for r in db.execute('SELECT * FROM ledger_dispatches WHERE run = ? ORDER BY started_ms, id', (run,))]
    by_task = defaultdict(list)
    by_worker = defaultdict(list)
    for d in dispatches:
        by_task[(d['ledger_id'], d['run'], d['task'])].append(d)
        by_worker[(d['ledger_id'], d['run'], d['worker'])].append(d)
    done = defaultdict(list)
    for m in db.execute("SELECT * FROM ledger_messages WHERE run = ? AND kind = 'worker_done'", (run,)):
        done[(m['ledger_id'], m['run'], m['sender'])].append(dict(m))
    rows = []
    for w in db.execute('SELECT * FROM ledger_workers WHERE run = ? ORDER BY started_ms, id', (run,)):
        held = by_worker[(w['ledger_id'], w['run'], w['id'])]
        if not held:
            continue
        # First attempt on this worker, rather than a later reassignment.
        d = held[0]
        key = (d['ledger_id'], d['run'], d['task'])
        t = tasks.get(key)
        if not t:
            continue
        all_attempts = by_task[key]
        prior = [p for p in all_attempts if p['started_ms'] < d['started_ms']]
        failures = 0
        for p in reversed(prior):
            if p['ended_ms'] is None or p['ended_ms'] > d['started_ms'] or p['succeeded'] is None:
                continue
            if p['succeeded']:
                break
            failures += 1
        messages = done[(w['ledger_id'], w['run'], 'worker:' + w['id'])]
        messages = [m for m in messages if m['dispatch'] == d['id'] or (m['dispatch'] is None and d['started_ms'] <= m['created_ms'] and (d['ended_ms'] is None or m['created_ms'] <= d['ended_ms']))]
        receipt = False
        for m in messages:
            try:
                body = json.loads(m['body'])
                payload = json.loads(m['payload'] or '{}')
                receipt |= bool(isinstance(body, dict) and body.get('head')) or bool(isinstance(payload, dict) and payload.get('reportPath'))
            except (ValueError, TypeError):
                continue
        try:
            result = json.loads(t['result'] or '{}')
            author = json.loads(t['result_author'] or '{}')
        except (ValueError, TypeError):
            result, author = {}, {}
        result = result if isinstance(result, dict) else {}
        coordinator = isinstance(author, dict) and author.get('kind') == 'coordinator'
        rows.append(dict(run=w['run'], worker=w['id'], task=d['task'], dispatch=d['id'],
                         agent=w['agent'], effort=w['effort'], title=t['title'], spec=t['spec'],
                         attempt=len(prior), failures=failures, retryOf=d['retry_of'] is not None,
                         reworkRounds=max(0, len(all_attempts)-1),
                         retryCount=sum(p['retry_of'] is not None for p in all_attempts),
                         workerDone=bool(messages), doneWithReceipts=receipt,
                         verified=coordinator and result.get('verified') is True,
                         merged=coordinator and bool(result.get('mergeHead') or result.get('merged') is True),
                         ended=d['ended_ms'] is not None))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--store', required=True, type=Path)
    parser.add_argument('--run', required=True)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    with closing(sqlite3.connect(args.store.resolve().as_uri() + '?mode=ro', uri=True)) as db:
        db.row_factory = sqlite3.Row
        db.execute('BEGIN')
        rows = extract(db, args.run)
    args.out.write_text(json.dumps(rows, ensure_ascii=False, indent=2) + '\n')
    print(f'{len(rows)} summonses extracted read-only')


if __name__ == '__main__':
    main()
