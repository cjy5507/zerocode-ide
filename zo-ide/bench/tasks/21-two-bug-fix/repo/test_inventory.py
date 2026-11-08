import unittest

import inventory

ITEMS = [
    {"name": "bolt", "qty": 3, "price": 0.2},
    {"name": "nut", "qty": 10, "price": 0.1},
    {"name": "gear", "qty": 2, "price": 4.0},
]


class InventoryTest(unittest.TestCase):
    def test_total_units_counts_every_item(self):
        self.assertEqual(inventory.total_units(ITEMS), 15)

    def test_restock_includes_items_at_the_threshold(self):
        self.assertEqual(inventory.restock_needed(ITEMS, 3), ["bolt", "gear"])

    def test_price_of_unknown_is_none(self):
        self.assertIsNone(inventory.price_of(ITEMS, "washer"))


if __name__ == "__main__":
    unittest.main()
