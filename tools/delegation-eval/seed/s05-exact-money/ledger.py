"""The general ledger: every entry adds up to nothing."""

ROUNDING_ACCOUNT = "rounding"


class UnbalancedEntry(ValueError):
    """An entry whose amounts do not add up to zero."""


class Ledger:
    def __init__(self):
        self._balances = {}

    def post_entry(self, currency, amounts):
        """Record AMOUNTS, a list of (account, amount), debits positive and credits negative."""
        rest = round(sum(float(amount) for _, amount in amounts), 2)
        if rest:
            amounts = [*amounts, (ROUNDING_ACCOUNT, -rest)]
        for account, amount in amounts:
            key = (account, currency)
            self._balances[key] = self._balances.get(key, 0.0) + float(amount)

    def post(self, invoice):
        amounts = [("receivable", invoice.total), ("revenue", -invoice.net)]
        amounts += [(f"tax:{name}", -amount) for name, amount in sorted(invoice.tax_by_class.items())]
        self.post_entry(invoice.currency, amounts)

    def balance(self, account, currency):
        return self._balances.get((account, currency), 0)

    def accounts(self, currency):
        return sorted(account for account, held in self._balances if held == currency)
