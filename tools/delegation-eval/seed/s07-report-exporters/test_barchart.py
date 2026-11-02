import unittest

from exporters import EXPORTERS
from exporters.barchart import export
from report_core import Column, Report, Row, Section


def report_of(*rows, sections=None):
    columns = [Column("sku", "text"), Column("qty", "int"), Column("weight", "decimal", places=1)]
    return Report("Stock", columns, sections or [Section("North", [Row(list(row)) for row in rows])])


class BarChartTest(unittest.TestCase):
    maxDiff = None

    def test_the_documented_example(self):
        self.assertEqual(export(report_of(("bolt", 12, "1.0"), ("washer", 6, "1.0"), ("nut", None, "1.0")), width=20), "\n".join([
            "Stock: qty",
            "North",
            "  bolt   ████████████████████ 12",
            "  washer ██████████ 6",
            "  nut    (unknown)",
        ]) + "\n")

    def test_half_cells(self):
        text = export(report_of(("a", 8, "1"), ("b", 3, "1"), ("c", 1, "1")), width=4)
        self.assertEqual(text.splitlines()[2:], ["  a ████ 8", "  b █▌ 3", "  c ▌ 1"])

    def test_rounding_goes_half_up_in_half_cells(self):
        text = export(report_of(("a", 16, "1"), ("b", 1, "1"), ("c", 3, "1")), width=4)
        self.assertEqual(text.splitlines()[2:], ["  a ████ 16", "  b ▌ 1", "  c █ 3"])

    def test_a_value_above_zero_never_rounds_to_nothing(self):
        text = export(report_of(("a", 1000, "1"), ("b", 1, "1")), width=4)
        self.assertEqual(text.splitlines()[3], "  b ▌ 1")

    def test_zero_and_negative_draw_no_bar_and_do_not_set_the_scale(self):
        text = export(report_of(("a", 4, "1"), ("b", 0, "1"), ("c", -100, "1")), width=4)
        self.assertEqual(text.splitlines()[2:], ["  a ████ 4", "  b 0", "  c -100"])

    def test_nothing_above_zero_means_no_bars(self):
        text = export(report_of(("a", 0, "1"), ("b", -1, "1")), width=4)
        self.assertEqual(text.splitlines()[2:], ["  a 0", "  b -1"])

    def test_a_decimal_column_keeps_the_value_as_written(self):
        text = export(report_of(("a", 1, "2.50"), ("b", 1, "1.25")), column="weight", width=4)
        self.assertEqual(text.splitlines()[0], "Stock: weight")
        self.assertEqual(text.splitlines()[2:], ["  a ████ 2.50", "  b ██ 1.25"])

    def test_decimals_scale_exactly(self):
        text = export(report_of(("a", 1, "0.3"), ("b", 1, "0.1")), column="weight", width=3)
        self.assertEqual(text.splitlines()[2:], ["  a ███ 0.3", "  b █ 0.1"])

    def test_the_scale_and_the_padding_span_every_section(self):
        sections = [Section("One", [Row(["a", 4, "1"])]), Section("Empty", []), Section("Two", [Row(["longer", 8, "1"])])]
        self.assertEqual(export(report_of(sections=sections), width=4), "\n".join([
            "Stock: qty",
            "One",
            "  a      ██ 4",
            "Empty",
            "Two",
            "  longer ████ 8",
        ]) + "\n")

    def test_a_row_with_no_known_label(self):
        text = export(report_of((None, 2, "1")), width=2)
        self.assertEqual(text.splitlines()[2], "  ? ██ 2")

    def test_the_label_is_the_first_text_cell_wherever_it_stands(self):
        report = Report("T", [Column("n", "int"), Column("t", "text")], [Section("S", [Row([2, "x"])])])
        self.assertEqual(export(report, width=2).splitlines()[2], "  x ██ 2")

    def test_columns_it_cannot_draw(self):
        for column in ("nothing", "sku"):
            with self.assertRaises(ValueError, msg=column):
                export(report_of(("a", 1, "1")), column=column)
        with self.assertRaises(ValueError):
            export(Report("T", [Column("t", "text")], [Section("S", [Row(["a"])])]))

    def test_every_exporter_is_listed(self):
        self.assertEqual(sorted(EXPORTERS), ["barchart", "jsonl", "markdown", "tsv"])
        self.assertIs(EXPORTERS["barchart"], export)


if __name__ == "__main__":
    unittest.main()
