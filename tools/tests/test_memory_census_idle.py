"""The idle census's arithmetic: a stretch's counters become rates, and a repeat's rows sum per kind (t-82914)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "memory-census"))
import idle  # noqa: E402


def counters(cpu_ns, idle_wakeups, interrupt_wakeups, energy_nj, footprint=100, peak=200):
    return {"cpu_ns": cpu_ns, "idle_wakeups": idle_wakeups, "interrupt_wakeups": interrupt_wakeups,
            "energy_nj": energy_nj, "footprint": footprint, "peak": peak, "age_s": 1}


class IdleArithmetic(unittest.TestCase):
    def test_a_stretch_becomes_rates_from_the_processes_standing_at_both_ends(self):
        one = {"role": "helper", "kind": "renderer", "pids": [1, 2, 3]}
        before = {1: counters(0, 0, 0, 0), 2: counters(0, 0, 0, 0), 3: counters(0, 0, 0, 0)}
        # Two seconds: pid 1 spent 0.5 s of CPU, pid 2 0.1 s; pid 3 ended in between.
        after = {1: counters(5e8, 10, 100, 4e6, footprint=300), 2: counters(1e8, 2, 20, 0, footprint=50), 3: None}
        row = idle.delta_row(one, before, after, 2.0)
        self.assertEqual(row["processes"], 2)
        self.assertEqual(row["gone"], 1)
        self.assertAlmostEqual(row["cpu_pct"], 30.0)
        self.assertAlmostEqual(row["idle_wakeups_per_s"], 6.0)
        self.assertAlmostEqual(row["interrupt_wakeups_per_s"], 60.0)
        self.assertAlmostEqual(row["energy_mw"], 2.0)
        self.assertEqual(row["footprint"], 350)

    def test_the_busiest_threads_come_first_and_a_new_thread_says_so(self):
        before = {7: ("pump", 0), 8: ("idle", 5)}
        after = {7: ("pump", 4e8), 8: ("idle", 5), 9: ("fresh", 1e8)}
        rows = idle.busiest_threads(before, after, 1.0, 5)
        self.assertEqual([one["thread"] for one in rows], ["pump", "fresh"])
        self.assertEqual(rows[1]["new"], True)
        self.assertAlmostEqual(rows[0]["cpu_pct"], 40.0)

    def test_a_repeat_sums_its_rows_per_role_and_kind(self):
        line = {"rows": [
            {"role": "helper", "kind": "renderer", "processes": 1, "cpu_pct": 1.0, "idle_wakeups_per_s": 1,
             "interrupt_wakeups_per_s": 2, "energy_mw": 0, "footprint": 10},
            {"role": "helper", "kind": "renderer", "processes": 1, "cpu_pct": 2.0, "idle_wakeups_per_s": 1,
             "interrupt_wakeups_per_s": 2, "energy_mw": 0, "footprint": 20},
            {"role": "panes", "kind": "3 pane programs, with descendants", "processes": 9, "cpu_pct": 5.0,
             "idle_wakeups_per_s": 0, "interrupt_wakeups_per_s": 0, "energy_mw": 0, "footprint": 0},
            {"role": "helper", "kind": "gpu-process", "gone": 1},
        ]}
        summed = idle.by_kind(line)
        self.assertEqual(summed[("helper", "renderer")]["cpu_pct"], 3.0)
        self.assertEqual(summed[("helper", "renderer")]["processes"], 2)
        self.assertIn(("panes", "pane programs, with descendants"), summed)
        self.assertNotIn(("helper", "gpu-process"), summed)

    def test_the_descendants_of_the_pane_programs_are_found_whole(self):
        rows = [{"pid": 10, "ppid": 1}, {"pid": 11, "ppid": 10}, {"pid": 12, "ppid": 11}, {"pid": 20, "ppid": 1}]
        self.assertEqual(idle.descendants(rows, [10]), {10, 11, 12})


if __name__ == "__main__":
    unittest.main()
