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
                                                        cold_ns=50_000_000, scene="heldout-01", qos="q", block=0)]})
        line = summary["series"][0]
        self.assertEqual(line["cold_max_ms"], 50.0)
        self.assertEqual(line["p50_ms"], 1.0)
        self.assertEqual(line["p95_ms"], 1.0)
        self.assertEqual(line["max_ms"], 9.0)
        self.assertEqual(summary["cells_worst_scene_p95_ms"], {"q": (1.0, "heldout-01")})

    def test_blocks_of_one_qos_pool_and_qos_stay_apart(self):
        rows = [row("tick", [1] * 10, 2, qos="a", block=0), row("tick", [3] * 10, 2, qos="b", block=1),
                row("tick", [3] * 10, 2, qos="b", block=2), row("tick", [1] * 9 + [100], 2, qos="a", block=3)]
        lines = {line["qos"]: line for line in summarize({"limits": {}, "rows": rows})["series"]}
        self.assertEqual(lines["a"]["n"], 20)
        self.assertEqual(lines["a"]["blocks"], [0, 3])
        self.assertEqual(lines["a"]["max_ms"], 100 / 1e6)
        self.assertEqual(lines["b"]["p95_ms"], 3 / 1e6)
        self.assertEqual(len(lines["a"]["load"]), 2)

    def test_per_sample_cost_needs_samples(self):
        summary = summarize({"limits": {}, "rows": [row("blobs-detector", [262_144] * 4, 262_144),
                                                    row("motion", [10] * 4)]})
        self.assertEqual(summary["series"][0]["ns_per_sample_p95"], 1.0)
        self.assertNotIn("ns_per_sample_p95", summary["series"][1])
        self.assertEqual(summary["cells_worst_scene_p95_ms"], {})


if __name__ == "__main__":
    unittest.main()
