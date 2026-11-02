"""Price lists, one per currency, as written by the people who keep them."""

PRICES = {
    "USD": {"widget": "19.99", "gadget": "5.25", "bolt": "0.35", "washer": "0.10"},
    "JPY": {"widget": "2980", "gadget": "790", "bolt": "50", "washer": "15"},
    "KWD": {"widget": "6.125", "gadget": "1.610", "bolt": "0.105", "washer": "0.010"},
}


def price(sku, currency, prices=PRICES):
    """The unit price of SKU in CURRENCY."""
    return float(prices[currency][sku])
