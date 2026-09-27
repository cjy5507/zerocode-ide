"""The fixtures share one ordered recorder; no app or input is launched."""
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

import fixture_support


@unittest.skipUnless(sys.platform == 'darwin', 'native fixture support runs on macOS')
class RecorderTests(unittest.TestCase):
    def test_async_and_final_flush_keep_every_stream_once_and_in_order(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            main = root / 'main.swift'
            main.write_text('''import Foundation
let recorder = Recorder(folder: URL(fileURLWithPath: CommandLine.arguments[1]))
recorder.event(["n": 1])
recorder.frame(["n": 1])
recorder.scene(["n": 1])
recorder.flush(state: ["n": 1])
recorder.scene(["n": 2])
recorder.event(["n": 2])
recorder.frame(["n": 2])
recorder.flush(state: ["n": 2], wait: true)
recorder.flush(state: ["n": 3], wait: true)
''')
            binary = root / 'recorder'
            compiled = subprocess.run(['swiftc', '-swift-version', '6', '-warnings-as-errors',
                                       str(fixture_support.SUPPORT), str(main), '-o', str(binary)], capture_output=True, text=True)
            self.assertEqual(compiled.returncode, 0, compiled.stderr)
            subprocess.run([str(binary), str(root)], check=True, capture_output=True)
            for name in ('events', 'frames', 'oracle'):
                rows = [json.loads(line) for line in (root / f'{name}.jsonl').read_text().splitlines()]
                self.assertEqual(rows, [{'n': 1}, {'n': 2}], name)
            self.assertEqual(json.loads((root / 'fixture.json').read_text()), {'n': 3})


if __name__ == '__main__':
    unittest.main()
