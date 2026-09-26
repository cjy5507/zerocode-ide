"""RTS stimulus and accounting; no desktop input is posted by these tests."""
import copy
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

import fixture_reflex as reflex
import fixture_rts as rts
import tally


class RtsTests(unittest.TestCase):
    def setUp(self):
        self.values = tally.table()

    def test_seed_and_supply_alone_determine_the_round(self):
        first = rts.schedule(11, self.values)
        self.assertEqual(first, rts.schedule(11, self.values))
        self.assertNotEqual(first, rts.schedule(12, self.values))
        self.assertEqual({row['kind'] for row in first['targets']}, set(self.values['rts_inputs']))
        self.assertGreaterEqual(sum(len(row['actions']) for row in first['targets']), self.values['rts_floor']['apm'])
        self.assertTrue(all(a['expireMs'] <= b['appearMs'] for a, b in zip(first['targets'], first['targets'][1:])))

    def test_stress_draws_every_supply_step_with_no_task_crossing_its_step(self):
        drawn = rts.schedule(11, self.values, stress=True)
        self.assertEqual([step['apm'] for step in drawn['steps']], self.values['rts_supply']['steps_apm'])
        for target in drawn['targets']:
            step = drawn['steps'][target['step']]
            self.assertLessEqual(step['startMs'], target['appearMs'])
            self.assertLessEqual(target['expireMs'], step['endMs'])

    def test_plan_stays_inside_the_existing_expansion_budget_and_scope(self):
        geometry = {'display': {'x': 0, 'y': 0, 'width': 1512, 'height': 982},
                    'window': {'x': 396, 'y': 271, 'width': 720, 'height': 440}}
        plan = rts.plan(geometry, self.values, 'dev.zerocode.bench.rts', reflex.contract(), table_limits=reflex.limits())
        macros = {row['id']: row for row in plan['macros']}
        self.assertLessEqual(sum(row['max_fires'] * len(macros[row['macro_id']]['actions']) for row in plan['rules']),
                             reflex.limits()['max_expanded_actions'])
        self.assertEqual(plan['scope']['target'], 'dev.zerocode.bench.rts')
        self.assertEqual({action['kind'] for macro in macros.values() for action in macro['actions']}, {'key', 'click', 'drag'})
        group = next(m for m in plan['macros'] if m['id'] == 'act_group')
        self.assertEqual([a['kind'] for a in group['actions']], ['drag', 'key'])
        golden = json.loads((reflex.FIXTURES / 'reflex-contract/valid_rts_plan.json').read_text())['plan']
        golden['plan_hash'] = ''
        self.assertEqual(plan, golden, 'both native contracts validate these exact plan fields')

    def record(self):
        schedule = rts.schedule(11, self.values)
        t0 = 5_000_000_000_000
        events, frames = [], []
        for target in schedule['targets']:
            shown = t0 + int(target['appearMs'] * 1_000_000)
            frames.append({'ns': shown, 'shown': [target['id']]})
            for index, action in enumerate(target['actions']):
                events.append({'sourcePid': 4242, 'rxNs': shown + (index + 1) * 30_000_000,
                               'judged': {'hit': action['id'], 'input': action['input']}})
        return {'schedule': schedule, 'run': {'t0Ns': t0, 'verdictNs': t0 + 60_000_000_000, 'stoppedBy': None},
                'frames': frames, 'events': events, 'geometry': {'helperPid': 4242},
                'fixture': {'held': [], 'misses': {}, 'becameActive': False},
                'ended': {'status': {'reason': 'deadline', 'othersHeard': 0, 'monitor': 'hearing'},
                          'report': {'verified': True, 'writeFailures': 0}}, 'receipts': []}

    def test_oracle_uses_received_inputs_and_cannot_pass_an_empty_run(self):
        record = self.record()
        self.assertTrue(rts.judged(record, self.values, reflex.limits())['passed'])
        record['events'] = []
        result = rts.judged(record, self.values, reflex.limits())
        self.assertFalse(result['passed'])
        self.assertEqual(result['oracle'], 0)

    def test_duplicate_hit_foreign_input_and_stuck_modifier_fail(self):
        for change in ('duplicate', 'foreign', 'held', 'wrong'):
            record = self.record()
            if change == 'duplicate':
                record['events'].append(copy.deepcopy(record['events'][0]))
            elif change == 'foreign':
                record['events'][0]['sourcePid'] = 111
            elif change == 'held':
                record['fixture']['held'] = ['ctrl']
            else:
                record['events'].append({'sourcePid': 4242, 'judged': {'miss': 'modifier'}})
            self.assertFalse(rts.judged(record, self.values, reflex.limits())['passed'], change)

    def test_receipts_cannot_turn_a_miss_into_a_hit(self):
        record = self.record()
        record['events'].pop()
        before = rts.judged(record, self.values, reflex.limits())
        record['receipts'] = [{'outcome': 'done', 'kind': 'key'}] * 1000
        self.assertEqual(rts.judged(record, self.values, reflex.limits())['oracle'], before['oracle'])

    @unittest.skipUnless(sys.platform == 'darwin', 'AppKit oracle runs on macOS')
    def test_native_oracle_receives_each_input_and_rejects_wrong_modifiers_and_duplicates(self):
        with tempfile.TemporaryDirectory() as folder:
            path = pathlib.Path(folder)
            binary = path / 'rts-fixture'
            subprocess.run(['swiftc', '-O', '-swift-version', '6', '-warnings-as-errors', '-parse-as-library',
                            str(rts.HERE / 'RtsFixture.swift'), '-o', str(binary)], check=True, capture_output=True)
            round_file = path / 'round.json'
            round_file.write_text(json.dumps(rts.the_round('owner', 11, self.values, reflex.limits())))
            run = subprocess.run([str(binary), '--self-test', str(round_file), str(path)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            state = json.loads((path / 'fixture.json').read_text())
            self.assertEqual(state['misses'], {'click': 1, 'duplicate': 1, 'unpaired_release': 1})
            self.assertEqual(state['held'], [])


if __name__ == '__main__':
    unittest.main()
