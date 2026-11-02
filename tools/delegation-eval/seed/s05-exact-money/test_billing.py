import unittest

from export import render_csv
from invoice import build_invoice
from ledger import Ledger, UnbalancedEntry

HEAD = "sku,qty,unit,gross,discount,net"


def csv_of(currency, items, percent="0", prices=None):
    order = {"currency": currency, "items": items, "discount_percent": percent}
    if prices:
        order["prices"] = prices
    return render_csv(build_invoice(order))


class ExportTest(unittest.TestCase):
    maxDiff = None

    def test_dollars_with_a_discount_that_needs_sharing(self):
        self.assertEqual(csv_of("USD", [("widget", 3), ("gadget", 2), ("bolt", 7)], "12.5"), "\n".join([
            "currency,USD", HEAD,
            "widget,3,19.99,59.97,7.50,52.47",
            "gadget,2,5.25,10.50,1.31,9.19",
            "bolt,7,0.35,2.45,0.31,2.14",
            "net,,,,,63.80",
            "tax:reduced,,,,,0.46",
            "tax:standard,,,,,4.51",
            "total,,,,,68.77",
        ]) + "\n")

    def test_yen_have_no_decimal_places(self):
        self.assertEqual(csv_of("JPY", [("widget", 1), ("bolt", 3)], "10"), "\n".join([
            "currency,JPY", HEAD,
            "widget,1,2980,2980,298,2682",
            "bolt,3,50,150,15,135",
            "net,,,,,2817",
            "tax:standard,,,,,232",
            "total,,,,,3049",
        ]) + "\n")

    def test_dinars_have_three_and_tax_rounds_half_to_even(self):
        self.assertEqual(csv_of("KWD", [("gadget", 3), ("bolt", 1)]), "\n".join([
            "currency,KWD", HEAD,
            "gadget,3,1.610,4.830,0.000,4.830",
            "bolt,1,0.105,0.105,0.000,0.105",
            "net,,,,,4.935",
            "tax:reduced,,,,,0.242",
            "tax:standard,,,,,0.009",
            "total,,,,,5.186",
        ]) + "\n")

    def test_a_credit_invoice_prints_its_minus_signs(self):
        self.assertEqual(csv_of("USD", [("widget", -2)]), "\n".join([
            "currency,USD", HEAD,
            "widget,-2,19.99,-39.98,0.00,-39.98",
            "net,,,,,-39.98",
            "tax:standard,,,,,-3.30",
            "total,,,,,-43.28",
        ]) + "\n")

    def test_amounts_below_one_keep_their_sign(self):
        text = csv_of("USD", [("bolt", -1)], "50")
        self.assertIn("bolt,-1,0.35,-0.35,-0.18,-0.17\n", text)


class RuleTest(unittest.TestCase):
    def lines(self, *args, **kwargs):
        rows = csv_of(*args, **kwargs).splitlines()[2:]
        return [row.split(",") for row in rows if not row.startswith(("net,", "tax:", "total,"))]

    def test_the_discount_rounds_half_up_in_magnitude(self):
        self.assertEqual(self.lines("USD", [("bolt", 1)], "50")[0][4], "0.18")
        self.assertEqual(self.lines("USD", [("bolt", -1)], "50")[0][4], "-0.18")

    def test_shares_add_up_exactly_and_an_earlier_line_wins_a_tie(self):
        rows = self.lines("USD", [("bolt", 1), ("bolt", 1), ("bolt", 1)], "1")
        self.assertEqual([row[4] for row in rows], ["0.01", "0.00", "0.00"])

    def test_leftover_units_go_to_the_largest_fractions(self):
        prices = {"USD": {"widget": "1.00", "gadget": "1.00", "bolt": "1.01", "washer": "0.98"}}
        rows = self.lines("USD", [("widget", 1), ("gadget", 1), ("bolt", 1), ("washer", 1)], "0.5", prices=prices)
        self.assertEqual([row[4] for row in rows], ["0.01", "0.00", "0.01", "0.00"])

    def test_every_share_of_a_long_invoice_adds_up(self):
        items = [("widget", 7), ("gadget", 11), ("bolt", 13), ("washer", 17), ("bolt", 19), ("washer", 23)]
        for currency, unit in (("USD", 100), ("JPY", 1), ("KWD", 1000)):
            for percent in ("0.5", "7.25", "33.3", "99.99"):
                rows = self.lines(currency, items, percent)
                gross = sum(round(float(row[3]) * unit) for row in rows)
                shares = sum(round(float(row[4]) * unit) for row in rows)
                exact = gross * float(percent) / 100
                self.assertLessEqual(abs(shares - exact), 0.5 + 1e-6, (currency, percent))

    def test_tax_is_worked_out_on_the_class_not_the_line(self):
        prices = {"USD": {"widget": "0.10", "gadget": "0.10", "bolt": "0.10", "washer": "0.10"}}
        text = csv_of("USD", [("gadget", 1), ("washer", 1), ("gadget", 1)], prices=prices)
        self.assertIn("tax:reduced,,,,,0.02\n", text)
        text = csv_of("USD", [("gadget", 1), ("washer", 2)], prices=prices)
        self.assertIn("tax:reduced,,,,,0.02\n", text)

    def test_half_to_even_goes_down_as_well_as_up(self):
        prices = {"USD": {"widget": "0.10", "gadget": "0.10", "bolt": "0.10", "washer": "0.10"}}
        self.assertIn("tax:reduced,,,,,0.00\n", csv_of("USD", [("gadget", 1)], prices=prices))
        prices = {"USD": {"widget": "0.30", "gadget": "0.30", "bolt": "0.10", "washer": "0.10"}}
        self.assertIn("tax:reduced,,,,,0.02\n", csv_of("USD", [("gadget", 1)], prices=prices))

    def test_large_amounts_stay_exact(self):
        prices = {"USD": {"widget": "99999999999.99", "gadget": "0.10", "bolt": "0.10", "washer": "0.10"}}
        text = csv_of("USD", [("widget", 3)], prices=prices)
        self.assertIn("widget,3,99999999999.99,299999999999.97,0.00,299999999999.97\n", text)
        self.assertIn("tax:standard,,,,,24750000000.00\n", text)
        self.assertIn("total,,,,,324749999999.97\n", text)

    def test_what_cannot_be_is_refused(self):
        too_fine = {"USD": {"widget": "1.999"}, "JPY": {"widget": "10.5"}}
        for currency in too_fine:
            with self.assertRaises(ValueError, msg=currency):
                build_invoice({"currency": currency, "items": [("widget", 1)], "prices": too_fine})
        with self.assertRaises(ValueError):
            build_invoice({"currency": "XTS", "items": [("widget", 1)], "prices": {"XTS": {"widget": "1"}}})
        with self.assertRaises(ValueError):
            build_invoice({"currency": "USD", "items": [("widget", 1), ("bolt", -1)]})


class LedgerTest(unittest.TestCase):
    def test_an_invoice_posts_exact_integers(self):
        ledger = Ledger()
        ledger.post(build_invoice({"currency": "USD", "discount_percent": "12.5",
                                   "items": [("widget", 3), ("gadget", 2), ("bolt", 7)]}))
        expected = {"receivable": 6877, "revenue": -6380, "tax:reduced": -46, "tax:standard": -451}
        self.assertEqual(ledger.accounts("USD"), sorted(expected))
        for account, amount in expected.items():
            held = ledger.balance(account, "USD")
            self.assertEqual(held, amount, account)
            self.assertIs(type(held), int, account)

    def test_currencies_are_kept_apart(self):
        ledger = Ledger()
        ledger.post(build_invoice({"currency": "JPY", "items": [("widget", 1)]}))
        ledger.post(build_invoice({"currency": "KWD", "items": [("bolt", 1)]}))
        ledger.post(build_invoice({"currency": "JPY", "items": [("bolt", 2)]}))
        self.assertEqual(ledger.balance("revenue", "JPY"), -3080)
        self.assertEqual(ledger.balance("revenue", "KWD"), -105)
        self.assertEqual(ledger.balance("revenue", "USD"), 0)

    def test_a_credit_undoes_its_invoice(self):
        ledger = Ledger()
        ledger.post(build_invoice({"currency": "USD", "items": [("widget", 2), ("bolt", 9)], "discount_percent": "3"}))
        ledger.post(build_invoice({"currency": "USD", "items": [("widget", -2), ("bolt", -9)], "discount_percent": "3"}))
        for account in ledger.accounts("USD"):
            self.assertEqual(ledger.balance(account, "USD"), 0, account)

    def test_every_invoice_balances_without_any_other_account(self):
        ledger = Ledger()
        for percent in ("0", "0.5", "7.25", "33.3", "99.99"):
            for currency in ("USD", "JPY", "KWD"):
                ledger.post(build_invoice({"currency": currency, "discount_percent": percent,
                                           "items": [("widget", 7), ("gadget", 11), ("bolt", 13), ("washer", 17)]}))
        for currency in ("USD", "JPY", "KWD"):
            self.assertEqual(ledger.accounts(currency), ["receivable", "revenue", "tax:reduced", "tax:standard"])
            self.assertEqual(sum(ledger.balance(a, currency) for a in ledger.accounts(currency)), 0)

    def test_an_entry_that_does_not_balance_is_refused_whole(self):
        ledger = Ledger()
        with self.assertRaises(UnbalancedEntry):
            ledger.post_entry("USD", [("receivable", 100), ("revenue", -99)])
        self.assertEqual(ledger.accounts("USD"), [])
        self.assertEqual(ledger.balance("receivable", "USD"), 0)


if __name__ == "__main__":
    unittest.main()
