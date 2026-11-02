"""The reports the exporter tests share."""

from report_core import Column, Report, Row, Section


def stock():
    return Report(
        "Stock",
        [Column("sku", "text"), Column("qty", "int"), Column("price", "decimal", places=2)],
        [Section("North", [Row(["bolt", 12, "0.35"]), Row(["nut", None, "1.10"])])],
    )


def rich():
    return Report(
        "Odd | title",
        [Column("na|me", "text"), Column("n", "int"), Column("d", "decimal", places=1)],
        [
            Section("A", [Row(["x|y", 1000, "1.25"]), Row(["two\nlines", -5, "-1.25"]), Row([None, None, None])]),
            Section("Empty", []),
            Section("B", [Row(["é", 7, "0.05"])]),
        ],
    )
