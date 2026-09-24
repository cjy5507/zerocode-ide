import unittest

from probe import BOARD, CENTRES, PALETTE, classify_board, order, percentile
from web_probe import unframe


class ProbeTests(unittest.TestCase):
    def test_unknown_pixels_cannot_choose_an_action(self):
        from PIL import Image
        im = Image.new("RGB", (100, 200), "black")
        self.assertEqual(classify_board(im), [None] * 16)

    def test_pixel_reader_scores_cells_and_rejects_wrong_orientation(self):
        from PIL import Image, ImageDraw
        im = Image.new("RGB", (400, 900), "black")
        draw = ImageDraw.Draw(im)
        expected = []
        for i, (x, y) in enumerate(CENTRES):
            expected.append(i % len(PALETTE))
            dx, dy = BOARD["width"] / 2, BOARD["height"] / 2
            draw.rectangle(((x-dx)*im.width, (y-dy)*im.height,
                            (x+dx)*im.width, (y+dy)*im.height),
                           fill=tuple(PALETTE[expected[-1]]))
        self.assertEqual(classify_board(im), expected)
        self.assertNotEqual(classify_board(im.rotate(90, expand=True)), expected)

    def test_eval_wrapper_keeps_string_values_and_decodes_objects(self):
        self.assertEqual(unframe('<<<BEGIN>>>\n"London"\n<<<END>>>'), "London")
        self.assertEqual(unframe('<<<BEGIN>>>\n"{\\"count\\":1}"\n<<<END>>>'), {"count": 1})

    def test_abba_keeps_both_arms_in_each_block(self):
        self.assertEqual(order(2), ["tap", "touch", "touch", "tap"] * 2)

    def test_tail_is_not_a_median(self):
        self.assertEqual(percentile([1, 2, 3, 4, 100], .95), 100)


if __name__ == "__main__":
    unittest.main()
