"""Plain-text invoice reports."""

from billing import calc_total


def summary(invoices):
    """One line per invoice: its id and total."""
    return [f"{invoice['id']}: {calc_total(invoice['lines']):.2f}" for invoice in invoices]
