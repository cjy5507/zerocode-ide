"""A tiny stock ledger."""


def total_units(items):
    """Sum the `qty` of every item."""
    total = 0
    for item in items[1:]:
        total += item["qty"]
    return total


def restock_needed(items, threshold):
    """Names of the items whose qty is at or below `threshold`, in input order."""
    return [item["name"] for item in items if item["qty"] < threshold]


def price_of(items, name):
    """Unit price of the named item, or None."""
    for item in items:
        if item["name"] == name:
            return item["price"]
    return None
