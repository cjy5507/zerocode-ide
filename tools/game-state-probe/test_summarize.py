import unittest

from summarize import rank, summarize


def row(series, ns, work=0, **extra):
    return dict(series=series, ns=ns, work=work, load_before=1.0, load_after=2.0, **extra)


class SummarizeTests(unittest.TestCase):
    def test_the_tail_is_nearest_rank_not_a_median(self):
        self.assertEqual(rank([1, 2, 3, 4, 100], 0.95), 100)
        self.assertEqual(rank([5, 1, 3], 0.5), 3)
        self.assertEqual(rank(list(range(1, 101)), 0.95), 95)

    def test_a_first_observation_is_kept_apart_from_the_warm_ones(self):
        summary = summarize({"limits": {}, "rows": [row("cells", [1_000_000] * 19 + [9_000_000], 336,
                                                        cold_ns=50_000_000, scene="heldout-01")]})
        line = summary["series"][0]
        self.assertEqual(line["cold_ms"], 50.0)
        self.assertEqual(line["p50_ms"], 1.0)
        self.assertEqual(line["p95_ms"], 1.0)
        self.assertEqual(line["max_ms"], 9.0)
        self.assertEqual(summary["cells_worst_scene"], "heldout-01")

    def test_per_sample_cost_needs_samples(self):
        summary = summarize({"limits": {}, "rows": [row("blobs-detector", [262_144] * 4, 262_144),
                                                    row("motion", [10] * 4)]})
        self.assertEqual(summary["series"][0]["ns_per_sample_p95"], 1.0)
        self.assertNotIn("ns_per_sample_p95", summary["series"][1])
        self.assertIsNone(summary["cells_worst_p95_ms"])


if __name__ == "__main__":
    unittest.main()
