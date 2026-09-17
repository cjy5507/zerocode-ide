"""Independent judges must reject planted defects and incomplete evidence."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

BASE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fixture', BASE / 'fixture.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)

class Judges(unittest.TestCase):
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
