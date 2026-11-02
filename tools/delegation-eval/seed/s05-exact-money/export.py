"""An invoice as the CSV the accounting system imports."""

from money import fmt


def render_csv(invoice):
    rows = [f"currency,{invoice.currency}", "sku,qty,unit,gross,discount,net"]
    for line in invoice.lines:
        rows.append(f"{line.sku},{line.qty},{fmt(line.unit)},{fmt(line.gross)},{fmt(line.discount)},{fmt(line.net)}")
    rows.append(f"net,,,,,{fmt(invoice.net)}")
    for name, amount in sorted(invoice.tax_by_class.items()):
        rows.append(f"tax:{name},,,,,{fmt(amount)}")
    rows.append(f"total,,,,,{fmt(invoice.total)}")
    return "\n".join(rows) + "\n"
