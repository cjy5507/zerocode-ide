"""Seeded RTS input bench. Stimuli and receipts belong to the fixture, not the hand.

The existing reflex Desk supplies the authenticated helper, idle/stop checks,
supervision and cleanup. This module supplies only its round, plan and oracle.
"""
import argparse
import json
import pathlib
import random
import sys

import fixture_reflex as reflex
import fixture_support
import tally

HERE = pathlib.Path(__file__).resolve().parent
SCENARIO = 'rts-fixture'


def schedule(seed, values, stress=False):
    draw = random.Random(seed)
    supply, inputs = values['rts_supply'], values['rts_inputs']
    run_length = values['reflex_round']['run_s'] * 1000
    length = run_length + values['reflex_safety']['prep_s'] * 1000
    lead = values['reflex_round']['lead_ms']
    rates = supply['steps_apm'] if stress else [supply['apm']]
    # A normal round has stimuli left after an automatic plan's preparation.
    # Stress fits all supply steps inside one hand's minute.
    span = ((run_length if stress else length) - lead) / len(rates)
    steps = [{'apm': rate, 'startMs': lead + n * span, 'endMs': lead + (n + 1) * span}
             for n, rate in enumerate(rates)]
    box = reflex.field(values)
    size, edge = supply['target_pt'], supply['edge_pt']
    targets = []
    for index, step in enumerate(steps):
        at, order = step['startMs'], []
        while at < step['endMs']:
            if not order:
                order = list(inputs)
                draw.shuffle(order)
            kind = order.pop()
            wanted = inputs[kind]
            names = (['drag'] if kind == 'group' else []) + [wanted['input']]
            duration = len(names) * 60_000 / step['apm']
            if at + duration > step['endMs']:
                break
            identity = f't{len(targets) + 1}'
            targets.append({'id': identity, 'kind': kind, 'step': index,
                            'appearMs': at, 'expireMs': at + duration - supply['gap_ms'],
                            'x': draw.randint(box['x'] + edge, box['x'] + box['width'] - edge - size),
                            'y': draw.randint(box['y'] + edge, box['y'] + box['height'] - edge - size),
                            'width': size, 'height': size,
                            'actions': [{'id': f'{identity}:{name}', 'input': name} for name in names]})
            at += duration
    return {'seed': seed, 'lengthMs': length, 'stress': stress, 'steps': steps, 'targets': targets}


def plan(geometry, values, bundle, contract_version, rules=None, table_limits=None):
    display, window = geometry['display'], geometry['window']
    inputs, chosen = values['rts_inputs'], values['reflex_plan']
    x, y = window['x'] - int(display['x']), window['y'] - int(display['y'])
    hud, edge = values['reflex_hud_pt'], values['reflex_target']['edge_pt']
    roi = {'space': 'pixel', 'x': x + edge, 'y': y + hud + edge,
           'width': window['width'] - edge * 2, 'height': window['height'] - hud - edge * 2}
    detectors, macros = [], []
    for number, (kind, wanted) in enumerate(inputs.items(), start=1):
        detectors.append({'id': kind, 'kind': 'color', 'patches': 1, 'roi': roi,
                          'scale': {'numerator': 1, 'denominator': 1},
                          'color': {'space': 'srgb', 'reference_width': int(display['width']),
                                    'reference_height': int(display['height']), 'confirm': chosen['confirm'],
                                    'classes': [row['colour'] for row in inputs.values()],
                                    'ground': values['reflex_palette']['ground'],
                                    'layout': {'kind': 'blobs', 'class': number, 'gate': chosen['gate_pt'],
                                               'max_blobs': chosen['max_blobs'], 'min_samples': chosen['min_samples'],
                                               'step': chosen['step_pt']}}})
        actions = []
        if kind == 'group':
            actions.append({'id': f'drag_{kind}', 'kind': 'drag', 'target': kind,
                            'from': values['rts_plan']['drag_from'], 'to': values['rts_plan']['drag_to']})
        action = {'id': f'press_{kind}', 'target': kind}
        if 'key' in wanted:
            action.update(kind='key', key=wanted['key'])
        else:
            action.update(kind='click')
            if wanted['button'] != 'left':
                action['button'] = wanted['button']
        if wanted['modifiers']:
            action['modifiers'] = wanted['modifiers']
        actions.append(action)
        macros.append({'id': f'act_{kind}', 'repeat': 1, 'actions': actions})
    quota = (table_limits or reflex.limits())['max_expanded_actions'] // sum(len(m['actions']) for m in macros)
    rules = [{'id': f'see_{kind}', 'detector': kind, 'macro_id': f'act_{kind}',
              'predicate': {'op': 'eq', 'value': 1}, 'max_fires': quota, 'cooldown_ms': 0, 'priority': 0}
             for kind in inputs]
    return {'version': contract_version, 'plan_hash': '', 'scope': {'surface': 'macos_desktop', 'target': bundle},
            'detectors': detectors, 'rules': rules, 'macros': macros,
            'pointer': {'curve': 'cosine', 'duration_ms': values['rts_plan']['pointer_ms'], 'instant': False}}


def the_round(owner, seed, values, table_limits, stress=False):
    return {'owner': owner, 'seed': seed, 'canvas': values['reflex_canvas_pt'], 'hud': values['reflex_hud_pt'],
            'ground': values['reflex_palette']['ground'], 'inputs': values['rts_inputs'],
            'drawing': values['rts_supply'], 'framesPerSecond': table_limits['frames_per_second'],
            'schedule': schedule(seed, values, stress)}


def goal(values):
    return values['rts_goal'].format(hud=values['reflex_hud_pt'])


def judged(record, values, table_limits):
    """Only fixture hit ids count. Repeated ids, foreign events and held inputs
    invalidate a run, even if the hand claims every action completed."""
    schedule = record['schedule']
    expected = {action['id']: (target, action['input']) for target in schedule['targets'] for action in target['actions']}
    t0 = record['run']['t0Ns']
    started = record.get('started') or {}
    start = started.get('acceptedNs', t0)
    deadline = started.get('deadlineNs', record['run']['verdictNs'])
    def inside(target, since):
        return t0 + int(target['appearMs'] * 1e6) >= since and t0 + int(target['expireMs'] * 1e6) <= deadline
    due = {identity for identity, (target, _) in expected.items() if inside(target, start)}
    from_goal = {identity for identity, (target, _) in expected.items() if inside(target, t0)}
    shown = {}
    for frame in record['frames']:
        for identity in frame['shown']:
            shown.setdefault(identity, frame['ns'])
    good, wrong, foreign, duplicate = {}, [], 0, 0
    helper = record['geometry']['helperPid']
    for event in record['events']:
        if event.get('sourcePid') != helper:
            foreign += 1
        verdict = event.get('judged') or {}
        if verdict.get('miss'):
            wrong.append(verdict['miss'])
        identity = verdict.get('hit')
        if identity is None:
            continue
        if identity in good:
            duplicate += 1
        elif identity not in expected or verdict.get('input') != expected[identity][1]:
            wrong.append('unknown_hit')
        elif expected[identity][0]['id'] not in shown or event['rxNs'] < shown[expected[identity][0]['id']]:
            wrong.append('unshown_hit')
        else:
            good[identity] = event
    wall = (record['run']['verdictNs'] - record['run']['t0Ns']) / 1e9
    per_input = {}
    for name in sorted({name for _, name in expected.values()}):
        named = [identity for identity, (_, input_name) in expected.items() if input_name == name]
        wanted = [identity for identity in named if identity in due]
        hits = [identity for identity in named if identity in good]
        latencies = [(good[identity]['rxNs'] - shown[expected[identity][0]['id']]) / 1e6 for identity in hits]
        per_input[name] = {'offered': len(wanted), 'hits': len(set(hits) & due), 'actions': len(hits),
                           'apm': len(hits) * 60 / wall if wall > 0 else 0,
                           'reaction_ms': reflex.spread(latencies)}
    steps = []
    for index, step in enumerate(schedule['steps']):
        wanted = [identity for identity, (target, _) in expected.items() if target['step'] == index and identity in due]
        hits = sum(identity in good for identity in wanted)
        steps.append({**step, 'offered': len(wanted), 'hits': hits, 'oracle': hits / len(wanted) if wanted else 0,
                      'achieved_apm': hits * 60_000 / (step['endMs'] - step['startMs'])})
    floor = values['rts_floor']
    ceiling = max((step['apm'] for step in steps if step['oracle'] >= floor['stress_oracle']), default=0)
    latencies = [(event['rxNs'] - shown[expected[identity][0]['id']]) / 1e6 for identity, event in good.items()]
    reaction = reflex.spread(latencies)
    oracle = len(good.keys() & due) / len(due) if due else 0
    apm = len(good) * 60 / wall if wall > 0 else 0
    fixture, ended = record.get('fixture') or {}, record.get('ended') or {}
    status, report = ended.get('status') or {}, ended.get('report') or {}
    autopilot = reflex.autopilot_numbers(record)
    errors = []
    if not started:
        errors.append('start')
    if record['run'].get('autopilot') and (
            not autopilot or not autopilot['plans'] or set(autopilot['sources']) != {reflex.MODEL}):
        errors.append('model_plan')
    if wrong or foreign or duplicate or fixture.get('held'):
        errors.append('input')
    if fixture.get('becameActive'):
        errors.append('focus')
    if record['run'].get('stoppedBy') or status.get('othersHeard') or status.get('monitor') != 'hearing':
        errors.append('interrupted')
    if not report.get('verified') or report.get('writeFailures'):
        errors.append('receipts')
    if wall < values['reflex_floor']['wall_s']:
        errors.append('wall')
    if not schedule['stress']:
        if oracle < values['reflex_floor']['oracle']:
            errors.append('oracle')
        if apm < floor['apm']:
            errors.append('apm')
        if reaction['p50'] is None or reaction['p50'] > floor['reaction_p50_ms']:
            errors.append('reaction')
    elif not ceiling:
        errors.append('ceiling')
    return {'scenario': SCENARIO, 'seed': schedule['seed'], 'passed': not errors, 'errors': errors,
            'wall_s': wall, 'apm': apm, 'oracle': oracle, 'wrong': len(wrong), 'foreign': foreign,
            'oracle_offered': len(due), 'oracle_hits': len(good.keys() & due),
            'goal_oracle': len(good.keys() & from_goal) / len(from_goal) if from_goal else 0,
            'duplicates': duplicate, 'held': fixture.get('held'), 'reaction_ms': reaction,
            'inputs': per_input, 'steps': steps, 'ceiling_apm': ceiling if schedule['stress'] else None,
            'ceiling_capped': schedule['stress'] and ceiling == max(step['apm'] for step in steps),
            'autopilot': autopilot}


class Desk(reflex.Desk):
    def __init__(self, folder, values, table_limits, stress=False):
        super().__init__(folder, values, table_limits)
        self.stress = stress

    def round(self, seed):
        return the_round(self.session['owner'], seed, self.values, self.limits, self.stress)

    def plan(self, geometry, rules):
        return plan(geometry, self.values, self.session['bundle'], reflex.contract(), table_limits=self.limits)

    def result(self, record):
        return judged(record, self.values, self.limits)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'rehearse', 'run', 'judge'])
    parser.add_argument('folder', type=pathlib.Path)
    parser.add_argument('--seed', type=int)
    parser.add_argument('--seconds', type=float)
    parser.add_argument('--stress', action='store_true')
    parser.add_argument('--driver')
    parser.add_argument('--helper-app')
    parser.add_argument('--autopilot', action='store_true')
    parser.add_argument('--generator', choices=['window', reflex.STUB], default='window')
    parser.add_argument('--l1', choices=['auto', 'shadow', 'off'], default='shadow')
    args = parser.parse_args(argv)
    values, limits = tally.table(), reflex.limits()
    if args.command == 'prepare':
        return fixture_support.prepare(args.folder.resolve(), 'RtsFixture', HERE / 'RtsFixture.swift',
                                       'dev.zerocode.bench.rts')
    signals = reflex.Signals().install()
    try:
        if args.command == 'judge':
            result = judged(reflex.load(args.folder), values, limits)
        else:
            desk = Desk(args.folder.resolve(), values, limits, stress=args.stress)
            if args.seed is None:
                parser.error('--seed is required')
            if args.command == 'rehearse':
                result = desk.rehearse(args.seed, args.seconds or values['reflex_round']['run_s'])
            else:
                if not args.driver or not args.helper_app:
                    parser.error('run needs --driver and --helper-app')
                autopilot = {'generator': args.generator, 'words': goal(values), 'l1': args.l1} if args.autopilot else None
                result = desk.run(args.seed, args.driver, args.helper_app, autopilot=autopilot)
        print(json.dumps(result, indent=2))
        return 0
    except (reflex.Refused, reflex.Stopped) as error:
        print(f'REFUSED: {error}', file=sys.stderr)
        return 3
    finally:
        signals.quiet()


if __name__ == '__main__':
    sys.exit(main())
