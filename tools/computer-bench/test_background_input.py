import unittest
import pathlib
import tempfile
import json
from unittest.mock import patch

import background_input


class BackgroundAdmissionTests(unittest.TestCase):
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
