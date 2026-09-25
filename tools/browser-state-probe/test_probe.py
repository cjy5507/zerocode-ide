"""The probe's own parts: the Rust it reads, the order it measures in, and its
copy of the settle rule held to the core's cases."""
import re
import unittest

import probe


class ReadsTheRust(unittest.TestCase):
    def test_raw_plain_list_and_number_constants(self):
        text = '''
pub(crate) const A: &str = r#"
one "two"
"#;
pub(crate) const B: &str = r##"has "#" inside"##;
pub(crate) const C: &str = "plain \\"quoted\\"";
pub const D: &[&str] = &["x", "[aria-busy=\\"true\\"]"];
pub const E: [&str; 2] = ["p", "q"];
pub const F: usize = 1_500;
pub const G: f64 = 32.0;
'''
        self.assertEqual(probe.rust_text(text, "A"), '\none "two"\n')
        self.assertEqual(probe.rust_text(text, "B"), 'has "#" inside')
        self.assertEqual(probe.rust_text(text, "C"), 'plain "quoted"')
        self.assertIsNone(probe.rust_text(text, "MISSING"))
        self.assertEqual(probe.rust_list(text, "D"), ["x", '[aria-busy="true"]'])
        self.assertEqual(probe.rust_list(text, "E"), ["p", "q"])
        self.assertEqual(probe.rust_number(text, "F"), 1500)
        self.assertEqual(probe.rust_number(text, "G"), 32.0)

    def test_this_checkout_assembles_every_script_with_the_door_s_own_request(self):
        scripts = probe.Scripts(None)
        self.assertTrue(scripts.snapshot)
        for script in (scripts.look(), scripts.settle(), scripts.press("#go", {"epoch": "1", "value": None, "watch": scripts.watch})):
            self.assertTrue(script.startswith("(() => {") and script.endswith("})()"))
            self.assertIn("const zcEncode", script)
        # Every key `marks_request` hands the page, the probe hands it too.
        door = probe.source(None, probe.DOOR)
        request = door.split("pub(crate) fn marks_request()", 1)[1].split("\n}\n", 1)[0]
        for name in re.findall(r'^\s+"(\w+)": ', request, re.M):
            self.assertTrue(name in scripts.request or name in scripts.request["keys"]
                            or name in scripts.request["observed"] or name in scripts.request["field"],
                            name)
        self.assertEqual(scripts.request["observed"]["cap"], 12)
        self.assertTrue(all(scripts.request["keys"].values()))
        self.assertEqual(scripts.quiet_ms, 50)
        self.assertEqual(scripts.settle_ms, 250)

    def test_a_revision_without_the_snapshot_asks_the_old_request(self):
        scripts = probe.Scripts("a3bf8290")
        self.assertFalse(scripts.snapshot)
        self.assertEqual(sorted(scripts.request), ["answerCap", "selectors"])
        self.assertIn("faces.push(face)", scripts.look())


class MeasuresFairly(unittest.TestCase):
    def test_abba_reverses_every_other_round(self):
        self.assertEqual(probe.abba(["a", "b", "c"], 3), [["a", "b", "c"], ["c", "b", "a"], ["a", "b", "c"]])

    def test_percentile_is_nearest_rank(self):
        self.assertIsNone(probe.percentile([], 50))
        self.assertEqual(probe.percentile([5, 1, 3], 50), 3)
        self.assertEqual(probe.percentile(list(range(1, 9)), 95), 8)

    def test_a_fixture_keeps_its_query_apart_from_its_name(self):
        self.assertTrue(probe.page_url("form.html?delay=0").endswith("/pages/form.html?delay=0"))
        self.assertTrue(probe.page_url("normal.html").endswith("/pages/normal.html"))

    def test_the_fence_is_read_off_an_eval_answer(self):
        framed = '<<<BEGIN UNTRUSTED EXTERNAL CONTENT (x)>>>\n"{\\"ms\\": 3}"\n<<<END UNTRUSTED EXTERNAL CONTENT (x)>>>\n'
        self.assertEqual(probe.unframe(framed), {"ms": 3})


class SettlesByTheCoreRule(unittest.TestCase):
    """The core test `hidden_surface_short_settle_has_a_wall_deadline` cases,
    word for word: the probe's copy of the rule answers them the same."""

    def facts(self, now, last, busy, epoch="e", watched=True):
        return {"documentEpoch": epoch, "now": now, "last": last, "busy": busy, "watched": watched}

    def test_the_cases_the_core_holds(self):
        verdict = probe.settle_verdict
        self.assertIsNone(verdict(self.facts(1040, 1035, False), "e", 1000, 50))
        self.assertIsNone(verdict(self.facts(1200, 1000, True), "e", 1000, 50))
        self.assertIsNone(verdict(self.facts(1010, 200, False), "e", 1000, 50))
        self.assertEqual(verdict(self.facts(1050, 1000, False), "e", 1000, 50), ("ready", "quiet"))
        self.assertEqual(verdict(self.facts(1000, 1000, True, epoch="other"), "e", 1000, 50), ("invalidated", "replaced"))
        self.assertEqual(verdict(self.facts(1500, 1000, False, watched=False), "e", 1000, 50), ("not_ready", "unwatched"))


if __name__ == "__main__":
    unittest.main()
