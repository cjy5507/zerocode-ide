"""Independent judges must reject planted defects and incomplete evidence."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

BASE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fixture', BASE / 'fixture.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)

class Judges(unittest.TestCase):
    def test_inherited_git_directory_cannot_redirect_fixture_initialization(self):
        with tempfile.TemporaryDirectory() as tmp:
            foreign = Path(tmp) / 'foreign'
            intended = Path(tmp) / 'intended'
            fixture.prepare(foreign)
            foreign_repo = foreign / 'repo'
            before = fixture.git(foreign_repo, 'rev-parse', 'HEAD')
            with mock.patch.dict(os.environ, {'GIT_DIR': str(foreign_repo / '.git')}):
                fixture.prepare(intended)
                self.assertTrue((intended / 'repo/.git').is_dir())
                self.assertEqual(Path(fixture.git(intended / 'repo', 'rev-parse', '--show-toplevel')).resolve(), (intended / 'repo').resolve())
                self.assertEqual(fixture.git(foreign_repo, 'rev-parse', 'HEAD'), before)

    def test_the_oracle_cannot_be_weakened_inside_the_allowed_source_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture.prepare(root)
            source = root / 'repo/src/lib.rs'
            self.assertNotIn('#[test]', source.read_text())
            source.write_text(source.read_text() + '\n// incidental production edit\n')
            oracle = root / 'repo/tests/inclusive.rs'
            oracle.write_text(oracle.read_text().replace('inclusive_count(3, 3), 1', 'inclusive_count(3, 3), 0'))
            original = fixture.run
            def no_cargo(args, cwd, check=True):
                self.assertNotEqual(args[0], 'cargo', 'changed oracle must be rejected before compilation')
                return original(args, cwd, check)
            with mock.patch.object(fixture, 'run', side_effect=no_cargo):
                self.assertFalse(fixture.verify('Q1', root))

    def test_an_allowed_file_cannot_be_replaced_by_an_external_link(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture.prepare(root)
            fixture.answer('Q1', root)
            source = root / 'repo/src/lib.rs'
            external = root / 'external.rs'
            external.write_bytes(source.read_bytes())
            source.unlink()
            try:
                source.symlink_to(external)
            except OSError:
                self.skipTest('host cannot create fixture symlinks')
            original = fixture.run
            def no_cargo(args, cwd, check=True):
                self.assertNotEqual(args[0], 'cargo', 'external link must be rejected before compilation')
                return original(args, cwd, check)
            with mock.patch.object(fixture, 'run', side_effect=no_cargo):
                self.assertFalse(fixture.verify('Q1', root))

    def test_red_then_answer_green_and_evidence_is_required(self):
        for task in fixture.TASKS:
            with self.subTest(task=task), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                fixture.prepare(root)
                self.assertFalse(fixture.verify(task, root))
                fixture.answer(task, root)
                if task in ('Q2', 'Q3', 'Q5', 'Q6'):
                    self.assertFalse(fixture.verify(task, root))
                fixture.self_test_evidence(task, root)
                self.assertTrue(fixture.verify(task, root))
                # A green test cannot excuse a change outside the task boundary.
                (root / 'repo' / 'unexpected.txt').write_text('out of scope')
                self.assertFalse(fixture.verify(task, root))

    def test_spawn_receipt_is_not_a_completion_receipt(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture.prepare(root)
            fixture.answer('Q2', root)
            fixture.self_test_evidence('Q2', root)
            path = root / 'evidence/delegation.json'
            data = json.loads(path.read_text())
            data['receipts'][0]['status'] = 'running'
            path.write_text(json.dumps(data))
            self.assertFalse(fixture.verify('Q2', root))

    def test_six_executable_exit_code_judges(self):
        for task in fixture.TASKS:
            with tempfile.TemporaryDirectory() as tmp:
                fixture.prepare(Path(tmp))
                result = subprocess.run([str(BASE / 'tasks' / task / 'verify.sh'), tmp], capture_output=True)
                self.assertNotEqual(result.returncode, 0)

if __name__ == '__main__':
    unittest.main()
