"""Tax classes and their rates."""

from money import round2

RATES = {"standard": "0.0825", "reduced": "0.05"}
CLASS_OF = {"widget": "standard", "gadget": "reduced", "bolt": "standard", "washer": "reduced"}


def class_of(sku):
    return CLASS_OF[sku]


def tax_on(amount, tax_class):
    """The tax on AMOUNT in TAX_CLASS."""
    return round2(amount * float(RATES[tax_class]))
