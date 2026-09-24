import unittest
from summarize import cursor_changes, percentile, summarize


class Measurements(unittest.TestCase):
    def test_no_frames_cannot_pass(self):
        with self.assertRaises(ValueError):
            summarize({"rc": 0, "rows": []})

    def test_future_capture_clock_cannot_look_fast(self):
        with self.assertRaises(ValueError):
            summarize({"rc": 0, "rows": [{"kind": "frame", "age_ms": -1}]})

    def test_missing_samples_are_not_zero(self):
        self.assertIsNone(percentile([], 0.95))

    def test_nearest_rank_tail_is_not_an_average(self):
        self.assertEqual(percentile(list(range(1, 21)), 0.95), 19)

    def test_stationary_cursor_cannot_invent_waypoints(self):
        rows = [{"x": 4, "y": 2, "at_ns": t} for t in range(100)]
        self.assertEqual(len(cursor_changes(rows)) - 1, 0)

    def test_returning_to_an_earlier_point_is_still_a_new_move(self):
        rows = [{"x": x, "y": 2, "at_ns": t} for t, x in enumerate([1, 1, 2, 2, 1])]
        self.assertEqual([r["x"] for r in cursor_changes(rows)], [1, 2, 1])


if __name__ == "__main__":
    unittest.main()
