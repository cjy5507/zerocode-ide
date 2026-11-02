import unittest

from semrange import best, compare, matches, parse


def order(*texts):
    return sorted(texts, key=_Key)


class _Key:
    def __init__(self, text):
        self.version = parse(text)

    def __lt__(self, other):
        return compare(self.version, other.version) < 0


class ParseTest(unittest.TestCase):
    def test_a_plain_version(self):
        self.assertEqual(parse("1.4.0").numbers(), (1, 4, 0))

    def test_prerelease_and_build_are_told_apart(self):
        found = parse("3.1.0-beta.2+exp")
        self.assertEqual((found.prerelease, found.build), (("beta", "2"), "exp"))

    def test_a_build_may_hold_hyphens_and_is_not_a_prerelease(self):
        found = parse("1.0.0+build-5-a")
        self.assertEqual((found.prerelease, found.build), ((), "build-5-a"))

    def test_a_prerelease_may_hold_hyphens(self):
        found = parse("1.0.0-pre-alpha.1+b-2")
        self.assertEqual((found.prerelease, found.build), (("pre-alpha", "1"), "b-2"))

    def test_nonsense_is_refused(self):
        for text in ("1.2", "1.2.3.4", "a.b.c", ""):
            with self.assertRaises(ValueError, msg=text):
                parse(text)


class PrecedenceTest(unittest.TestCase):
    def test_numbers_compare_as_numbers(self):
        self.assertEqual(order("1.10.0", "1.9.0", "1.2.0"), ["1.2.0", "1.9.0", "1.10.0"])

    def test_a_prerelease_comes_before_its_release(self):
        self.assertLess(compare(parse("1.0.0-rc.1"), parse("1.0.0")), 0)
        self.assertGreater(compare(parse("1.0.0"), parse("1.0.0-rc.1")), 0)

    def test_the_whole_prerelease_ladder(self):
        ladder = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta",
                  "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0"]
        for lower, higher in zip(ladder, ladder[1:]):
            self.assertLess(compare(parse(lower), parse(higher)), 0, (lower, higher))
            self.assertGreater(compare(parse(higher), parse(lower)), 0, (lower, higher))

    def test_a_numeric_identifier_comes_before_a_word(self):
        self.assertLess(compare(parse("1.0.0-99"), parse("1.0.0-a")), 0)
        self.assertLess(compare(parse("1.0.0-rc.99"), parse("1.0.0-rc.a")), 0)

    def test_build_takes_no_part(self):
        self.assertEqual(compare(parse("1.0.0+a"), parse("1.0.0+b")), 0)


class RangeTest(unittest.TestCase):
    def check(self, range_text, inside, outside):
        for text in inside:
            self.assertTrue(matches(text, range_text), f"{text} should lie in {range_text}")
        for text in outside:
            self.assertFalse(matches(text, range_text), f"{text} should not lie in {range_text}")

    def test_exact_comparisons(self):
        self.check(">=1.2.3 <2.0.0", ["1.2.3", "1.9.9"], ["1.2.2", "2.0.0"])
        self.check("=1.2.3", ["1.2.3", "1.2.3+b"], ["1.2.4"])
        self.check("1.2.3", ["1.2.3"], ["1.2.4"])
        self.check("<=1.2.3 || >3.0.0", ["1.2.3", "3.0.1"], ["2.0.0", "3.0.0"])

    def test_wildcards(self):
        self.check("1.2.x", ["1.2.0", "1.2.9"], ["1.3.0", "1.1.9"])
        self.check("1.x", ["1.0.0", "1.9.9"], ["2.0.0", "0.9.0"])
        self.check("*", ["0.0.0", "9.9.9"], [])

    def test_tilde_stays_within_the_minor(self):
        self.check("~1.2.3", ["1.2.3", "1.2.9"], ["1.2.2", "1.3.0"])
        self.check("~0.2.3", ["0.2.3", "0.2.9"], ["0.3.0"])

    def test_caret_stays_below_the_next_change_of_the_leftmost_nonzero_number(self):
        self.check("^1.2.3", ["1.2.3", "1.9.0"], ["1.2.2", "2.0.0"])
        self.check("^0.2.3", ["0.2.3", "0.2.9"], ["0.2.2", "0.3.0"])
        self.check("^0.0.3", ["0.0.3"], ["0.0.2", "0.0.4", "0.1.0"])
        self.check("^0.0.0", ["0.0.0"], ["0.0.1"])

    def test_a_prerelease_needs_a_comparator_naming_its_own_numbers(self):
        self.check(">=1.2.3-rc.1 <2.0.0", ["1.2.3-rc.1", "1.2.3-rc.2", "1.2.3", "1.5.0"],
                   ["1.2.3-beta.1", "1.3.0-rc.1", "2.0.0-rc.1"])
        self.check("^1.2.3", [], ["1.2.3-rc.1", "1.3.0-rc.1", "2.0.0-rc.1"])
        self.check("*", [], ["1.0.0-rc.1"])
        self.check("<2.0.0", [], ["2.0.0-rc.1", "1.0.0-rc.1"])
        self.check("~1.2.3-beta.2", ["1.2.3-beta.2", "1.2.3-rc.1", "1.2.4"], ["1.2.3-beta.1", "1.2.4-rc.1"])

    def test_each_alternative_judges_prereleases_for_itself(self):
        self.check("^2.0.0 || >=1.2.3-rc.1 <1.3.0", ["1.2.3-rc.5", "2.1.0"], ["2.1.0-rc.1"])

    def test_best_picks_the_highest_inside(self):
        versions = ["1.2.0", "1.10.0", "2.0.0-rc.1", "1.9.0", "1.10.0+other", "1.11.0-rc.1"]
        self.assertEqual(best(versions, "^1.2.0"), "1.10.0")
        self.assertEqual(best(versions, ">=2.0.0-rc.1"), "2.0.0-rc.1")
        self.assertIsNone(best(versions, "^3.0.0"))


if __name__ == "__main__":
    unittest.main()
