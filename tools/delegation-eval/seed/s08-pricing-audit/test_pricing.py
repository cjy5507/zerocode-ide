import unittest

from pricing import price_order

COUPON = {"kind": "percent", "value": 10, "expires": "2026-12-31"}


class PricingTest(unittest.TestCase):
    def test_a_plain_order(self):
        self.assertEqual(price_order([(1999, 2)]), {
            "gross": 3998, "line_discount": 0, "coupon_discount": 0,
            "subtotal": 3998, "shipping": 499, "tax": 330, "total": 4827,
        })

    def test_volume_tiers(self):
        self.assertEqual(price_order([(500, 10)])["line_discount"], 250)
        self.assertEqual(price_order([(100, 100)])["line_discount"], 1500)
        self.assertEqual(price_order([(100, 9)])["line_discount"], 0)

    def test_a_member_without_a_tier(self):
        self.assertEqual(price_order([(1000, 1)], member=True)["line_discount"], 30)

    def test_a_percent_coupon(self):
        said = price_order([(2000, 3)], coupon=COUPON, date="2026-06-01")
        self.assertEqual((said["coupon_discount"], said["subtotal"], said["shipping"]), (600, 5400, 0))

    def test_a_fixed_coupon(self):
        coupon = {"kind": "fixed", "value": 500, "expires": "2026-12-31"}
        self.assertEqual(price_order([(2000, 1)], coupon=coupon)["total"], 2123)

    def test_a_fixed_coupon_never_goes_below_nothing(self):
        coupon = {"kind": "fixed", "value": 1000, "expires": "2026-12-31"}
        self.assertEqual(price_order([(300, 2), (100, 1)], coupon=coupon)["subtotal"], 420)

    def test_a_coupon_past_its_date_or_minimum_is_ignored(self):
        self.assertEqual(price_order([(2000, 1)], coupon=COUPON, date="2027-01-01")["coupon_discount"], 0)
        coupon = {**COUPON, "min_subtotal": 3000}
        self.assertEqual(price_order([(2000, 1)], coupon=coupon)["coupon_discount"], 0)

    def test_the_discount_cap(self):
        coupon = {"kind": "percent", "value": 60, "expires": "2026-12-31"}
        said = price_order([(10_000, 1)], coupon=coupon)
        self.assertEqual((said["coupon_discount"], said["subtotal"], said["tax"], said["total"]), (4000, 6000, 495, 6495))

    def test_a_heavy_parcel(self):
        self.assertEqual(price_order([(9000, 1)], weight_g=20_000)["shipping"], 0)
        self.assertEqual(price_order([(9000, 1)], weight_g=21_000)["shipping"], 50)

    def test_what_is_not_an_order(self):
        for lines in ([], [(100, 0)], [(100, -1)], [(-1, 1)], [(100, 1.5)]):
            with self.assertRaises(ValueError, msg=lines):
                price_order(lines)


if __name__ == "__main__":
    unittest.main()
