import unittest
import pathlib
import tempfile
import json
from unittest.mock import patch

import background_input


class BackgroundAdmissionTests(unittest.TestCase):
    def test_calibration_needs_real_cursor_change_and_a_hearing_observer(self):
        initial = {'sharedPointer': {'mouseMoves': 4, 'cursorChanges': 2}}
        current = {'sharedPointer': {'ready': True, 'overflow': False, 'cursorReadable': True,
                                     'mouseMoves': 5, 'cursorChanges': 3}}
        self.assertTrue(background_input.pointer_motion_since(initial, current))
        current['sharedPointer']['mouseMoves'] = 4
        self.assertFalse(background_input.pointer_motion_since(initial, current))
        current['sharedPointer']['mouseMoves'] = 5
        current['sharedPointer']['cursorChanges'] = 2
        self.assertFalse(background_input.pointer_motion_since(initial, current))
        current['sharedPointer']['overflow'] = True
        with self.assertRaises(RuntimeError):
            background_input.pointer_motion_since(initial, current)

    def test_concurrent_input_uses_the_provenance_oracle_without_running_a_latency_benchmark(self):
        for executable_name, test_name, show_windows, concurrent in [
            ('BackgroundFixture', 'window_scoped_input_while_the_person_moves_the_pointer', False, True),
            ('BackgroundFixture', 'window_scoped_input_while_the_person_moves_the_pointer', True, True),
            ('BackgroundWebFixture', 'background_accessibility_while_the_person_moves_the_pointer', False, True),
            ('BackgroundWebFixture', 'background_accessibility_while_the_person_moves_the_pointer', True, True),
            ('BackgroundFixture', 'window_pointer_is_visible_without_taking_focus', True, False),
            ('BackgroundWebFixture', 'accessibility_pointer_is_visible_without_taking_focus', True, False),
        ]:
            with self.subTest(engine=executable_name, visible=show_windows, concurrent=concurrent), tempfile.TemporaryDirectory() as directory:
                folder = pathlib.Path(directory)
                executable = folder / executable_name
                executable.touch()
                helper = folder / 'helper.app'
                helper.mkdir()
                binary = folder / 'test-bin'
                binary.touch()
                bundle = 'dev.zerocode.bench.background-input.test'
                (folder / 'session.json').write_text(json.dumps({
                    'swiftc': 0, 'executable': str(executable), 'bundle': bundle, 'app': 'fixture.app',
                }))
                state = {'pid': 123, 'bundleId': bundle}

                def prepared(path, expected_bundle):
                    self.assertEqual(expected_bundle, bundle)
                    path.write_text(json.dumps(state))
                    return state

                with patch.object(background_input, 'ready'), \
                        patch('builtins.print'), \
                        patch.object(background_input, 'wait_external_pointer', return_value=state), \
                        patch.object(background_input, 'wait_state', side_effect=prepared), \
                        patch.object(background_input, 'computer', return_value={'windows': []}) as computer, \
                        patch.object(background_input, 'run_program', side_effect=[
                            {'exitCode': 0},
                            {'exitCode': 0, 'stdout': 'test result: ok. 1 passed;'},
                        ]) as launch:
                    background_input.run(folder, binary, helper, concurrent_input=concurrent, show_windows=show_windows)
                opening = launch.call_args_list[0].args[2]
                self.assertIn('-g', opening)
                self.assertEqual('--show-windows' in opening, show_windows)
                environment = launch.call_args_list[1].args[3]
                self.assertEqual(environment['ZEROCODE_BACKGROUND_POINTER_VISIBLE'], '1' if show_windows else '0')
                arguments = launch.call_args_list[1].args[2]
                self.assertEqual(arguments, [str(binary), 'computer_use::macos::background_tests::' + test_name,
                                             '--ignored', '--nocapture', '--test-threads=1'])
                self.assertEqual(computer.call_args_list[-1].args, ('quit', '--app', 'pid:123'))
                self.assertEqual(len(computer.call_args_list), 2 if show_windows else 1)

    def test_each_action_needs_a_fresh_authorized_operator_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = pathlib.Path(directory)
            (folder / 'request.json').write_text('{"id": 1}')
            with patch.object(background_input, 'ready') as status:
                previous = background_input.answer_gate(folder, None)
                self.assertEqual(json.loads((folder / 'answer.json').read_text()), {'id': 1, 'ready': True})
                self.assertEqual(background_input.answer_gate(folder, previous), 1)
                status.assert_called_once()
            (folder / 'request.json').write_text('{"id": 2}')
            with patch.object(background_input, 'ready', side_effect=RuntimeError('stopped')):
                background_input.answer_gate(folder, previous)
                self.assertEqual(json.loads((folder / 'answer.json').read_text()), {'id': 2, 'ready': False})

    def test_paths_and_environment_are_data_not_shell_words(self):
        with tempfile.TemporaryDirectory(prefix='background bench ') as directory:
            with patch.object(background_input, 'computer', return_value={}) as call:
                folder = pathlib.Path(directory)
                background_input.run_program(folder, 'test', ['/program with spaces', "a'b;$(nothing)"],
                                             {'VALUE': 'a value with spaces'}, 5000)
                source = (folder / 'test.py').read_text()
                compile(source, 'runner', 'exec')
                self.assertIn("os.environ.update({'VALUE': 'a value with spaces'})", source)
                self.assertEqual(call.call_args.args[0:3], ('run', '--program', str(folder / 'test.py')))

    def test_it_never_resumes_or_runs_input_when_the_operator_is_stopped(self):
        for helper in [None, 'idle', {}, {'stopped': True},
                       {'hotkeyArmed': True, 'hotkeyHears': False}]:
            with self.subTest(helper=helper):
                with patch.object(background_input, 'computer', return_value={'stopped': True, 'helper': helper}) as call:
                    with self.assertRaises(RuntimeError):
                        background_input.ready()
                    self.assertTrue(all(row.args[0] in ('status', 'capabilities') for row in call.call_args_list))

    def test_an_idle_helper_is_readied_with_read_only_capabilities(self):
        with patch.object(background_input, 'computer', side_effect=[{'helper': 'idle'}, {}, {
            'helper': {'stopped': False, 'hotkeyArmed': True, 'hotkeyHears': True},
        }]) as call:
            background_input.ready()
            self.assertEqual([row.args[0] for row in call.call_args_list], ['status', 'capabilities', 'status'])

    def test_pending_confirmation_is_not_answered_by_the_bench(self):
        with patch.object(background_input, 'computer', return_value={
            'confirming': 1, 'helper': {'hotkeyArmed': True, 'hotkeyHears': True},
        }):
            with self.assertRaises(RuntimeError):
                background_input.ready()


if __name__ == '__main__':
    unittest.main()
