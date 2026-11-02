import unittest

from exporters.markdown import export
from report_core import Column, Report, Row, Section
from reports_for_tests import rich, stock


class MarkdownTest(unittest.TestCase):
    maxDiff = None

    def test_the_documented_example(self):
        self.assertEqual(export(stock()), "\n".join([
            "# Stock", "",
            "## North", "",
            "| sku  | qty | price |",
            "|:-----|----:|------:|",
            "| bolt |  12 |  0.35 |",
            "| nut  |     |  1.10 |",
            "| **total** | 12 | 1.45 |",
        ]) + "\n")

    def test_escapes_empty_sections_and_rounding(self):
        self.assertEqual(export(rich()), "\n".join([
            "# Odd | title", "",
            "## A", "",
            "| na\\|me       |    n |    d |",
            "|:-------------|-----:|-----:|",
            "| x\\|y         | 1000 |  1.3 |",
            "| two<br>lines |   -5 | -1.3 |",
            "|              |      |      |",
            "| **total** | 995 | 0.0 |",
            "",
            "## Empty", "",
            "_No rows._",
            "",
            "## B", "",
            "| na\\|me |   n |   d |",
            "|:-------|----:|----:|",
            "| é      |   7 | 0.1 |",
            "| **total** | 7 | 0.1 |",
        ]) + "\n")

    def test_a_report_with_no_sections_is_its_title(self):
        self.assertEqual(export(Report("T", [Column("a", "text")], [])), "# T\n")

    def test_widths_are_at_least_three(self):
        report = Report("T", [Column("a", "text"), Column("b", "int")], [Section("S", [Row(["x", 1])])])
        self.assertEqual(export(report).splitlines()[4:7], ["| a   |   b |", "|:----|----:|", "| x   |   1 |"])

    def test_a_numeric_first_column_has_no_label(self):
        report = Report("T", [Column("n", "int"), Column("t", "text")], [Section("S", [Row([1, "a"]), Row([2, "b"])])])
        self.assertEqual(export(report).splitlines()[-1], "| 3 |  |")

    def test_a_backslash_stays_as_it_is(self):
        report = Report("T", [Column("a", "text")], [Section("S", [Row(["c:\\x"])])])
        self.assertIn("| c:\\x |", export(report))

    def test_a_decimal_with_more_digits_than_a_float_holds(self):
        report = Report("T", [Column("d", "decimal", places=2)], [Section("S", [Row(["12345678901234567890.125"])])])
        self.assertIn("| 12345678901234567890.13 |", export(report))


if __name__ == "__main__":
    unittest.main()
