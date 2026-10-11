"""Exercise a staged signed helper against owned, inactive fixture windows.

Prepare never launches an app. Run uses the window's official Computer Use
door, a caller-supplied Rust test binary and the helper's authenticated session.
It never changes permissions, resumes a stopped operator or reads clipboard text.
"""
import argparse
import json
import pathlib
import re
import subprocess
import time

import fixture_support
from fixture_reflex import write_atomic

CONCURRENT_READY_SECONDS = 20


def decoded(returncode, stdout, stderr):
    value = json.loads(stdout or stderr)
    if returncode or not value.get('ok'):
        raise RuntimeError(value.get('error', 'Computer Use refused'))
    return value['result']


def computer(*arguments):
    reply = subprocess.run(['zerocode-computer', *arguments, '--json'],
                           capture_output=True, text=True, check=False)
    return decoded(reply.returncode, reply.stdout, reply.stderr)


def ready():
    status = computer('status')
    if status.get('helper') == 'idle':
        computer('capabilities')
        status = computer('status')
    helper = status.get('helper')
    if not isinstance(helper, dict) or status.get('stopped') or status.get('confirming'):
        raise RuntimeError('operator is stopped, unavailable or awaiting confirmation')
    if helper.get('stopped') or not helper.get('hotkeyArmed') or not helper.get('hotkeyHears'):
        raise RuntimeError('operator stop must be armed and hearing before the fixture runs: '
                           f"stopped={helper.get('stopped')}, armed={helper.get('hotkeyArmed')}, hears={helper.get('hotkeyHears')}")


def wait_state(path, bundle):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if path.exists():
            state = json.loads(path.read_text())
            if state.get('bundleId') != bundle or state.get('active'):
                raise RuntimeError('fixture ownership or background state changed')
            if len(state.get('windows', [])) == 2:
                return state
        time.sleep(.02)
    raise RuntimeError('owned background fixture did not become ready')


def answer_gate(folder, previous):
    path = folder / 'request.json'
    if not path.exists():
        return previous
    request = json.loads(path.read_text()).get('id')
    if not isinstance(request, int) or request <= 0 or request == previous:
        return previous
    try:
        ready()
        permitted = True
    except (RuntimeError, ValueError, OSError):
        permitted = False
    write_atomic(folder / 'answer.json', {'id': request, 'ready': permitted})
    return request


def pointer_motion_since(initial, current):
    before = initial.get('sharedPointer', {})
    after = current.get('sharedPointer', {})
    if not after.get('ready') or after.get('overflow') or not after.get('cursorReadable'):
        raise RuntimeError('the concurrent pointer observer is unavailable or incomplete')
    return (after.get('mouseMoves', 0) > before.get('mouseMoves', 0)
            and after.get('cursorChanges', 0) > before.get('cursorChanges', 0))


def wait_external_pointer(path, initial):
    deadline = time.monotonic() + CONCURRENT_READY_SECONDS
    print('Concurrent fixture ready; waiting for external mouse movement before input.', flush=True)
    while time.monotonic() < deadline:
        current = json.loads(path.read_text())
        if current.get('pid') != initial.get('pid') or current.get('bundleId') != initial.get('bundleId') or current.get('active'):
            raise RuntimeError('fixture ownership or background state changed during calibration')
        if pointer_motion_since(initial, current):
            return current
        time.sleep(.05)
    raise RuntimeError('external pointer motion was not independently observed; no input trial started')


def run_program(folder, name, argv, environment, timeout, gate=False):
    runner = folder / (name + '.py')
    runner.write_text('#!/usr/bin/env python3\nimport os\n'
                      f'os.environ.update({environment!r})\n'
                      f'os.execv({argv[0]!r}, {argv!r})\n')
    runner.chmod(0o700)
    arguments = ['run', '--program', str(runner), '--timeout-ms', str(timeout)]
    if not gate:
        return computer(*arguments)
    with subprocess.Popen(['zerocode-computer', *arguments, '--json'],
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
        previous = None
        while True:
            try:
                stdout, stderr = process.communicate(timeout=.02)
                return decoded(process.returncode, stdout, stderr)
            except subprocess.TimeoutExpired:
                previous = answer_gate(folder, previous)


def run(folder, binary, helper, observation_only=False, input_only=False, concurrent_input=False, show_windows=False):
    session = json.loads((folder / 'session.json').read_text())
    if session.get('swiftc') != 0 or not pathlib.Path(session['executable']).is_file():
        raise RuntimeError('the owned fixture must compile successfully before it can launch')
    state_path = folder / 'state.json'
    if state_path.exists():
        raise RuntimeError('use a freshly prepared fixture folder')
    if not binary.is_file() or not helper.is_dir():
        raise RuntimeError('the test binary and staged helper must exist')
    ready()
    arguments = ['/usr/bin/open', '-g', '-n', session['app'], '--args', str(state_path)]
    if show_windows:
        arguments.append('--show-windows')
    launch = run_program(folder, 'launch', arguments, {}, 5000)
    if launch.get('exitCode') != 0 or launch.get('timedOut'):
        raise RuntimeError('fixture launch failed')
    state = None
    try:
        state = wait_state(state_path, session['bundle'])
        if show_windows:
            visibility = computer('list-all-windows', '--all-layers')
            (folder / 'visibility-start.json').write_text(json.dumps(visibility, indent=2) + '\n')
        if concurrent_input:
            state = wait_external_pointer(state_path, state)
        (folder / 'fixture-start.json').write_text(json.dumps(state, indent=2) + '\n')
        ready()
        prefix = 'computer_use::macos::background_tests::'
        action = ('background_accessibility_preserves_the_persons_desktop'
                  if pathlib.Path(session['executable']).name == 'BackgroundWebFixture'
                  else 'window_scoped_input_preserves_the_persons_desktop')
        if concurrent_input:
            action = ('background_accessibility_while_the_person_moves_the_pointer'
                      if pathlib.Path(session['executable']).name == 'BackgroundWebFixture'
                      else 'window_scoped_input_while_the_person_moves_the_pointer')
        elif show_windows:
            action = ('accessibility_pointer_is_visible_without_taking_focus'
                      if pathlib.Path(session['executable']).name == 'BackgroundWebFixture'
                      else 'window_pointer_is_visible_without_taking_focus')
        tests = [prefix + action, prefix + 'background_observation_memory_and_latency']
        if observation_only:
            tests = tests[1:]
        elif input_only or concurrent_input or show_windows:
            tests = tests[:1]
        environment = {'ZEROCODE_BACKGROUND_FIXTURE_STATE': str(state_path),
                       'ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH': str(helper),
                       'ZEROCODE_BACKGROUND_OPERATOR_GATE': str(folder),
                       'ZEROCODE_BACKGROUND_POINTER_VISIBLE': '1' if show_windows else '0'}
        result = run_program(folder, 'test', [str(binary), *tests, '--ignored', '--nocapture', '--test-threads=1'],
                             environment, 120000, gate=True)
        (folder / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(result.get('stdout', '') + result.get('stderr', ''))
        passed = re.search(r'test result: ok\. [1-9]\d* passed;', result.get('stdout', ''))
        if result.get('exitCode') != 0 or result.get('timedOut') or not passed:
            raise RuntimeError('background provider assertions failed or did not run')
    finally:
        ready()
        if state is None:
            computer('quit', '--app', session['bundle'])
        else:
            current = json.loads(state_path.read_text())
            if current.get('pid') != state['pid'] or current.get('bundleId') != session['bundle']:
                raise RuntimeError('fixture identity changed; cleanup refused')
            computer('quit', '--app', f"pid:{state['pid']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    prepare = commands.add_parser('prepare')
    prepare.add_argument('folder', type=pathlib.Path)
    prepare.add_argument('--engine', choices=('appkit', 'webkit'), default='appkit')
    execute = commands.add_parser('run')
    execute.add_argument('folder', type=pathlib.Path)
    execute.add_argument('--test-bin', type=pathlib.Path, required=True)
    execute.add_argument('--helper', type=pathlib.Path, required=True)
    execute.add_argument('--show-windows', action='store_true',
                         help='show owned windows without activating them; assert and capture the virtual pointer')
    selected = execute.add_mutually_exclusive_group()
    selected.add_argument('--observation-only', action='store_true')
    selected.add_argument('--input-only', action='store_true')
    selected.add_argument('--concurrent-input', action='store_true')
    arguments = parser.parse_args()
    folder = arguments.folder.resolve()
    if arguments.command == 'prepare':
        name = 'BackgroundWebFixture' if arguments.engine == 'webkit' else 'BackgroundFixture'
        return fixture_support.prepare(folder, name,
                                       pathlib.Path(__file__).with_name(name + '.swift'),
                                       'dev.zerocode.bench.background-input',
                                       [pathlib.Path(__file__).with_name('BackgroundPointerObserver.swift')])
    run(folder, arguments.test_bin.resolve(), arguments.helper.resolve(), arguments.observation_only,
        arguments.input_only, arguments.concurrent_input, arguments.show_windows)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
