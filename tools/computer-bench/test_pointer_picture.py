"""The pointer's own picture, told the same here as in the core (t-12979):
one shared table of rows read on this Mac and of guards, and the core's own
three values read from its source. Nothing here reads a screen."""
import json
import pathlib
import re
import unittest

import pointer_picture as pointer

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXAMPLES = json.loads((ROOT / "crates/zerocode-core/fixtures/pointer-picture/examples.json").read_text())["examples"]
CORE = (ROOT / "crates/zerocode-core/src/computer_use_protocol/pointer.rs").read_text()


def row(example, x=0, y=0):
    """A list-all-windows row of the example's layer, owner and size."""
    return {"id": 1, "app": {"name": example["owner"], "pid": 1}, "x": x, "y": y, "width": example["width"],
            "height": example["height"], "layer": example["layer"], "alpha": 1.0, "overlay": False, "own": False}


class SameAnswers(unittest.TestCase):
    def test_every_example_of_the_shared_table_is_answered_as_the_table_says(self):
        self.assertGreaterEqual(len(EXAMPLES), 7)
        for example in EXAMPLES:
            self.assertEqual(pointer.is_pointer_picture(row(example)), example["pointer"], example["what"])

    def test_the_three_values_are_the_cores(self):
        def core(name):
            found = re.search(rf"pub const {name}: [^=]+= ([^;]+);", CORE)
            self.assertIsNotNone(found, name)
            return found.group(1).strip()
        self.assertEqual(int(core("CURSOR_WINDOW_LAYER").replace("_", "")), pointer.CURSOR_WINDOW_LAYER)
        self.assertEqual(json.loads(core("WINDOW_SERVER_OWNER")), pointer.WINDOW_SERVER_OWNER)
        self.assertEqual(float(core("POINTER_PICTURE_MAX_SIDE_PT")), pointer.POINTER_PICTURE_MAX_SIDE_PT)


class FrontAt(unittest.TestCase):
    """The workbench presses only where ZeroCode's own control is the window
    a press lands on; the pointer resting on the control is not a window."""

    ZEROCODE = {"id": 7, "app": {"name": "ZeroCode", "pid": 500}, "x": 0, "y": 0, "width": 1200, "height": 800,
                "layer": 0, "alpha": 1.0, "overlay": False, "own": True}

    def test_the_pointer_resting_on_the_control_is_not_what_a_press_lands_on(self):
        read = next(example for example in EXAMPLES if example["pointer"])
        self.assertEqual(pointer.front_at([row(read, 590, 390), self.ZEROCODE], 600, 400), self.ZEROCODE)

    def test_a_window_not_all_three_of_the_pointer_is_what_a_press_lands_on(self):
        for example in (example for example in EXAMPLES if not example["pointer"]):
            over = row(example, 590, 390)
            self.assertEqual(pointer.front_at([over, self.ZEROCODE], 600, 400), over, example["what"])


if __name__ == "__main__":
    unittest.main()
