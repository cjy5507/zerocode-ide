#!/usr/bin/env python3
"""The conformance suite of scenario s08-pricing-audit, run with the checkout as the working
directory. It lives outside the seed so that the audit has to be done from SPEC.md."""

import json
import os
import sys
import unittest
from pathlib import Path

CHECKOUT = Path.cwd()
sys.path.insert(0, os.fspath(CHECKOUT))

from pricing import price_order  # noqa: E402

VIOLATED_IN_THE_SEED = ["R10", "R2", "R3", "R7", "R9"]
YEAR_END = "2026-12-31"


def percent(value, **more):
    return {"kind": "percent", "value": value, "expires": YEAR_END, **more}


def fixed(value, **more):
    return {"kind": "fixed", "value": value, "expires": YEAR_END, **more}


class Rules(unittest.TestCase):
    def field(self, name, *args, **kwargs):
        return price_order(*args, **kwargs)[name]

    def test_r1_input(self):
        for lines in ([], [(100, 0)], [(100, -1)], [(-1, 1)], [(100, 1.5)], [(100, 1), (100, 0)]):
            with self.assertRaises(ValueError, msg=lines):
                price_order(lines)
        self.assertEqual(self.field("gross", [(100, 2), (0, 1), (7, 3)]), 221)

    def test_r2_tier_edges(self):
        for quantity, price in ((9, 9000), (10, 9500), (49, 46550), (50, 45000), (51, 45900), (99, 89100), (100, 85000), (101, 85850)):
            self.assertEqual(self.field("gross", [(1000, quantity)]) - self.field("line_discount", [(1000, quantity)]), price, quantity)

    def test_r2_tiers_go_line_by_line(self):
        self.assertEqual(self.field("line_discount", [(100, 9), (100, 9)]), 0)
        self.assertEqual(self.field("line_discount", [(100, 10), (100, 9)]), 50)

    def test_r3_percentages_multiply(self):
        self.assertEqual(self.field("line_discount", [(1000, 10)], member=True), 785)
        self.assertEqual(self.field("line_discount", [(1000, 50)], member=True), 6350)
        self.assertEqual(self.field("line_discount", [(1000, 100)], member=True), 17550)
        self.assertEqual(self.field("line_discount", [(1000, 1)], member=True), 30)

    def test_r4_a_line_rounds_half_up_once(self):
        self.assertEqual(self.field("line_discount", [(333, 10)]), 166)
        self.assertEqual(self.field("line_discount", [(1, 1)], member=True), 0)
        self.assertEqual(self.field("line_discount", [(50, 1)], member=True), 1)
        self.assertEqual(self.field("line_discount", [(333, 10), (333, 10)]), 332)

    def test_r5_eligibility_edges(self):
        self.assertEqual(self.field("coupon_discount", [(2000, 1)], coupon=percent(10), date=YEAR_END), 200)
        self.assertEqual(self.field("coupon_discount", [(2000, 1)], coupon=percent(10), date="2027-01-01"), 0)
        self.assertEqual(self.field("coupon_discount", [(2000, 1)], coupon=percent(10, min_subtotal=2000)), 200)
        self.assertEqual(self.field("coupon_discount", [(2000, 1)], coupon=percent(10, min_subtotal=2001)), 0)

    def test_r5_the_minimum_reads_the_price_after_line_discounts(self):
        self.assertEqual(self.field("coupon_discount", [(1000, 10)], coupon=percent(10, min_subtotal=9500)), 950)
        self.assertEqual(self.field("coupon_discount", [(1000, 10)], coupon=percent(10, min_subtotal=9501)), 0)

    def test_r6_amounts(self):
        self.assertEqual(self.field("coupon_discount", [(1000, 1)], coupon=percent(25)), 250)
        self.assertEqual(self.field("coupon_discount", [(10, 1), (5, 1)], coupon=percent(30)), 5)
        self.assertEqual(self.field("coupon_discount", [(1050, 1)], coupon=percent(10)), 105)
        self.assertEqual(self.field("coupon_discount", [(1005, 1)], coupon=percent(10)), 101)
        self.assertEqual(self.field("coupon_discount", [(2000, 1)], coupon=fixed(500)), 500)

    def test_r6_a_fixed_coupon_is_bounded_by_the_sum_before_the_cap(self):
        self.assertEqual(self.field("coupon_discount", [(1000, 1)], coupon=fixed(300)), 300)
        self.assertEqual(self.field("coupon_discount", [(1000, 1)], coupon=fixed(400)), 400)
        self.assertEqual(self.field("coupon_discount", [(1000, 1)], coupon=fixed(5000)), 400)

    def test_r7_the_cap_reads_the_gross(self):
        said = price_order([(1000, 100)], coupon=percent(50))
        self.assertEqual((said["line_discount"], said["coupon_discount"], said["subtotal"]), (15000, 25000, 60000))
        said = price_order([(1000, 10)], member=True, coupon=percent(60))
        self.assertEqual((said["line_discount"], said["coupon_discount"]), (785, 3215))

    def test_r7_the_cap_rounds_down(self):
        self.assertEqual(self.field("coupon_discount", [(101, 1)], coupon=percent(90)), 40)
        self.assertEqual(self.field("coupon_discount", [(103, 1)], coupon=fixed(90)), 41)

    def test_r7_a_discount_under_the_cap_is_left_alone(self):
        self.assertEqual(self.field("coupon_discount", [(1000, 10)], coupon=percent(10)), 950)

    def test_r8_shipping_edge(self):
        self.assertEqual(self.field("shipping", [(5000, 1)]), 0)
        self.assertEqual(self.field("shipping", [(4999, 1)]), 499)
        self.assertEqual(self.field("shipping", [(5100, 1)], coupon=fixed(101)), 499)
        self.assertEqual(self.field("shipping", [(5100, 1)], coupon=fixed(100)), 0)

    def test_r9_parts_of_a_kilogram(self):
        cases = {20_000: 0, 20_001: 50, 20_999: 50, 21_000: 50, 21_001: 100, 25_500: 300, 0: 0, 19_999: 0}
        for weight, surcharge in cases.items():
            self.assertEqual(self.field("shipping", [(9000, 1)], weight_g=weight), surcharge, weight)

    def test_r9_it_adds_to_paid_shipping_too(self):
        self.assertEqual(self.field("shipping", [(100, 1)], weight_g=20_001), 549)

    def test_r10_half_to_even(self):
        for subtotal, tax in ((200, 16), (600, 50), (1000, 82), (1400, 116), (1800, 148), (999, 82), (5400, 446)):
            self.assertEqual(self.field("tax", [(subtotal, 1)]), tax, subtotal)

    def test_r10_shipping_is_not_taxed(self):
        self.assertEqual(self.field("tax", [(999, 1)]), self.field("tax", [(999, 1)], weight_g=30_000))

    def test_r11_total(self):
        said = price_order([(333, 10), (1200, 1)], member=True, coupon=fixed(100), weight_g=20_500)
        self.assertEqual(said["total"], said["subtotal"] + said["shipping"] + said["tax"])
        self.assertEqual(said["gross"] - said["line_discount"] - said["coupon_discount"], said["subtotal"])


class Handover(unittest.TestCase):
    def test_the_findings_name_exactly_the_rules_the_seed_broke(self):
        found = json.loads((CHECKOUT / "findings.json").read_text())
        self.assertEqual(sorted(found["violated"]), VIOLATED_IN_THE_SEED)

    def test_the_authors_tests_still_pass(self):
        suite = unittest.defaultTestLoader.discover(os.fspath(CHECKOUT), pattern="test_*.py")
        with open(os.devnull, "w") as quiet:
            result = unittest.TextTestRunner(stream=quiet).run(suite)
        self.assertGreater(result.testsRun, 0)
        self.assertTrue(result.wasSuccessful())


if __name__ == "__main__":
    unittest.main()
