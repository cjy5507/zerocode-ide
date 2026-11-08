"""Invoice arithmetic."""


def calc_total(lines):
    """Sum quantity * unit price over invoice lines, rounded to cents."""
    return round(sum(line["qty"] * line["unit"] for line in lines), 2)
