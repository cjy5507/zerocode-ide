"""An order worked out into an invoice."""

import catalog
import tax
from money import round2


class Line:
    def __init__(self, sku, qty, unit, gross, discount):
        self.sku = sku
        self.qty = qty
        self.unit = unit
        self.gross = gross
        self.discount = discount
        self.net = round2(gross - discount)


class Invoice:
    def __init__(self, currency, lines, tax_by_class):
        self.currency = currency
        self.lines = lines
        self.tax_by_class = tax_by_class

    @property
    def net(self):
        return round2(sum(line.net for line in self.lines))

    @property
    def tax(self):
        return round2(sum(self.tax_by_class.values()))

    @property
    def total(self):
        return round2(self.net + self.tax)


def build_invoice(order):
    """ORDER is {"currency", "items": [(sku, qty), ...], "discount_percent": "12.5"}."""
    currency = order["currency"]
    percent = float(order.get("discount_percent", "0"))
    lines = []
    tax_by_class = {}
    for sku, qty in order["items"]:
        unit = catalog.price(sku, currency, order.get("prices", catalog.PRICES))
        gross = round2(unit * qty)
        line = Line(sku, qty, unit, gross, round2(gross * percent / 100))
        lines.append(line)
        tax_class = tax.class_of(sku)
        tax_by_class[tax_class] = round2(tax_by_class.get(tax_class, 0) + tax.tax_on(line.net, tax_class))
    return Invoice(currency, lines, tax_by_class)
